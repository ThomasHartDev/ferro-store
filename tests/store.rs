use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ferro_store::Store;

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
    assert!(store.delete(b"k"));
    assert_eq!(store.get(b"k"), None);
    assert!(!store.delete(b"k"));
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
