use super::{Error, QUEUE, Result};
use agent_client_protocol::{Channel, TransportFrame};
use futures::StreamExt;
use std::io;
use tokio::sync::mpsc::{Receiver, Sender};

pub const MAX_RECORD_BYTES: usize = 1_048_576;

/// The owned-process reader must bound LF records before allocating these strings.
/// This bridge also bounds the SDK's otherwise unbounded incoming channel.
pub(super) async fn bridge(
    mut channel: Channel,
    mut input: Receiver<io::Result<String>>,
    output: Sender<String>,
) -> Result<()> {
    loop {
        tokio::select! {
            biased;
            frame = channel.rx.next() => {
                let frame = frame.ok_or(Error("ACP_CONNECTION_CLOSED"))?;
                let line = frame.to_json().map_err(|_| Error("ACP_RECORD_INVALID"))?;
                if line.len() > MAX_RECORD_BYTES {
                    return Err(Error("ACP_RECORD_TOO_LARGE"));
                }
                // Do not feed more input while a blocked writer accumulates SDK replies.
                tokio::time::timeout(super::DEADLINE, output.send(line)).await
                    .map_err(|_| Error("ACP_WRITE_OUTCOME_UNKNOWN"))?
                    .map_err(|_| Error("ACP_WRITE_OUTCOME_UNKNOWN"))?;
            }
            line = input.recv(), if channel.tx.len() < QUEUE => {
                let line = line.ok_or(Error("ACP_CONNECTION_CLOSED"))?
                    .map_err(|_| Error("ACP_READ_FAILED"))?;
                if line.len() > MAX_RECORD_BYTES {
                    return Err(Error("ACP_RECORD_TOO_LARGE"));
                }
                match TransportFrame::parse_json(&line) {
                    frame @ TransportFrame::Single(_) => channel.tx.unbounded_send(frame)
                        .map_err(|_| Error("ACP_CONNECTION_CLOSED"))?,
                    _ => return Err(Error("ACP_RECORD_INVALID")),
                }
            }
            // The upstream channel exposes occupancy but has no capacity wake-up.
            _ = tokio::time::sleep(std::time::Duration::from_millis(5)), if channel.tx.len() >= QUEUE => {}
        }
    }
}
