use super::{Error, MAX_RECORD_BYTES, Result};
use std::io::{BufRead, Read, Write};

/// Strict LF records with a hard allocation bound, including on a stream with no delimiter.
/// A framing error is terminal; the caller must stop the owned process, not skip and retry.
pub fn read_line(input: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    input
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Error("ACP_READ_FAILED"))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(Error(if bytes.len() > MAX_RECORD_BYTES {
            "ACP_RECORD_TOO_LARGE"
        } else {
            "ACP_RECORD_INCOMPLETE"
        }));
    }
    bytes.pop();
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Error("ACP_RECORD_INVALID"))
}
pub fn write_line(output: &mut impl Write, line: &str) -> Result<()> {
    if line.len() > MAX_RECORD_BYTES || line.as_bytes().contains(&b'\n') {
        return Err(Error("ACP_RECORD_INVALID"));
    }
    output
        .write_all(line.as_bytes())
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(|_| Error("ACP_WRITE_OUTCOME_UNKNOWN"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, BufReader, Cursor};

    #[test]
    fn lf_only_utf8_crlf_and_bounded_unterminated_records() {
        let text = "{\"text\":\"茶\u{2028}\u{2029}\"}";
        let bytes = format!("{text}\r\n{text}\n");
        let mut reader = BufReader::with_capacity(1, Cursor::new(bytes));
        assert_eq!(read_line(&mut reader).unwrap().as_deref(), Some(text));
        assert_eq!(read_line(&mut reader).unwrap().as_deref(), Some(text));
        assert!(read_line(&mut reader).unwrap().is_none());
        assert_eq!(
            read_line(&mut Cursor::new(b"{}")),
            Err(Error("ACP_RECORD_INCOMPLETE"))
        );
        assert_eq!(
            read_line(&mut Cursor::new(b"\xff\n")),
            Err(Error("ACP_RECORD_INVALID"))
        );
        let mut reader = Cursor::new(vec![b'x'; MAX_RECORD_BYTES * 2]);
        assert_eq!(read_line(&mut reader), Err(Error("ACP_RECORD_TOO_LARGE")));
        assert_eq!(reader.position(), (MAX_RECORD_BYTES + 1) as u64);
        let mut maximum = vec![b'x'; MAX_RECORD_BYTES];
        maximum.push(b'\n');
        assert_eq!(
            read_line(&mut Cursor::new(maximum)).unwrap().unwrap().len(),
            MAX_RECORD_BYTES
        );
    }
    #[test]
    fn partial_write_is_unknown_and_never_retried() {
        struct Broken {
            calls: usize,
        }
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                self.calls += 1;
                if self.calls == 1 {
                    Ok(1)
                } else {
                    Err(io::Error::other("private native error"))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("must not flush a broken write")
            }
        }
        let mut broken = Broken { calls: 0 };
        assert_eq!(
            write_line(&mut broken, "{}"),
            Err(Error("ACP_WRITE_OUTCOME_UNKNOWN"))
        );
        assert_eq!(broken.calls, 2);
        let mut output = Vec::new();
        write_line(&mut output, "{}").unwrap();
        assert_eq!(output, b"{}\n");
        assert_eq!(
            write_line(&mut output, "{}\n{}"),
            Err(Error("ACP_RECORD_INVALID"))
        );
    }
}
