use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ferro_store::{Error, Options, Store};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "ferro-store-{}-{}-{}",
        std::process::id(),
        nanos,
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open() -> (Store, Guard) {
    let dir = scratch();
    let store = Store::open(&dir).expect("open");
    (store, Guard(dir))
}

#[test]
fn open_creates_directory() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    assert!(!dir.exists());
    let store = Store::open(&dir).expect("open");
    assert!(dir.is_dir());
    assert_eq!(store.path(), dir.as_path());
}

#[test]
fn put_get_round_trip() {
    let (mut store, _g) = open();
    store.put(b"alpha", b"one").unwrap();
    assert_eq!(store.get(b"alpha"), Some(b"one".as_slice()));
    assert_eq!(store.len(), 1);
}

#[test]
fn missing_key_is_none() {
    let (store, _g) = open();
    assert_eq!(store.get(b"missing"), None);
    assert!(!store.contains_key(b"missing"));
    assert!(store.is_empty());
}

#[test]
fn overwrite_last_write_wins() {
    let (mut store, _g) = open();
    store.put(b"k", b"v1").unwrap();
    store.put(b"k", b"v2").unwrap();
    assert_eq!(store.get(b"k"), Some(b"v2".as_slice()));
    assert_eq!(store.len(), 1);
}

#[test]
fn delete_existing_and_missing() {
    let (mut store, _g) = open();
    store.put(b"k", b"v").unwrap();
    assert!(store.delete(b"k").unwrap());
    assert_eq!(store.get(b"k"), None);
    assert!(!store.delete(b"k").unwrap());
}

#[test]
fn empty_key_and_empty_value() {
    let (mut store, _g) = open();
    store.put(b"", b"").unwrap();
    assert_eq!(store.get(b""), Some(b"".as_slice()));
    store.put(b"", b"x").unwrap();
    assert_eq!(store.get(b""), Some(b"x".as_slice()));
}

#[test]
fn binary_keys_and_values() {
    let (mut store, _g) = open();
    let key = [0u8, 1, 255, 10];
    let val = [255u8, 0, 128];
    store.put(&key, &val).unwrap();
    assert_eq!(store.get(&key), Some(val.as_slice()));
}

#[test]
fn iter_is_sorted_by_key() {
    let (mut store, _g) = open();
    store.put(b"c", b"3").unwrap();
    store.put(b"a", b"1").unwrap();
    store.put(b"b", b"2").unwrap();
    let keys: Vec<&[u8]> = store.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec![b"a".as_slice(), b"b", b"c"]);
}

#[test]
fn get_borrows_from_store() {
    let (mut store, _g) = open();
    store.put(b"k", b"value").unwrap();
    let got = store.get(b"k").unwrap();
    assert_eq!(got, b"value");
}

fn open_with(bytes: usize) -> (Store, Guard) {
    let dir = scratch();
    let store = Store::open_with(&dir, Options::new().write_buffer_size(bytes)).expect("open");
    (store, Guard(dir))
}

#[test]
fn default_write_buffer_is_four_mib() {
    let (store, _g) = open();
    assert_eq!(store.write_buffer_size(), Options::DEFAULT_WRITE_BUFFER);
    assert_eq!(store.size_bytes(), 0);
    assert!(!store.should_flush());
}

#[test]
fn zero_write_buffer_is_rejected() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    assert!(!dir.exists());
    let err = Store::open_with(&dir, Options::new().write_buffer_size(0)).unwrap_err();
    assert!(
        matches!(err, Error::InvalidArgument("write_buffer_size must be > 0")),
        "{err:?}"
    );
    assert!(!dir.join("wal.log").exists());
    assert!(!dir.exists());
}

#[test]
fn zero_write_buffer_does_not_touch_an_existing_dir() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("keep-me");
    fs::write(&marker, b"ok").unwrap();
    let err = Store::open_with(&dir, Options::new().write_buffer_size(0)).unwrap_err();
    assert!(
        matches!(err, Error::InvalidArgument("write_buffer_size must be > 0")),
        "{err:?}"
    );
    assert!(!dir.join("wal.log").exists());
    assert_eq!(fs::read(&marker).unwrap(), b"ok");
}

#[test]
fn size_bytes_is_key_plus_value_payload() {
    let (mut store, _g) = open_with(64);
    store.put(b"user:1", b"ada").unwrap();
    assert_eq!(store.size_bytes(), 9);
}

#[test]
fn size_tracks_put_overwrite_and_delete() {
    let (mut store, _g) = open_with(64);
    store.put(b"ab", b"c").unwrap();
    assert_eq!(store.size_bytes(), 3);
    store.put(b"ab", b"cdef").unwrap();
    assert_eq!(store.size_bytes(), 6);
    store.put(b"ab", b"x").unwrap();
    assert_eq!(store.size_bytes(), 3);
    assert!(store.delete(b"ab").unwrap());
    assert_eq!(store.size_bytes(), 0);
}

#[test]
fn should_flush_at_threshold() {
    let (mut store, _g) = open_with(4);
    store.put(b"abc", b"").unwrap();
    assert!(!store.should_flush());
    store.put(b"d", b"").unwrap();
    assert_eq!(store.size_bytes(), 4);
    assert!(store.should_flush());
}

#[test]
fn replay_rebuilds_the_same_size_bytes() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let opts = Options::new().write_buffer_size(8);
    let (live_size, flush) = {
        let mut store = Store::open_with(&dir, opts.clone()).unwrap();
        store.put(b"aa", b"bb").unwrap();
        store.put(b"c", b"ddd").unwrap();
        store.put(b"aa", b"z").unwrap();
        store.delete(b"c").unwrap();
        store.put(b"", b"empty-key").unwrap();
        (store.size_bytes(), store.should_flush())
    };
    let store = Store::open_with(&dir, opts).unwrap();
    assert_eq!(store.size_bytes(), live_size);
    assert_eq!(store.should_flush(), flush);
    assert_eq!(store.get(b"aa"), Some(b"z".as_slice()));
    assert_eq!(store.get(b"c"), None);
    assert_eq!(store.get(b""), Some(b"empty-key".as_slice()));
    assert_eq!(live_size, 12);
    assert!(store.should_flush());
}
