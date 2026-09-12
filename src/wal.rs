use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::crc::crc32;
use crate::error::{Error, Result};

const OP_PUT: u8 = 1;
const OP_DELETE: u8 = 2;
const RECORD_HEADER_LEN: usize = 8;
const FILE_HEADER_LEN: usize = 32;
const LSN_PREFIX: usize = 16;
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"FERWAL\0\0";
const VERSION: u32 = 1;

const WAL_FILE: &str = "wal.log";

#[derive(Debug)]
pub enum Record {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

enum ReadOutcome {
    Full,
    Eof,
    Torn,
}

#[derive(Debug)]
pub struct Wal {
    file: File,
    last_lsn: u64,
    next_lsn: u64,
}

impl Wal {
    pub fn open(dir: &Path) -> Result<(Self, Vec<Record>)> {
        let path = dir.join(WAL_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        init_superblock(&mut file, dir)?;
        let (records, last_lsn) = replay(&mut file)?;
        let next_lsn = last_lsn
            .checked_add(1)
            .ok_or(Error::InvalidArgument("lsn space exhausted"))?;
        Ok((
            Self {
                file,
                last_lsn,
                next_lsn,
            },
            records,
        ))
    }

    pub(crate) fn last_lsn(&self) -> u64 {
        self.last_lsn
    }

    pub fn append_put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.append_op(OP_PUT, key, Some(value))
    }

    pub fn append_delete(&mut self, key: &[u8]) -> Result<()> {
        self.append_op(OP_DELETE, key, None)
    }

    fn append_op(&mut self, op: u8, key: &[u8], value: Option<&[u8]>) -> Result<()> {
        let lsn = self.next_lsn;
        let bytes = encode(lsn, self.last_lsn, op, key, value)?;
        self.append(&bytes)?;
        self.last_lsn = lsn;
        self.next_lsn = lsn
            .checked_add(1)
            .ok_or(Error::InvalidArgument("lsn space exhausted"))?;
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        // each mutation is durable on return; group commit can wait
        self.file.sync_all()?;
        Ok(())
    }
}

fn init_superblock(file: &mut File, dir: &Path) -> Result<()> {
    let len = file.metadata()?.len();
    if len < FILE_HEADER_LEN as u64 {
        file.set_len(0)?;
        write_superblock(file)?;
        file.sync_all()?;
        // POSIX: fsync of the file does not persist the directory entry
        sync_dir(dir)?;
        return Ok(());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; FILE_HEADER_LEN];
    file.read_exact(&mut header)?;
    if &header[0..8] != MAGIC {
        return Err(Error::Corrupt {
            offset: 0,
            reason: "bad wal magic",
        });
    }
    let version = u32_from_prefix(&header[8..12]);
    if version != VERSION {
        return Err(Error::Corrupt {
            offset: 0,
            reason: "unsupported wal version",
        });
    }
    Ok(())
}

fn write_superblock(file: &mut File) -> Result<()> {
    let mut header = [0u8; FILE_HEADER_LEN];
    header[0..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&VERSION.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)?;
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn encode(lsn: u64, prev_lsn: u64, op: u8, key: &[u8], value: Option<&[u8]>) -> Result<Vec<u8>> {
    let key_len = u32_len(key.len())?;
    let mut inner = Vec::new();
    inner.extend_from_slice(&lsn.to_le_bytes());
    inner.extend_from_slice(&prev_lsn.to_le_bytes());
    inner.push(op);
    inner.extend_from_slice(&key_len.to_le_bytes());
    inner.extend_from_slice(key);
    if let Some(val) = value {
        let value_len = u32_len(val.len())?;
        inner.extend_from_slice(&value_len.to_le_bytes());
        inner.extend_from_slice(val);
    }
    if inner.len() > MAX_PAYLOAD as usize {
        return Err(Error::InvalidArgument("record exceeds max payload"));
    }
    let crc = crc32(&inner);
    let mut rec = Vec::with_capacity(RECORD_HEADER_LEN + inner.len());
    rec.extend_from_slice(&crc.to_le_bytes());
    rec.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    rec.extend_from_slice(&inner);
    Ok(rec)
}

fn u32_len(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::InvalidArgument("key or value longer than u32"))
}

fn replay(file: &mut File) -> Result<(Vec<Record>, u64)> {
    file.seek(SeekFrom::Start(FILE_HEADER_LEN as u64))?;
    let mut records = Vec::new();
    let mut last_lsn = 0u64;
    loop {
        let pos = file.stream_position()?;
        let mut header = [0u8; RECORD_HEADER_LEN];
        match read_exact_or_torn(file, &mut header)? {
            ReadOutcome::Eof => break,
            ReadOutcome::Torn => {
                truncate_torn(file, pos)?;
                break;
            }
            ReadOutcome::Full => {}
        }
        let crc = u32_from_prefix(&header[0..4]);
        let len = u32_from_prefix(&header[4..8]);
        let remaining = file.metadata()?.len().saturating_sub(pos);
        let claimed = RECORD_HEADER_LEN as u64 + u64::from(len);
        if claimed > remaining {
            truncate_torn(file, pos)?;
            break;
        }
        if len > MAX_PAYLOAD {
            return Err(Error::Corrupt {
                offset: pos,
                reason: "payload too large",
            });
        }
        let mut inner = vec![0u8; len as usize];
        match read_exact_or_torn(file, &mut inner)? {
            ReadOutcome::Eof | ReadOutcome::Torn => {
                truncate_torn(file, pos)?;
                break;
            }
            ReadOutcome::Full => {}
        }
        if crc32(&inner) != crc {
            let end = file.stream_position()?;
            let file_len = file.metadata()?.len();
            // checksum fail at EOF is a torn write, not a corrupt log
            if end == file_len {
                truncate_torn(file, pos)?;
                break;
            }
            return Err(Error::Corrupt {
                offset: pos,
                reason: "checksum mismatch",
            });
        }
        let (lsn, prev_lsn, rec) = decode(&inner).ok_or(Error::Corrupt {
            offset: pos,
            reason: "malformed payload",
        })?;
        let expected_prev = last_lsn;
        let expected_lsn = last_lsn.saturating_add(1);
        if lsn != expected_lsn || prev_lsn != expected_prev {
            return Err(Error::Corrupt {
                offset: pos,
                reason: "lsn chain broken",
            });
        }
        last_lsn = lsn;
        records.push(rec);
    }
    file.seek(SeekFrom::End(0))?;
    Ok((records, last_lsn))
}

fn truncate_torn(file: &mut File, pos: u64) -> Result<()> {
    file.set_len(pos)?;
    // durable size before later appends, or a crash can restore the old tail
    file.sync_all()?;
    Ok(())
}

fn read_exact_or_torn(file: &mut File, buf: &mut [u8]) -> Result<ReadOutcome> {
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(ReadOutcome::Eof),
            0 => return Ok(ReadOutcome::Torn),
            n => got += n,
        }
    }
    Ok(ReadOutcome::Full)
}

fn u32_from_prefix(slice: &[u8]) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(slice);
    u32::from_le_bytes(bytes)
}

fn u64_from_prefix(slice: &[u8]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(slice);
    u64::from_le_bytes(bytes)
}

fn decode(inner: &[u8]) -> Option<(u64, u64, Record)> {
    if inner.len() < LSN_PREFIX {
        return None;
    }
    let lsn = u64_from_prefix(&inner[0..8]);
    let prev_lsn = u64_from_prefix(&inner[8..16]);
    if lsn == 0 {
        return None;
    }
    let rec = decode_body(&inner[LSN_PREFIX..])?;
    Some((lsn, prev_lsn, rec))
}

fn decode_body(payload: &[u8]) -> Option<Record> {
    let mut i = 0usize;
    let op = *payload.get(i)?;
    i += 1;
    let key_len = take_u32(payload, &mut i)? as usize;
    let key = payload.get(i..i.checked_add(key_len)?)?.to_vec();
    i += key_len;
    match op {
        OP_PUT => {
            let value_len = take_u32(payload, &mut i)? as usize;
            let value = payload.get(i..i.checked_add(value_len)?)?.to_vec();
            i += value_len;
            if i != payload.len() {
                return None;
            }
            Some(Record::Put { key, value })
        }
        OP_DELETE => {
            if i != payload.len() {
                return None;
            }
            Some(Record::Delete { key })
        }
        _ => None,
    }
}

fn take_u32(buf: &[u8], i: &mut usize) -> Option<u32> {
    let end = i.checked_add(4)?;
    let slice = buf.get(*i..end)?;
    *i = end;
    Some(u32_from_prefix(slice))
}
