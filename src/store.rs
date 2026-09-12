use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::wal::{Record, Wal};

#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    mem: BTreeMap<Vec<u8>, Vec<u8>>,
    wal: Wal,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let (wal, records) = Wal::open(&dir)?;
        let mut mem = BTreeMap::new();
        for rec in records {
            match rec {
                Record::Put { key, value } => {
                    mem.insert(key, value);
                }
                Record::Delete { key } => {
                    mem.remove(&key);
                }
            }
        }
        Ok(Self { dir, mem, wal })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn last_lsn(&self) -> u64 {
        self.wal.last_lsn()
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.wal.append_put(key, value)?;
        self.mem.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.mem.get(key).map(Vec::as_slice)
    }

    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        self.wal.append_delete(key)?;
        Ok(self.mem.remove(key).is_some())
    }

    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.mem.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.mem.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mem.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.mem.iter().map(|(k, v)| (k.as_slice(), v.as_slice()))
    }
}
