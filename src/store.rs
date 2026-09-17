use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::memtable::Memtable;
use crate::wal::{Record, Wal};

#[derive(Clone, Debug)]
pub struct Options {
    write_buffer_size: usize,
}

impl Options {
    pub const DEFAULT_WRITE_BUFFER: usize = 4 * 1024 * 1024;

    pub fn new() -> Self {
        Self {
            write_buffer_size: Self::DEFAULT_WRITE_BUFFER,
        }
    }

    pub fn write_buffer_size(mut self, bytes: usize) -> Self {
        self.write_buffer_size = bytes;
        self
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    mem: Memtable,
    wal: Wal,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, Options::new())
    }

    pub fn open_with(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        let mut mem = Memtable::new(options.write_buffer_size)?;
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let (wal, records) = Wal::open(&dir)?;
        for rec in records {
            match rec {
                Record::Put { key, value } => mem.put(key, value),
                Record::Delete { key } => {
                    let _ = mem.delete(&key);
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

    pub fn write_buffer_size(&self) -> usize {
        self.mem.write_buffer_size()
    }

    pub fn size_bytes(&self) -> usize {
        self.mem.size_bytes()
    }

    pub fn should_flush(&self) -> bool {
        self.mem.should_flush()
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.wal.append_put(key, value)?;
        self.mem.put(key.to_vec(), value.to_vec());
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.mem.get(key)
    }

    pub fn delete(&mut self, key: &[u8]) -> Result<bool> {
        self.wal.append_delete(key)?;
        Ok(self.mem.delete(key))
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
        self.mem.iter()
    }
}
