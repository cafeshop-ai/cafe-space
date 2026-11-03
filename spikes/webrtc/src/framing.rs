//! v0 PCD1 framing, pinned to pi-packages 3294487 relay/internal/remote/rtc.go.
use std::time::{Duration, Instant};
pub const PACKET_BYTES: usize = 16 * 1024;
pub const HEADER_BYTES: usize = 16;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
const MAGIC: u32 = 0x50434431;

pub fn packet(id: u32, raw: &[u8], offset: usize) -> Result<Vec<u8>, &'static str> {
    if id == 0 || raw.is_empty() || raw.len() > MAX_FRAME_BYTES || offset >= raw.len() {
        return Err("FRAME_BOUNDS");
    }
    let end = (offset + PACKET_BYTES - HEADER_BYTES).min(raw.len());
    let mut bytes = Vec::with_capacity(HEADER_BYTES + end - offset);
    for value in [MAGIC, id, raw.len() as u32, offset as u32] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&raw[offset..end]);
    Ok(bytes)
}

#[derive(Default)]
pub struct Decoder {
    last: u32,
    id: u32,
    total: usize,
    data: Vec<u8>,
    deadline: Option<Instant>,
}
impl Decoder {
    pub fn push(&mut self, packet: &[u8], now: Instant) -> Result<Option<Vec<u8>>, &'static str> {
        if packet.len() <= HEADER_BYTES || packet.len() > PACKET_BYTES {
            return Err("PACKET_SIZE");
        }
        let word = |at| u32::from_be_bytes(packet[at..at + 4].try_into().expect("checked header"));
        let (id, total, offset) = (word(4), word(8) as usize, word(12) as usize);
        let body = &packet[HEADER_BYTES..];
        if word(0) != MAGIC
            || id == 0
            || total == 0
            || total > MAX_FRAME_BYTES
            || offset + body.len() > total
        {
            return Err("PACKET_BOUNDS");
        }
        if offset == 0 {
            if self.deadline.is_some() || id != self.last.wrapping_add(1).max(1) {
                return Err("PACKET_REPLAY_OR_OVERLAP");
            }
            self.id = id;
            self.total = total;
            self.data = Vec::with_capacity(total);
            self.deadline = Some(now + Duration::from_secs(10));
        }
        if self.deadline.is_none_or(|deadline| now > deadline)
            || self.id != id
            || self.total != total
            || self.data.len() != offset
        {
            return Err("PACKET_SEQUENCE_OR_TIMEOUT");
        }
        self.data.extend_from_slice(body);
        if self.data.len() != total {
            return Ok(None);
        }
        self.deadline = None;
        self.last = id;
        Ok(Some(std::mem::take(&mut self.data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn golden_header_maximum_frame_and_replay_match_v0() {
        assert_eq!(
            packet(1, b"ab", 0).unwrap(),
            [
                b"PCD1".as_slice(),
                &[0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0],
                b"ab"
            ]
            .concat()
        );
        let raw: Vec<_> = (0..MAX_FRAME_BYTES).map(|n| (n % 251) as u8).collect();
        let mut decoder = Decoder::default();
        let now = Instant::now();
        let mut received = None;
        for offset in (0..raw.len()).step_by(PACKET_BYTES - HEADER_BYTES) {
            received = decoder
                .push(&packet(1, &raw, offset).unwrap(), now)
                .unwrap();
        }
        assert_eq!(received.unwrap(), raw);
        assert!(decoder.push(&packet(1, b"a", 0).unwrap(), now).is_err());
        decoder.last = u32::MAX;
        assert_eq!(
            decoder.push(&packet(1, b"a", 0).unwrap(), now).unwrap(),
            Some(b"a".to_vec())
        );
    }
    #[test]
    fn bounds_overlap_sequence_and_deadline_fail_closed() {
        let now = Instant::now();
        assert!(packet(0, b"a", 0).is_err());
        assert!(packet(1, &vec![0; MAX_FRAME_BYTES + 1], 0).is_err());
        for bytes in [vec![], vec![0; 16], vec![0; PACKET_BYTES + 1]] {
            assert!(Decoder::default().push(&bytes, now).is_err());
        }
        let raw = vec![1; PACKET_BYTES];
        let first = packet(1, &raw, 0).unwrap();
        let second = packet(1, &raw, PACKET_BYTES - HEADER_BYTES).unwrap();
        assert!(Decoder::default().push(&second, now).is_err());
        let mut d = Decoder::default();
        d.push(&first, now).unwrap();
        assert!(d.push(&first, now).is_err());
        assert!(d.push(&second, now + Duration::from_secs(11)).is_err());
        let mut invalid = first;
        invalid[8..12].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        assert!(Decoder::default().push(&invalid, now).is_err());
    }
}
