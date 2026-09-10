use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::crc::crc32;
use crate::error::{Error, Result};

const OP_PUT: u8 = 1;
const OP_DELETE: u8 = 2;
const HEADER_LEN: usize = 8;
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

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
        let records = replay(&mut file)?;
        Ok((Self { file }, records))
    }

    pub fn append_put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.append(&encode(OP_PUT, key, Some(value))?)
    }

    pub fn append_delete(&mut self, key: &[u8]) -> Result<()> {
        self.append(&encode(OP_DELETE, key, None)?)
    }

    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        // each mutation is durable on return; group commit can wait
        self.file.sync_all()?;
        Ok(())
    }
}

fn encode(op: u8, key: &[u8], value: Option<&[u8]>) -> Result<Vec<u8>> {
    let key_len = u32_len(key.len())?;
    let mut payload = Vec::new();
    payload.push(op);
    payload.extend_from_slice(&key_len.to_le_bytes());
    payload.extend_from_slice(key);
    if let Some(val) = value {
        let value_len = u32_len(val.len())?;
        payload.extend_from_slice(&value_len.to_le_bytes());
        payload.extend_from_slice(val);
    }
    if payload.len() > MAX_PAYLOAD as usize {
        return Err(Error::InvalidArgument("record exceeds max payload"));
    }
    let crc = crc32(&payload);
    let mut rec = Vec::with_capacity(HEADER_LEN + payload.len());
    rec.extend_from_slice(&crc.to_le_bytes());
    rec.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    rec.extend_from_slice(&payload);
    Ok(rec)
}

fn u32_len(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::InvalidArgument("key or value longer than u32"))
}

fn replay(file: &mut File) -> Result<Vec<Record>> {
    file.seek(SeekFrom::Start(0))?;
    let mut records = Vec::new();
    loop {
        let pos = file.stream_position()?;
        let mut header = [0u8; HEADER_LEN];
        match read_exact_or_torn(file, &mut header)? {
            ReadOutcome::Eof => break,
            ReadOutcome::Torn => {
                file.set_len(pos)?;
                break;
            }
            ReadOutcome::Full => {}
        }
        let crc = u32_from_prefix(&header[0..4]);
        let len = u32_from_prefix(&header[4..8]);
        if len > MAX_PAYLOAD {
            return Err(Error::Corrupt {
                offset: pos,
                reason: "payload too large",
            });
        }
        let mut payload = vec![0u8; len as usize];
        match read_exact_or_torn(file, &mut payload)? {
            ReadOutcome::Eof | ReadOutcome::Torn => {
                file.set_len(pos)?;
                break;
            }
            ReadOutcome::Full => {}
        }
        if crc32(&payload) != crc {
            let end = file.stream_position()?;
            let file_len = file.metadata()?.len();
            // checksum fail at EOF is a torn write, not a corrupt log
            if end == file_len {
                file.set_len(pos)?;
                break;
            }
            return Err(Error::Corrupt {
                offset: pos,
                reason: "checksum mismatch",
            });
        }
        let rec = decode(&payload).ok_or(Error::Corrupt {
            offset: pos,
            reason: "malformed payload",
        })?;
        records.push(rec);
    }
    file.seek(SeekFrom::End(0))?;
    Ok(records)
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

fn decode(payload: &[u8]) -> Option<Record> {
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
