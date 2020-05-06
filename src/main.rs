/*
 * Copyright 2019 Joyent, Inc.
 */

#[macro_use]
extern crate serde_json;

#[macro_use]
extern crate failure;

use failure::Error;
use libmanta::moray::{MantaObject, MantaObjectShark};
use moray::buckets;
use moray::client::MorayClient;
use moray::objects::{self, BatchPutOp, BatchRequest};
use quickcheck::{Arbitrary, StdThreadGen};
use serde_json::Value;
use slog::{o, Drain, Logger};
use std::collections::HashMap;
use std::sync::Mutex;

use rand::distributions::Alphanumeric;
use rand::seq::SliceRandom;
use rand::{thread_rng, Rng};
use std::net::{IpAddr, SocketAddr};

// We can't use trust-dns-resolver here because it uses futures with a
// block_on, and calling a block_on from within a block_on is not allowed.
use resolve::resolve_host;
use resolve::{record::Srv, DnsConfig, DnsResolver};

static BUCKET_NAME: &str = "rust_batch_test_bucket";

#[derive(Debug, Fail)]
enum InternalError {
    #[fail(display = "catchall")]
    CatchAll,
}

// Get the SRV record which gives us the target and port of the moray service.
fn get_srv_record(svc: &str, proto: &str, host: &str) -> Result<Srv, Error> {
    let query = format!("{}.{}.{}", svc, proto, host);
    let r = DnsResolver::new(DnsConfig::load_default()?)?;
    r.resolve_record::<Srv>(&query)?
        .choose(&mut rand::thread_rng())
        .map(|r| r.to_owned())
        .ok_or_else(|| InternalError::CatchAll.into())
}

fn lookup_ip(host: &str) -> Result<IpAddr, Error> {
    match resolve_host(host)?.collect::<Vec<IpAddr>>().first() {
        Some(a) => Ok(*a),
        None => Err(InternalError::CatchAll.into()),
    }
}

fn get_moray_srv_sockaddr(host: &str) -> Result<SocketAddr, Error> {
    let srv_record = get_srv_record("_moray", "_tcp", &host)?;
    dbg!(&srv_record);

    let ip = lookup_ip(&srv_record.target)?;

    Ok(SocketAddr::new(ip, srv_record.port))
}

// Create a moray client using the shard and the domain name only.  This will
// query binder for the SRV record for us.
pub fn create_client(shard: u32, domain: &str) -> Result<MorayClient, Error> {
    let domain_name = format!("{}.moray.{}", shard, domain);
    let sock_addr = get_moray_srv_sockaddr(&domain_name)?;
    let plain = slog_term::PlainSyncDecorator::new(std::io::sink());
    let log = Logger::root(
        Mutex::new(slog_term::FullFormat::new(plain).build()).fuse(),
        o!("build-id" => "0.1.0"),
    );

    MorayClient::new(sock_addr, log, None).map_err(Error::from)
}

fn random_string(len: usize) -> String {
    thread_rng().sample_iter(&Alphanumeric).take(len).collect()
}

fn gen_test_objects(num_objects: u32) -> HashMap<String, MantaObject> {
    let mut test_objects = HashMap::new();
    let mut g = StdThreadGen::new(10);
    let mut rng = rand::thread_rng();

    for _ in 0..num_objects {
        let mut mobj = MantaObject::arbitrary(&mut g);
        let mut sharks = vec![];

        // first pass: 1 or 2
        // second pass: 3 or 4
        for i in 0..2 {
            let shark_num = rng.gen_range(1 + i * 2, 3 + i * 2);

            let shark = MantaObjectShark {
                datacenter: String::from("foo"), //todo
                manta_storage_id: format!("{}.stor.domain", shark_num),
            };
            sharks.push(shark);
        }
        mobj.sharks = sharks;

        test_objects.insert(mobj.object_id.clone(), mobj);
    }

    test_objects
}

fn main() -> Result<(), Error> {
    let opts = objects::MethodOptions::default();
    let bucket_opts = buckets::MethodOptions::default();
    let mut mclient = create_client(1, "perf2.scloud.host")?;

    let ignore_callback = |_bucket: &buckets::Bucket| Ok(());

    println!("===get or create bucket===");
    if mclient
        .get_bucket(BUCKET_NAME, bucket_opts.clone(), ignore_callback)
        .is_err()
    {
        let bucket_config = json!({
            "index": {
                "dirname": {
                  "type": "string"
                },
                "name": {
                  "type": "string"
                },
                "owner": {
                  "type": "string"
                },
                "objectId": {
                  "type": "string"
                },
                "type": {
                  "type": "string"
                }
            }
        });

        match mclient.create_bucket(BUCKET_NAME, bucket_config, bucket_opts) {
            Ok(()) => {
                println!("Bucket Created Successfully");
            }
            Err(e) => {
                eprintln!("Error Creating Bucket: {}", e);
            }
        }
    }

    println!("Creating test objects");
    let test_objects = gen_test_objects(10000);

    println!("Seeding objects");

    for (key, obj) in test_objects.iter() {
        let val = serde_json::to_value(obj).unwrap();

        mclient
            .put_object(BUCKET_NAME, key, val, &opts, |_| Ok(()))
            .expect("put object");
    }

    println!(" ==== pass 1, sequential first then batch ====");

    let altered_objects = alter_objects(&test_objects);
    run_sequential_test(&mut mclient, altered_objects)?;

    let batch_objects = alter_objects(&test_objects);
    run_batch_test(&mut mclient, batch_objects, 50)?;

    println!("\n ==== pass 2, batch first then sequential ====");

    let batch_objects = alter_objects(&test_objects);
    run_batch_test(&mut mclient, batch_objects, 50)?;

    let seq_objects = alter_objects(&test_objects);
    run_sequential_test(&mut mclient, seq_objects)?;

    Ok(())
}

fn run_sequential_test(
    mclient: &mut MorayClient,
    objects: HashMap<String, Value>,
) -> Result<(), Error> {
    println!("Updating objects sequentially");
    let opts = objects::MethodOptions::default();
    let start = std::time::Instant::now();
    for (key, obj) in objects.iter() {
        mclient
            .put_object(BUCKET_NAME, key, obj.clone(), &opts, |_| Ok(()))
            .expect("put object");
    }
    println!(
        "Done updating objects sequentially : {}ms",
        start.elapsed().as_millis()
    );

    Ok(())
}

fn alter_objects(objects: &HashMap<String, MantaObject>) -> HashMap<String, Value> {
    let mut rng = rand::thread_rng();
    let mut altered_objects: HashMap<String, Value> = HashMap::new();
    let rand_string = random_string(10);
    let rand_id: u16 = rng.gen();

    println!(
        "Altering objects.  datacenter: {} | storage id: {}",
        rand_string, rand_id
    );

    for (k, v) in objects.iter() {
        let mut mobj: MantaObject = v.clone();
        mobj.sharks.pop();
        let shark = MantaObjectShark {
            datacenter: rand_string.clone(),
            manta_storage_id: format!("{}.stor.domain", rand_id),
        };
        mobj.sharks.push(shark);

        let mobj_value = serde_json::to_value(mobj).unwrap();
        altered_objects.insert(k.clone(), mobj_value);
    }
    altered_objects
}

fn run_batch_test(
    mclient: &mut MorayClient,
    objects: HashMap<String, Value>,
    batch_size: u32,
) -> Result<(), Error> {
    println!("Updating objects in batches of {}", batch_size);
    let mut batch: Vec<BatchRequest> = vec![];
    let mut batch_count = 0;
    let opts = objects::MethodOptions::default();
    let start = std::time::Instant::now();

    for (key, value) in objects.iter() {
        batch.push(BatchRequest::Put(BatchPutOp {
            bucket: BUCKET_NAME.to_string(),
            options: opts.clone(),
            key: key.clone(),
            value: value.clone(),
        }));

        batch_count += 1;

        if batch_count == batch_size {
            mclient.batch(&batch, &opts, |_| Ok(()))?;
            batch.clear();
        }
    }

    println!(
        "Done updating objects in batches: {}ms",
        start.elapsed().as_millis()
    );

    Ok(())
}
