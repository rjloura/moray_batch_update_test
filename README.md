# Moray Batch Test
## Usage
```
cargo run
```

## Sample Output
```
===get or create bucket===
Bucket Created Successfully
Creating test objects
Seeding objects
==== pass 1, sequential first then batch ====
Altering objects.  datacenter: Rd4O6L325k | storage id: 37586
Updating objects sequentially
Done updating objects sequentially : 601805ms
Altering objects.  datacenter: jJN1BuossN | storage id: 52616
Updating objects in batches of 50
Done updating objects in batches: 632ms

==== pass 2, batch first then sequential ====
Altering objects.  datacenter: bcSLpkXB0F | storage id: 51050
Updating objects in batches of 50
Done updating objects in batches: 459ms
Altering objects.  datacenter: mSzOTvwm5Q | storage id: 62365
Updating objects sequentially
Done updating objects sequentially : 601694ms
```

