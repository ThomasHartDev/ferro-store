use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;

pub struct Store {
    dir: PathBuf,
    mem: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            mem: BTreeMap::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.mem.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.mem.get(key).map(Vec::as_slice)
    }

    pub fn delete(&mut self, key: &[u8]) -> bool {
        self.mem.remove(key).is_some()
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
