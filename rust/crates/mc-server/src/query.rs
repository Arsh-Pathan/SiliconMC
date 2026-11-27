use anyhow::Result;
use mc_data::VERSION_NAME;
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tracing::{info, warn};

/// Snapshot of live server status answered over the vanilla Minecraft GameSpy4 UDP Query protocol.
#[derive(Debug, Clone)]
pub struct QuerySnapshot {
    pub motd: String,
    pub world_name: String,
    pub max_players: i32,
    pub server_port: u16,
    pub host_ip: String,
    pub players: Vec<String>,
}

pub type QueryCallback = Arc<dyn Fn(oneshot::Sender<QuerySnapshot>) + Send + Sync>;

pub struct QueryServer {
    socket: UdpSocket,
    local_addr: SocketAddr,
    callback: QueryCallback,
}

impl QueryServer {
    pub async fn bind(bind_addr: &str, port: u16, callback: QueryCallback) -> Result<Self> {
        let socket = UdpSocket::bind(format!("{bind_addr}:{port}")).await?;
        let local_addr = socket.local_addr()?;
        info!("GameSpy4 UDP Query listening on {local_addr}");
        Ok(Self {
            socket,
            local_addr,
            callback,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run(self) {
        let mut buf = [0u8; 1024];
        let mut challenges: HashMap<SocketAddr, (i32, Instant)> = HashMap::new();

        loop {
            let (len, peer) = match self.socket.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(e) => {
                    warn!("UDP Query recv error: {e}");
                    continue;
                }
            };
            if len < 7 || buf[0] != 0xFE || buf[1] != 0xFD {
                continue;
            }
            let pkt_type = buf[2];
            let session_id = [buf[3], buf[4], buf[5], buf[6]];

            // Prune expired challenge tokens (> 30s)
            let now = Instant::now();
            challenges.retain(|_, (_, ts)| now.duration_since(*ts) < Duration::from_secs(30));

            match pkt_type {
                9 => {
                    // Handshake -> issue random challenge token as null-terminated ASCII decimal
                    let token = (rand::thread_rng().next_u32() & 0x00FF_FFFF) as i32;
                    challenges.insert(peer, (token, now));
                    let mut out = Vec::with_capacity(24);
                    out.push(9);
                    out.extend_from_slice(&session_id);
                    out.extend_from_slice(token.to_string().as_bytes());
                    out.push(0);
                    let _ = self.socket.send_to(&out, peer).await;
                }
                0 => {
                    // Stat request: validate 4-byte big-endian challenge token at bytes 7..11
                    if len < 11 {
                        continue;
                    }
                    let req_tok = i32::from_be_bytes([buf[7], buf[8], buf[9], buf[10]]);
                    let valid = challenges
                        .get(&peer)
                        .map(|&(tok, _)| tok == req_tok)
                        .unwrap_or(false);
                    if !valid {
                        continue;
                    }
                    let (tx, rx) = oneshot::channel();
                    (self.callback)(tx);
                    let Ok(snap) = rx.await else {
                        continue;
                    };

                    let mut out = Vec::with_capacity(512);
                    out.push(0);
                    out.extend_from_slice(&session_id);
                    if len >= 15 {
                        // Full stat (rules + player list) matching vanilla QueryThreadGs4.buildRuleResponse
                        write_cstring(&mut out, "splitnum");
                        out.push(128);
                        out.push(0);
                        for (k, v) in [
                            ("hostname", snap.motd.as_str()),
                            ("gametype", "SMP"),
                            ("game_id", "MINECRAFT"),
                            ("version", VERSION_NAME),
                            ("plugins", "SiliconMC-Rust"),
                            ("map", snap.world_name.as_str()),
                        ] {
                            write_cstring(&mut out, k);
                            write_cstring(&mut out, v);
                        }
                        write_cstring(&mut out, "numplayers");
                        write_cstring(&mut out, &snap.players.len().to_string());
                        write_cstring(&mut out, "maxplayers");
                        write_cstring(&mut out, &snap.max_players.to_string());
                        write_cstring(&mut out, "hostport");
                        write_cstring(&mut out, &snap.server_port.to_string());
                        write_cstring(&mut out, "hostip");
                        write_cstring(&mut out, &snap.host_ip);
                        out.push(0);
                        out.push(1);
                        write_cstring(&mut out, "player_");
                        out.push(0);
                        for pname in &snap.players {
                            write_cstring(&mut out, pname);
                        }
                        out.push(0);
                    } else {
                        // Basic stat
                        write_cstring(&mut out, &snap.motd);
                        write_cstring(&mut out, "SMP");
                        write_cstring(&mut out, &snap.world_name);
                        write_cstring(&mut out, &snap.players.len().to_string());
                        write_cstring(&mut out, &snap.max_players.to_string());
                        out.extend_from_slice(&snap.server_port.to_le_bytes());
                        write_cstring(&mut out, &snap.host_ip);
                    }
                    let _ = self.socket.send_to(&out, peer).await;
                }
                _ => {}
            }
        }
    }
}

fn write_cstring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}
