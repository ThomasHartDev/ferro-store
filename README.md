# ferro-store

An embedded key-value storage engine in Rust, built toward an LSM-tree (memtable, SSTables, compaction) with a write-ahead log and crash recovery.

## What this demonstrates

Production stores such as RocksDB, LevelDB, and Bitcask separate an in-memory write path from durable on-disk structures. This crate starts with that split: a `Store` that owns a sorted memtable, exposes a small embedded KV API (`put` / `get` / `delete`), and returns borrowed slices for in-memory reads so callers do not copy bytes they only need to inspect. Later work adds a WAL, SSTable flush, and compaction.

## Concepts demonstrated

- Embedded key-value API (byte keys and values, last-write-wins)
- Memtable as an ordered `BTreeMap` (the in-process stand-in for a skiplist)
- Ownership: the store owns buffers; `get` borrows them (zero-copy in-memory reads)
- Directory-backed open path, ready for WAL and SSTable files
- Rust crate layout, `cargo test`, Clippy `-D warnings`, GitHub Actions CI

## What's implemented

- Cargo scaffold, the embedded KV API, CI (`cargo test` + `clippy`)

## Usage

```rust
use ferro_store::Store;

let mut store = Store::open("/tmp/ferro-demo")?;
store.put(b"user:1", b"ada")?;
assert_eq!(store.get(b"user:1"), Some(b"ada".as_slice()));
store.delete(b"user:1");
```

## Tests

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```
