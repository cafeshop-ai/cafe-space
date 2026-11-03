//! Anonymous R4 research daemon in loopback or fresh CI namespaces; never opens Café state.
mod framing;
use bytes::BytesMut;
use framing::{Decoder, HEADER_BYTES, PACKET_BYTES, packet};
use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    peer_connection::configuration::setting_engine::SctpMaxMessageSize,
};
use serde::Deserialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::mpsc,
};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy, RTCSessionDescription,
        SettingEngineBuilder,
    },
};
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Direct,
    Relay,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    mode: Mode,
    servers: Vec<RTCIceServer>,
    offer: RTCSessionDescription,
}
struct Handler {
    gathered: mpsc::Sender<()>,
    claimed: AtomicBool,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.try_send(());
        }
    }
    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let reject = self.claimed.swap(true, Ordering::Relaxed);
        // DataChannel calls round-trip to this driver: never await them inside its callback.
        tokio::spawn(async move {
            if reject || dc.label().await.as_deref() != Ok("cafe-r4") {
                let _ = dc.close().await;
                return;
            }
            if echo(&dc).await.is_err() {
                diagnostic("ECHO_FAILED", 0);
                let _ = dc.close().await;
            }
        });
    }
}
fn diagnostic(code: &str, bytes: usize) {
    println!("{{\"event\":\"diagnostic\",\"code\":\"{code}\",\"bytes\":{bytes}}}");
}
async fn echo(dc: &Arc<dyn DataChannel>) -> Result<(), Error> {
    let mut decoder = Decoder::default();
    let mut next = 1u32;
    while let Some(event) = dc.poll().await {
        match event {
            DataChannelEvent::OnMessage(message) => {
                if next <= 2 {
                    diagnostic("PACKET_RX", message.data.len());
                }
                if message.is_string {
                    return Err("BINARY_PACKETS_REQUIRED".into());
                }
                if let Some(raw) = decoder.push(&message.data, Instant::now())? {
                    if next <= 2 {
                        diagnostic("FRAME_RX", raw.len());
                    }
                    for offset in (0..raw.len()).step_by(PACKET_BYTES - HEADER_BYTES) {
                        let bytes = BytesMut::from(packet(next, &raw, offset)?.as_slice());
                        tokio::time::timeout(Duration::from_secs(5), dc.send(bytes)).await??;
                    }
                    if next <= 2 {
                        diagnostic("FRAME_TX", raw.len());
                    }
                    next = next.wrapping_add(1).max(1);
                }
            }
            DataChannelEvent::OnOpen => diagnostic("CHANNEL_OPEN", 0),
            DataChannelEvent::OnClose => {
                diagnostic("CHANNEL_CLOSE", 0);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
fn validate_servers(mode: &Mode, servers: &[RTCIceServer], turn_ip: &str) -> Result<(), Error> {
    match mode {
        Mode::Direct if servers.is_empty() => return Ok(()),
        Mode::Relay if servers.len() == 1 => {}
        _ => return Err("INVALID_LAB_SERVERS".into()),
    }
    let server = &servers[0];
    if server.urls.len() != 1
        || server.username.is_empty()
        || server.username.len() > 128
        || server.credential.is_empty()
        || server.credential.len() > 128
    {
        return Err("INVALID_LAB_SERVERS".into());
    }
    let port = server.urls[0]
        .strip_prefix(&format!("turn:{turn_ip}:"))
        .and_then(|url| url.strip_suffix("?transport=udp"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0);
    if port.is_none() {
        return Err("ONLY_ISOLATED_UDP_TURN_ALLOWED".into());
    }
    Ok(())
}
fn topology() -> Result<(&'static str, &'static str), Error> {
    if std::env::var("CAFE_SPACE_R4_NETNS").as_deref() != Ok("1") {
        return Ok(("127.0.0.1:0", "127.0.0.1"));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::fs::metadata("/proc/self/ns/net")?.ino()
                == std::fs::metadata("/var/run/netns/r4-rust")?.ino()
        {
            return Ok(("10.203.0.2:0", "10.203.0.1"));
        }
    }
    Err("NOT_IN_DISPOSABLE_RUST_NAMESPACE".into())
}
async fn run() -> Result<(), Error> {
    let (bind, turn_ip) = topology()?;
    println!("{{\"event\":\"ready\"}}");
    let mut input = BufReader::new(tokio::io::stdin());
    let mut pc: Option<Arc<dyn PeerConnection>> = None;
    loop {
        let mut line = Vec::new();
        let length = (&mut input)
            .take(65_537)
            .read_until(b'\n', &mut line)
            .await?;
        if length == 0 {
            break;
        }
        if length > 65_536 || !line.ends_with(b"\n") {
            return Err("SIGNAL_BOUNDS".into());
        }
        let message: Input = serde_json::from_slice(&line)?;
        if let Some(previous) = pc.take() {
            tokio::time::timeout(Duration::from_secs(5), previous.close()).await??;
        }
        let Input {
            mode,
            servers,
            offer,
        } = message;
        validate_servers(&mode, &servers, turn_ip)?;
        let (gathered, mut complete) = mpsc::channel(1);
        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(servers)
            .with_ice_transport_policy(match mode {
                Mode::Direct => RTCIceTransportPolicy::All,
                Mode::Relay => RTCIceTransportPolicy::Relay,
            })
            .build();
        let settings = SettingEngineBuilder::new()
            .with_include_loopback_candidate(true)
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_network_types(vec![NetworkType::Udp4])
            .with_sctp_max_message_size(SctpMaxMessageSize::Bounded(PACKET_BYTES as u32))
            .with_sctp_max_receive_buffer_size(512 * 1024)
            .with_ice_timeouts(
                Some(Duration::from_secs(8)),
                Some(Duration::from_secs(15)),
                Some(Duration::from_secs(2)),
            )
            .build();
        let peer = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_setting_engine(settings)
            .with_handler(Arc::new(Handler {
                gathered,
                claimed: AtomicBool::new(false),
            }))
            .with_data_channel_send_buffer_limit(256 * 1024)
            .with_udp_addrs(vec![bind])
            .build()
            .await?;
        let peer: Arc<dyn PeerConnection> = Arc::new(peer);
        peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        peer.set_local_description(answer).await?;
        let gathered = matches!(
            tokio::time::timeout(Duration::from_secs(8), complete.recv()).await,
            Ok(Some(()))
        );
        let answer = peer
            .local_description()
            .await
            .ok_or("NO_LOCAL_DESCRIPTION")?;
        let relay_candidates = answer
            .sdp
            .lines()
            .filter(|line| line.starts_with("a=candidate:") && line.contains(" typ relay"))
            .count();
        println!(
            "{}",
            serde_json::json!({"event":"answer","answer":answer,"gathered":gathered,"relayCandidates":relay_candidates})
        );
        pc = Some(peer);
    }
    if let Some(pc) = pc {
        tokio::time::timeout(Duration::from_secs(5), pc.close()).await??;
    }
    Ok(())
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("R4_SPIKE_FAILED");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lab_never_accepts_production_or_non_udp_turn() {
        let mut server = RTCIceServer {
            urls: vec!["turn:127.0.0.1:3478?transport=udp".into()],
            username: "ephemeral".into(),
            credential: "ephemeral".into(),
        };
        assert!(validate_servers(&Mode::Direct, &[], "127.0.0.1").is_ok());
        assert!(validate_servers(&Mode::Relay, &[server.clone()], "127.0.0.1").is_ok());
        for url in [
            "turn:example.com:3478?transport=udp",
            "turns:127.0.0.1:5349",
            "turn:127.0.0.1:3478?transport=tcp",
            "turn:127.0.0.1:0?transport=udp",
        ] {
            server.urls = vec![url.into()];
            assert!(validate_servers(&Mode::Relay, &[server.clone()], "127.0.0.1").is_err());
        }
    }
}
