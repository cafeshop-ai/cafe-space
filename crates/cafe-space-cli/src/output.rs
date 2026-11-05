//! Lossless JSON for consoles whose shell decodes redirected bytes using a legacy code page.
use cafe_space_protocol::RequestResult;
use std::{fmt::Write, io};

pub(super) fn json(value: &RequestResult, ascii: bool) -> io::Result<String> {
    let encoded = if ascii {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
    .map_err(|_| io::Error::other("CLI_OUTPUT_INVALID"))?;
    if !ascii {
        return Ok(encoded);
    }
    let mut output = String::with_capacity(encoded.len());
    for ch in encoded.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                write!(&mut output, "\\u{unit:04x}")
                    .map_err(|_| io::Error::other("CLI_OUTPUT_INVALID"))?;
            }
        }
    }
    Ok(output)
}
