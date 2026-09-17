use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct Memtable {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    size_bytes: usize,
    write_buffer_size: usize,
}

impl Memtable {
    pub fn new(write_buffer_size: usize) -> Result<Self> {
        if write_buffer_size == 0 {
            return Err(Error::InvalidArgument("write_buffer_size must be > 0"));
        }
        Ok(Self {
            map: BTreeMap::new(),
            size_bytes: 0,
            write_buffer_size,
        })
    }

    pub fn write_buffer_size(&self) -> usize {
        self.write_buffer_size
    }

    pub fn size_bytes(&self) -> usize {
        self.size_bytes
    }

    pub fn should_flush(&self) -> bool {
        self.size_bytes >= self.write_buffer_size
    }

    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let new_val_len = value.len();
        match self.map.entry(key) {
            Entry::Occupied(mut occ) => {
                let key_len = occ.key().len();
                let old = occ.insert(value);
                self.adjust(
                    key_len.saturating_add(old.len()),
                    key_len.saturating_add(new_val_len),
                );
            }
            Entry::Vacant(vac) => {
                let added = vac.key().len().saturating_add(new_val_len);
                vac.insert(value);
                self.adjust(0, added);
            }
        }
    }

    pub fn delete(&mut self, key: &[u8]) -> bool {
        match self.map.remove(key) {
            Some(old) => {
                self.adjust(key.len().saturating_add(old.len()), 0);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.map.get(key).map(Vec::as_slice)
    }

    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.map.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.map.iter().map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    fn adjust(&mut self, old_bytes: usize, new_bytes: usize) {
        // overwrite can shrink; saturating_sub keeps a logic bug from wrapping the counter
        self.size_bytes = self
            .size_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::Memtable;

    #[test]
    fn rejects_zero_write_buffer() {
        assert!(Memtable::new(0).is_err());
    }

    #[test]
    fn empty_has_zero_size() {
        let mem = Memtable::new(64).unwrap();
        assert_eq!(mem.size_bytes(), 0);
        assert!(!mem.should_flush());
        assert!(mem.is_empty());
        assert_eq!(mem.get(b"missing"), None);
    }

    #[test]
    fn put_charges_key_and_value() {
        let mut mem = Memtable::new(64).unwrap();
        mem.put(b"abc".to_vec(), b"xy".to_vec());
        assert_eq!(mem.size_bytes(), 5);
        assert_eq!(mem.get(b"abc"), Some(b"xy".as_slice()));
        assert_eq!(mem.len(), 1);
    }

    #[test]
    fn overwrite_grows_and_shrinks() {
        let mut mem = Memtable::new(64).unwrap();
        mem.put(b"k".to_vec(), b"aa".to_vec());
        assert_eq!(mem.size_bytes(), 3);
        mem.put(b"k".to_vec(), b"bbbb".to_vec());
        assert_eq!(mem.size_bytes(), 5);
        mem.put(b"k".to_vec(), b"c".to_vec());
        assert_eq!(mem.size_bytes(), 2);
        assert_eq!(mem.len(), 1);
        assert_eq!(mem.get(b"k"), Some(b"c".as_slice()));
    }

    #[test]
    fn overwrite_same_size_is_stable() {
        let mut mem = Memtable::new(64).unwrap();
        mem.put(b"k".to_vec(), b"ab".to_vec());
        mem.put(b"k".to_vec(), b"cd".to_vec());
        assert_eq!(mem.size_bytes(), 3);
    }

    #[test]
    fn delete_releases_bytes_missing_is_noop() {
        let mut mem = Memtable::new(64).unwrap();
        mem.put(b"a".to_vec(), b"1".to_vec());
        mem.put(b"bb".to_vec(), b"22".to_vec());
        assert_eq!(mem.size_bytes(), 6);
        assert!(mem.delete(b"a"));
        assert_eq!(mem.size_bytes(), 4);
        assert!(!mem.delete(b"a"));
        assert_eq!(mem.size_bytes(), 4);
        assert!(mem.delete(b"bb"));
        assert_eq!(mem.size_bytes(), 0);
        assert!(mem.is_empty());
    }

    #[test]
    fn empty_key_and_value_charge_zero() {
        let mut mem = Memtable::new(1).unwrap();
        mem.put(Vec::new(), Vec::new());
        assert_eq!(mem.size_bytes(), 0);
        assert!(!mem.should_flush());
        assert_eq!(mem.get(b""), Some(b"".as_slice()));
        assert!(mem.delete(b""));
        assert_eq!(mem.size_bytes(), 0);
    }

    #[test]
    fn flush_signal_at_and_over_threshold() {
        let mut mem = Memtable::new(4).unwrap();
        mem.put(b"ab".to_vec(), b"c".to_vec());
        assert_eq!(mem.size_bytes(), 3);
        assert!(!mem.should_flush());
        mem.put(b"d".to_vec(), Vec::new());
        assert_eq!(mem.size_bytes(), 4);
        assert!(mem.should_flush());
        mem.put(b"e".to_vec(), b"f".to_vec());
        assert_eq!(mem.size_bytes(), 6);
        assert!(mem.should_flush());
    }

    #[test]
    fn delete_can_drop_below_threshold() {
        let mut mem = Memtable::new(4).unwrap();
        mem.put(b"abcd".to_vec(), Vec::new());
        assert!(mem.should_flush());
        mem.delete(b"abcd");
        assert!(!mem.should_flush());
        assert_eq!(mem.size_bytes(), 0);
    }

    #[test]
    fn iter_is_sorted() {
        let mut mem = Memtable::new(64).unwrap();
        mem.put(b"c".to_vec(), b"3".to_vec());
        mem.put(b"a".to_vec(), b"1".to_vec());
        mem.put(b"b".to_vec(), b"2".to_vec());
        let keys: Vec<&[u8]> = mem.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![b"a".as_slice(), b"b", b"c"]);
    }

    #[test]
    fn binary_keys() {
        let mut mem = Memtable::new(64).unwrap();
        let key = vec![0u8, 255, 1];
        mem.put(key.clone(), vec![9, 8]);
        assert_eq!(mem.size_bytes(), 5);
        assert_eq!(mem.get(&key), Some([9, 8].as_slice()));
        assert!(mem.contains_key(&key));
    }
}
