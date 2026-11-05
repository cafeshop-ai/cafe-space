//! BOM-directed input only. Never guess ANSI or replace malformed Unicode.
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, IsTerminal, Read},
};

const MAX_INPUT_BYTES: u64 = 2 * 65536 + 2;
fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}

pub(super) fn decode(bytes: &[u8]) -> io::Result<String> {
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(error("CLI_INPUT_TOO_LARGE"));
    }
    if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
        return Err(error("CLI_INPUT_UTF32_UNSUPPORTED"));
    }
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(error("CLI_INPUT_INVALID_UTF16"));
        }
        let little = bytes[0] == 0xff;
        let units: Vec<_> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| error("CLI_INPUT_INVALID_UTF16"))?
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        std::str::from_utf8(bytes)
            .map_err(|_| error("CLI_INPUT_INVALID_UTF8"))?
            .to_owned()
    };
    if text.len() > 65536 {
        return Err(error("CLI_INPUT_TOO_LARGE"));
    }
    Ok(text)
}

fn bounded(reader: impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("CLI_INPUT_READ_FAILED"))?;
    decode(&bytes)
}

pub(super) fn file(path: &OsStr) -> io::Result<String> {
    if path == "-" {
        if io::stdin().is_terminal() {
            return Err(error("CLI_STDIN_REQUIRES_PIPE"));
        }
        return bounded(io::stdin().lock());
    }
    if !std::fs::metadata(path)
        .map_err(|_| error("CLI_INPUT_READ_FAILED"))?
        .is_file()
    {
        return Err(error("CLI_INPUT_NOT_FILE"));
    }
    let file = File::open(path).map_err(|_| error("CLI_INPUT_READ_FAILED"))?;
    if !file
        .metadata()
        .map_err(|_| error("CLI_INPUT_READ_FAILED"))?
        .is_file()
    {
        return Err(error("CLI_INPUT_NOT_FILE"));
    }
    bounded(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bom_decoding_preserves_unicode_newlines_and_rejects_malformed_input() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/cli/session.json")).unwrap();
        let text = fixture["text"].as_str().unwrap();
        assert_eq!(fixture["limits"]["inputBytes"], MAX_INPUT_BYTES);
        let utf8 = text.as_bytes().to_vec();
        let mut with_bom = vec![0xef, 0xbb, 0xbf];
        with_bom.extend_from_slice(&utf8);
        let mut le = vec![0xff, 0xfe];
        let mut be = vec![0xfe, 0xff];
        for unit in text.encode_utf16() {
            le.extend(unit.to_le_bytes());
            be.extend(unit.to_be_bytes());
        }
        for bytes in [utf8, with_bom, le, be] {
            assert_eq!(decode(&bytes).unwrap(), text);
        }
        for (bytes, code) in [
            (vec![0xb2, 0xe8], "CLI_INPUT_INVALID_UTF8"),
            (vec![0xff, 0xfe, 0x00], "CLI_INPUT_INVALID_UTF16"),
            (vec![0xff, 0xfe, 0x00, 0xd8], "CLI_INPUT_INVALID_UTF16"),
            (vec![0xfe, 0xff, 0xdc, 0x00], "CLI_INPUT_INVALID_UTF16"),
            (vec![0xff, 0xfe, 0x00, 0x00], "CLI_INPUT_UTF32_UNSUPPORTED"),
            (
                vec![b'x'; MAX_INPUT_BYTES as usize + 1],
                "CLI_INPUT_TOO_LARGE",
            ),
            (vec![b'x'; 65537], "CLI_INPUT_TOO_LARGE"),
        ] {
            assert_eq!(decode(&bytes).unwrap_err().to_string(), code);
        }
        assert_eq!(decode(&[b'x'; 65536]).unwrap().len(), 65536);
        assert_eq!(decode(&[0xef, 0xbb, 0xbf]).unwrap(), "");
    }
}
