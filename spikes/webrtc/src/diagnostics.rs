//! Only numeric counters from the pinned SCTP implementation; never forward library logs.
use log::{LevelFilter, Log, Metadata, Record};
use serde::Serialize;
use std::sync::Mutex;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Counters {
    data_chunks_received: u64,
    sacks_built: u64,
    sacks_received: u64,
    t3_timeouts: u64,
    fast_retransmits: u64,
    receive_buffer_drops: u64,
    last_data_tsn: Option<u32>,
    last_peer_tsn: Option<u32>,
    last_sack_built_tsn: Option<u32>,
    last_local_window: Option<u32>,
    last_sack_received_tsn: Option<u32>,
    last_remote_window: Option<u32>,
}

fn number(text: &str, key: &str) -> Option<u32> {
    text.split_once(key)?
        .1
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

impl Counters {
    fn record(&mut self, text: &str) {
        if text.contains(" DATA: tsn=") {
            self.data_chunks_received += 1;
            self.last_data_tsn = number(text, "DATA: tsn=");
            self.last_peer_tsn = number(text, "peer_last_tsn=");
        } else if text.contains(" sending SACK: SACK cumTsnAck=") {
            self.sacks_built += 1;
            self.last_sack_built_tsn = number(text, "cumTsnAck=");
            self.last_local_window = number(text, "arwnd=");
        } else if text.contains(", SACK: cumTSN=") {
            self.sacks_received += 1;
            self.last_sack_received_tsn = number(text, "cumTSN=");
            self.last_remote_window = number(text, "a_rwnd=");
        } else if text.contains(" T3-rtx timed out:") {
            self.t3_timeouts += 1;
        } else if text.contains(" fast-retransmit: tsn=") {
            self.fast_retransmits += 1;
        } else if text.contains(" receive buffer full. dropping DATA") {
            self.receive_buffer_drops += 1;
        }
    }
}

struct SctpLogger;
static LOGGER: SctpLogger = SctpLogger;
static COUNTERS: Mutex<Option<Counters>> = Mutex::new(None);

impl Log for SctpLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "rtc_sctp::association"
    }
    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata())
            && let Ok(mut counters) = COUNTERS.lock()
            && let Some(counters) = counters.as_mut()
        {
            counters.record(&record.args().to_string());
        }
    }
    fn flush(&self) {}
}

pub fn install() -> Result<(), log::SetLoggerError> {
    log::set_logger(&LOGGER)?;
    log::set_max_level(LevelFilter::Trace);
    Ok(())
}

pub fn reset() {
    if let Ok(mut counters) = COUNTERS.lock() {
        *counters = Some(Counters::default());
    }
}

pub fn snapshot() -> serde_json::Value {
    match COUNTERS.lock() {
        Ok(counters) => serde_json::to_value(counters.as_ref()).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whitelisted_numeric_sctp_fields_survive() {
        let mut counters = Counters::default();
        for message in [
            "[Server] DATA: tsn=400 peer_last_tsn=399 immediateSack=false len=1200, unordered=false",
            "[Server] sending SACK: SACK cumTsnAck=400 arwnd=524288 dupTsn=[]",
            "[Server] Established, SACK: cumTSN=100 a_rwnd=65536",
            "[Server] T3-rtx timed out: n_rtos=1 cwnd=1200 ssthresh=4800",
            "[Server] fast-retransmit: tsn=101 sent=2 htna=105",
            "[Server] receive buffer full. dropping DATA with tsn=401 ssn=2",
            "PRIVATE_SECRET 192.0.2.1 session=PRIVATE_ID",
        ] {
            counters.record(message);
        }
        let value = serde_json::to_value(counters).unwrap();
        assert_eq!(value["dataChunksReceived"], 1);
        assert_eq!(value["sacksBuilt"], 1);
        assert_eq!(value["sacksReceived"], 1);
        assert_eq!(value["t3Timeouts"], 1);
        assert_eq!(value["fastRetransmits"], 1);
        assert_eq!(value["receiveBufferDrops"], 1);
        assert_eq!(value["lastDataTsn"], 400);
        assert_eq!(value["lastSackBuiltTsn"], 400);
        assert_eq!(value["lastLocalWindow"], 524288);
        assert_eq!(value["lastRemoteWindow"], 65536);
        assert!(!value.to_string().contains("PRIVATE_"));
        assert!(!value.to_string().contains("192.0.2.1"));
        assert_eq!(number("arwnd=PRIVATE_SECRET", "arwnd="), None);
        assert_eq!(number("arwnd=4294967296", "arwnd="), None);
        assert!(
            !LOGGER.enabled(
                &Metadata::builder()
                    .target("rtc_ice::agent")
                    .level(log::Level::Trace)
                    .build()
            )
        );
    }
}
