use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ferro_store::{Error, Store};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("ferro-wal-{}-{}-{}", std::process::id(), nanos, n));
    let _ = fs::remove_dir_all(&dir);
    dir
}

struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const FILE_HEADER_LEN: usize = 32;

fn wal_path(dir: &std::path::Path) -> PathBuf {
    dir.join("wal.log")
}

#[test]
fn reopen_replays_puts_and_deletes() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"a", b"1").unwrap();
        store.put(b"b", b"2").unwrap();
        store.delete(b"a").unwrap();
        store.put(b"c", b"3").unwrap();
        store.put(b"", b"empty-key").unwrap();
    }
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"a"), None);
    assert_eq!(store.get(b"b"), Some(b"2".as_slice()));
    assert_eq!(store.get(b"c"), Some(b"3".as_slice()));
    assert_eq!(store.get(b""), Some(b"empty-key".as_slice()));
    assert_eq!(store.len(), 3);
    assert_eq!(store.last_lsn(), 5);
}

#[test]
fn last_write_wins_across_reopen() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"k", b"v1").unwrap();
        store.put(b"k", b"v2").unwrap();
    }
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"k"), Some(b"v2".as_slice()));
}

#[test]
fn torn_tail_is_discarded_then_appends_resume() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"keep", b"yes").unwrap();
        store.put(b"lost", b"no").unwrap();
    }
    let path = wal_path(&dir);
    let len = fs::metadata(&path).unwrap().len();
    assert!(len > 3);
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(len - 3)
        .unwrap();

    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"keep"), Some(b"yes".as_slice()));
    assert_eq!(store.get(b"lost"), None);
    assert_eq!(store.last_lsn(), 1);

    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"after", b"ok").unwrap();
        assert_eq!(store.last_lsn(), 2);
    }
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"keep"), Some(b"yes".as_slice()));
    assert_eq!(store.get(b"after"), Some(b"ok".as_slice()));
    assert_eq!(store.get(b"lost"), None);
    assert_eq!(store.last_lsn(), 2);
}

#[test]
fn last_record_length_past_eof_is_torn() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"keep", b"yes").unwrap();
        store.put(b"lost", b"no").unwrap();
    }
    let path = wal_path(&dir);
    let mut bytes = fs::read(&path).unwrap();
    let at = last_record_len_offset(&bytes);
    bytes[at..at + 4].copy_from_slice(&20_000_000u32.to_le_bytes());
    fs::write(&path, &bytes).unwrap();

    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"keep"), Some(b"yes".as_slice()));
    assert_eq!(store.get(b"lost"), None);
}

fn last_record_len_offset(bytes: &[u8]) -> usize {
    let mut i = FILE_HEADER_LEN;
    let mut last = None;
    while i + 8 <= bytes.len() {
        let rec_len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        last = Some(i + 4);
        match i.checked_add(8).and_then(|n| n.checked_add(rec_len)) {
            Some(next) if next <= bytes.len() => i = next,
            _ => break,
        }
    }
    last.expect("wal has a framed record")
}

#[test]
fn checksum_mismatch_on_last_record_is_torn() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"only", b"one").unwrap();
    }
    let path = wal_path(&dir);
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();

    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"only"), None);
    assert!(store.is_empty());
}

#[test]
fn checksum_mismatch_in_middle_is_corrupt() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"first", b"1").unwrap();
        store.put(b"second", b"2").unwrap();
    }
    let path = wal_path(&dir);
    let mut bytes = fs::read(&path).unwrap();
    bytes[FILE_HEADER_LEN + 12] ^= 0xFF;
    fs::write(&path, &bytes).unwrap();

    let err = Store::open(&dir).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                reason: "checksum mismatch",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn empty_wal_opens_empty_store() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let store = Store::open(&dir).unwrap();
    assert!(store.is_empty());
    assert_eq!(store.last_lsn(), 0);
    assert!(wal_path(&dir).is_file());
    assert_eq!(
        fs::metadata(wal_path(&dir)).unwrap().len(),
        FILE_HEADER_LEN as u64
    );
}

#[test]
fn binary_payload_survives_reopen() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let key = [0u8, 255, 1, 2];
    let val = vec![0u8; 1024];
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(&key, &val).unwrap();
    }
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(&key), Some(val.as_slice()));
}

#[test]
fn header_only_torn_write_is_ignored() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"keep", b"yes").unwrap();
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(wal_path(&dir))
        .unwrap();
    file.write_all(&[0x11, 0x22, 0x33]).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let store = Store::open(&dir).unwrap();
    assert_eq!(store.get(b"keep"), Some(b"yes".as_slice()));
}

#[test]
fn last_lsn_survives_reopen_and_continues() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"a", b"1").unwrap();
        store.put(b"b", b"2").unwrap();
        assert_eq!(store.last_lsn(), 2);
    }
    {
        let mut store = Store::open(&dir).unwrap();
        assert_eq!(store.last_lsn(), 2);
        store.delete(b"a").unwrap();
        assert_eq!(store.last_lsn(), 3);
    }
    let store = Store::open(&dir).unwrap();
    assert_eq!(store.last_lsn(), 3);
    assert_eq!(store.get(b"a"), None);
    assert_eq!(store.get(b"b"), Some(b"2".as_slice()));
}

#[test]
fn torn_superblock_recovers_as_empty() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let _ = Store::open(&dir).unwrap();
    fs::write(wal_path(&dir), [0x11u8, 0x22, 0x33]).unwrap();
    let store = Store::open(&dir).unwrap();
    assert!(store.is_empty());
    assert_eq!(store.last_lsn(), 0);
}

#[test]
fn bad_magic_is_corrupt() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let _ = Store::open(&dir).unwrap();
    let mut bytes = fs::read(wal_path(&dir)).unwrap();
    bytes[0] = b'X';
    fs::write(wal_path(&dir), &bytes).unwrap();
    let err = Store::open(&dir).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                reason: "bad wal magic",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn unsupported_version_is_corrupt() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    let _ = Store::open(&dir).unwrap();
    let mut bytes = fs::read(wal_path(&dir)).unwrap();
    bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
    fs::write(wal_path(&dir), &bytes).unwrap();
    let err = Store::open(&dir).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                reason: "unsupported wal version",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn swapped_records_break_lsn_chain() {
    let dir = scratch();
    let _g = Guard(dir.clone());
    {
        let mut store = Store::open(&dir).unwrap();
        store.put(b"first", b"1").unwrap();
        store.put(b"second", b"2").unwrap();
    }
    let path = wal_path(&dir);
    let bytes = fs::read(&path).unwrap();
    let recs = framed_records(&bytes);
    assert_eq!(recs.len(), 2);
    let mut out = bytes[..FILE_HEADER_LEN].to_vec();
    out.extend_from_slice(&recs[1]);
    out.extend_from_slice(&recs[0]);
    fs::write(&path, &out).unwrap();

    let err = Store::open(&dir).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Corrupt {
                reason: "lsn chain broken",
                ..
            }
        ),
        "{err:?}"
    );
}

fn framed_records(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut i = FILE_HEADER_LEN;
    let mut recs = Vec::new();
    while i + 8 <= bytes.len() {
        let rec_len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        let end = i + 8 + rec_len;
        if end > bytes.len() {
            break;
        }
        recs.push(bytes[i..end].to_vec());
        i = end;
    }
    recs
}
