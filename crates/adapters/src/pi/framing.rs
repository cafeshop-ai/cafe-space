//! Strict LF JSON objects, bounded before allocation; no Unicode line splitting.
use super::{Error, Result};
use serde_json::Value;
use std::io::{BufRead, Write};

pub const MAX_RECORD_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Json(Value),
    Omitted { bytes: u64 },
}
impl From<Value> for Record {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

pub struct Records<R> {
    input: R,
    failed: bool,
}
impl<R: BufRead> Records<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            failed: false,
        }
    }
    pub fn next_record(&mut self) -> Result<Option<Record>> {
        if self.failed {
            return Err(Error("PI_CONNECTION_CLOSED"));
        }
        let result = self.read_record();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn read_record(&mut self) -> Result<Option<Record>> {
        let mut record = Vec::new();
        let mut total = 0u64;
        let mut omitted = false;
        loop {
            let bytes = self.input.fill_buf().map_err(|_| Error("PI_READ_FAILED"))?;
            if bytes.is_empty() {
                return if total == 0 {
                    Ok(None)
                } else {
                    Err(Error("PI_RECORD_INCOMPLETE"))
                };
            }
            let end = bytes.iter().position(|byte| *byte == b'\n');
            let count = end.unwrap_or(bytes.len());
            total = total.saturating_add(count as u64);
            if total > MAX_RECORD_BYTES as u64 {
                omitted = true;
                record.clear();
            }
            if !omitted {
                record.extend_from_slice(&bytes[..count]);
            }
            self.input.consume(count + usize::from(end.is_some()));
            if end.is_some() {
                if omitted {
                    return Ok(Some(Record::Omitted { bytes: total }));
                }
                if record.last() == Some(&b'\r') {
                    record.pop();
                }
                let value: Value =
                    serde_json::from_slice(&record).map_err(|_| Error("PI_RECORD_INVALID"))?;
                if !value.is_object() {
                    return Err(Error("PI_RECORD_INVALID"));
                }
                return Ok(Some(Record::Json(value)));
            }
        }
    }
}

pub fn encode(value: &Value) -> Result<Vec<u8>> {
    if !value.is_object() {
        return Err(Error("PI_COMMAND_INVALID"));
    }
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error("PI_COMMAND_INVALID"))?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error("PI_RECORD_TOO_LARGE"));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Call outside Host's admission mutex, on a dedicated bounded writer. Never retry after a partial write.
pub fn write_record(output: &mut impl Write, bytes: &[u8]) -> Result<()> {
    output
        .write_all(bytes)
        .and_then(|()| output.flush())
        .map_err(|_| Error("PI_WRITE_OUTCOME_UNKNOWN"))
}
