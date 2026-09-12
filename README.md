# ferro-store

An embedded key-value storage engine in Rust, built toward an LSM-tree (memtable, SSTables, compaction) with a write-ahead log and crash recovery.

## What this demonstrates

Production stores such as RocksDB, LevelDB, and Bitcask separate an in-memory write path from durable on-disk structures. This crate starts with that split: a `Store` that owns a sorted memtable, logs every mutation to a versioned WAL, and returns borrowed slices for in-memory reads so callers do not copy bytes they only need to inspect. Later work adds SSTable flush and compaction.

## Concepts demonstrated

- Embedded key-value API (byte keys and values, last-write-wins)
- Memtable as an ordered `BTreeMap` (the in-process stand-in for a skiplist)
- Ownership: the store owns buffers; `get` borrows them (zero-copy in-memory reads)
- Write-ahead logging: log the mutation, `fsync`, then apply to the memtable
- WAL superblock: 32-byte magic + format version so garbage and future layouts are rejected
- Log sequence numbers (LSN): monotonic 64-bit ids, LSN 0 reserved as "no previous"
- ARIES-style prevLSN chain: each record stores the prior LSN; a hole is corruption
- Length-prefixed record framing (`crc32` + `len` + `lsn` + `prev_lsn` + payload)
- CRC-32/ISO-HDLC checksums over the LSN prefix and payload
- Torn-write detection: a short tail or a bad checksum on the last record is discarded and the file is truncated to the last good offset
- Mid-log checksum failure is corruption, not a torn write
- Directory `fsync` after creating `wal.log` so the directory entry survives a crash
- Crash recovery by replaying the WAL on `Store::open`
- Rust crate layout, `cargo test`, Clippy `-D warnings`, GitHub Actions CI

## What's implemented

- Cargo scaffold, the embedded KV API, CI (`cargo test` + `clippy`)
- Write-ahead log with CRC-32 framing, `fsync` on each mutation, and crash recovery on open
- WAL superblock, monotonic LSNs with a prevLSN chain, and directory fsync on log create

## Usage

```rust
use ferro_store::Store;

let mut store = Store::open("/tmp/ferro-demo")?;
store.put(b"user:1", b"ada")?;
assert_eq!(store.get(b"user:1"), Some(b"ada".as_slice()));
assert_eq!(store.last_lsn(), 1);
drop(store);

let store = Store::open("/tmp/ferro-demo")?;
assert_eq!(store.get(b"user:1"), Some(b"ada".as_slice()));
assert_eq!(store.last_lsn(), 1);
```

## Tests

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```
