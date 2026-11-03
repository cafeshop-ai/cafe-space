//! Socket/pipe framing only; callers pass payloads through the strict protocol codec.

use std::{io, time::Duration};

use cafe_space_protocol::MAX_FRAME_BYTES;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const FRAME_DEADLINE: Duration = Duration::from_secs(30);

/// `None` means clean EOF between frames. Once a frame starts, it must finish in 30s.
/// Not cancellation-safe: after an error or cancellation the connection must be closed.
pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 4];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    tokio::time::timeout(FRAME_DEADLINE, async {
        reader.read_exact(&mut header[1..]).await?;
        let size = u32::from_be_bytes(header) as usize;
        if size == 0 || size > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid IPC frame length",
            ));
        }
        let mut frame = vec![0; size];
        reader.read_exact(&mut frame).await?;
        Ok(Some(frame))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IPC frame deadline exceeded"))?
}

/// Only one writer may own a stream. Failed/cancelled writes require closing it,
/// have an unknown outcome, and must never cause replay of a command.
pub async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), frame: &[u8]) -> io::Result<()> {
    if frame.is_empty() || frame.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid IPC frame length",
        ));
    }
    tokio::time::timeout(FRAME_DEADLINE, async {
        writer
            .write_all(&(frame.len() as u32).to_be_bytes())
            .await?;
        writer.write_all(frame).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "IPC write deadline exceeded"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_protocol::{decode_client_message, encode_client_message};

    #[tokio::test]
    async fn frames_survive_fragmentation_and_back_to_back_writes() {
        let hello = include_bytes!("../../../fixtures/v2/client-hello.json");
        let request = include_bytes!("../../../fixtures/v2/client-request.json");
        let (mut sender, mut receiver) = tokio::io::duplex(1);
        let sending = async {
            write_frame(&mut sender, hello).await.unwrap();
            write_frame(&mut sender, request).await.unwrap();
            sender.shutdown().await.unwrap();
        };
        let receiving = async {
            for expected in [hello.as_slice(), request.as_slice()] {
                let frame = read_frame(&mut receiver).await.unwrap().unwrap();
                assert_eq!(frame, expected);
                let message = decode_client_message(&frame).unwrap();
                assert_eq!(
                    decode_client_message(&encode_client_message(&message).unwrap()).unwrap(),
                    message
                );
            }
            assert!(read_frame(&mut receiver).await.unwrap().is_none());
        };
        tokio::join!(sending, receiving);
    }

    #[tokio::test]
    async fn prefix_counts_utf8_bytes_not_characters() {
        let mut message =
            decode_client_message(include_bytes!("../../../fixtures/v2/client-hello.json"))
                .unwrap();
        let cafe_space_protocol::ClientMessage::Hello(hello) = &mut message else {
            panic!("hello expected")
        };
        hello.client_version = "中文 🧪 e\u{301} مرحبا".into();
        let frame = encode_client_message(&message).unwrap();
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).await.unwrap();
        assert_eq!(
            u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize,
            frame.len()
        );
        assert!(frame.len() > std::str::from_utf8(&frame).unwrap().chars().count());
        let received = read_frame(&mut wire.as_slice()).await.unwrap().unwrap();
        assert_eq!(decode_client_message(&received).unwrap(), message);
    }

    #[tokio::test]
    async fn maximum_frame_round_trips() {
        let frame = vec![b'x'; MAX_FRAME_BYTES];
        let (mut sender, mut receiver) = tokio::io::duplex(1024);
        let sending = write_frame(&mut sender, &frame);
        let receiving = read_frame(&mut receiver);
        let (sent, received) = tokio::join!(sending, receiving);
        sent.unwrap();
        assert_eq!(received.unwrap().unwrap(), frame);
    }

    #[tokio::test]
    async fn rejects_invalid_lengths_before_reading_any_body() {
        for size in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
            let header = size.to_be_bytes();
            let mut input = header.as_slice();
            assert_eq!(
                read_frame(&mut input).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[tokio::test]
    async fn distinguishes_clean_eof_from_truncated_header_or_body() {
        assert!(read_frame(&mut &b""[..]).await.unwrap().is_none());
        for input in [&b"\0"[..], &b"\0\0\0"[..], &b"\0\0\0\x02x"[..]] {
            assert_eq!(
                read_frame(&mut &input[..]).await.unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[tokio::test]
    async fn invalid_outgoing_lengths_write_nothing() {
        for frame in [vec![], vec![0; MAX_FRAME_BYTES + 1]] {
            let mut output = Vec::new();
            assert_eq!(
                write_frame(&mut output, &frame).await.unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(output.is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn partial_frames_and_blocked_writes_time_out() {
        for prefix in [&b"\0"[..], &b"\0\0\0\x02x"[..]] {
            let (mut sender, mut receiver) = tokio::io::duplex(8);
            sender.write_all(prefix).await.unwrap();
            assert_eq!(
                read_frame(&mut receiver).await.unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
        }
        let (mut sender, _receiver) = tokio::io::duplex(1);
        assert_eq!(
            write_frame(&mut sender, b"{}").await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[tokio::test]
    async fn broken_writes_surface_without_retrying() {
        let (mut sender, receiver) = tokio::io::duplex(1);
        drop(receiver);
        assert_eq!(
            write_frame(&mut sender, b"{}").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
