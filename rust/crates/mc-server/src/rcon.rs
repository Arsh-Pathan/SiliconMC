use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{info, warn};

const SERVERDATA_AUTH: i32 = 3;
const SERVERDATA_AUTH_RESPONSE: i32 = 2;
const SERVERDATA_EXECCOMMAND: i32 = 2;
const SERVERDATA_RESPONSE_VALUE: i32 = 0;

pub type RconCommandHandler =
    Arc<dyn Fn(String, oneshot::Sender<String>) + Send + Sync + 'static>;

/// Source RCON TCP server compatible with Minecraft's `enable-rcon=true`.
pub struct RconServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    password: String,
    handler: RconCommandHandler,
}

impl RconServer {
    pub async fn bind(
        bind_addr: &str,
        port: u16,
        password: String,
        handler: RconCommandHandler,
    ) -> Result<Self> {
        let listener = TcpListener::bind(format!("{bind_addr}:{port}")).await?;
        let local_addr = listener.local_addr()?;
        info!("RCON server listening on {local_addr}");
        Ok(Self {
            listener,
            local_addr,
            password,
            handler,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run(self) {
        let password = Arc::new(self.password);
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    let pw = Arc::clone(&password);
                    let handler = Arc::clone(&self.handler);
                    tokio::spawn(async move {
                        if let Err(e) = handle_rcon_client(stream, &pw, handler).await {
                            warn!("RCON client {peer} disconnected: {e}");
                        }
                    });
                }
                Err(e) => {
                    warn!("RCON accept error: {e}");
                    break;
                }
            }
        }
    }
}

async fn read_rcon_packet(stream: &mut TcpStream) -> Result<(i32, i32, String)> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = i32::from_le_bytes(len_buf);
    if !(10..=4096).contains(&len) {
        anyhow::bail!("Invalid RCON packet length: {len}");
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await?;
    let req_id = i32::from_le_bytes(body[0..4].try_into().unwrap());
    let pkt_type = i32::from_le_bytes(body[4..8].try_into().unwrap());
    let payload_end = body.len().saturating_sub(2);
    let payload = String::from_utf8_lossy(&body[8..payload_end])
        .trim_end_matches('\0')
        .to_string();
    Ok((req_id, pkt_type, payload))
}

async fn write_rcon_packet(
    stream: &mut TcpStream,
    req_id: i32,
    pkt_type: i32,
    payload: &str,
) -> Result<()> {
    let bytes = payload.as_bytes();
    let len = (4 + 4 + bytes.len() + 2) as i32;
    let mut buf = Vec::with_capacity((len + 4) as usize);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&req_id.to_le_bytes());
    buf.extend_from_slice(&pkt_type.to_le_bytes());
    buf.extend_from_slice(bytes);
    buf.push(0);
    buf.push(0);
    stream.write_all(&buf).await?;
    Ok(())
}

async fn handle_rcon_client(
    mut stream: TcpStream,
    expected_pw: &str,
    handler: RconCommandHandler,
) -> Result<()> {
    let mut authed = false;
    loop {
        let (req_id, pkt_type, payload) = read_rcon_packet(&mut stream).await?;
        match pkt_type {
            SERVERDATA_AUTH => {
                if !expected_pw.is_empty() && payload == expected_pw {
                    authed = true;
                    write_rcon_packet(&mut stream, req_id, SERVERDATA_AUTH_RESPONSE, "").await?;
                } else {
                    authed = false;
                    write_rcon_packet(&mut stream, -1, SERVERDATA_AUTH_RESPONSE, "").await?;
                }
            }
            SERVERDATA_EXECCOMMAND => {
                if !authed {
                    write_rcon_packet(&mut stream, -1, SERVERDATA_AUTH_RESPONSE, "").await?;
                    continue;
                }
                let (tx, rx) = oneshot::channel();
                (handler)(payload, tx);
                let reply = rx.await.unwrap_or_else(|_| "Server unavailable".into());
                write_rcon_packet(&mut stream, req_id, SERVERDATA_RESPONSE_VALUE, &reply).await?;
            }
            _ => {
                write_rcon_packet(
                    &mut stream,
                    req_id,
                    SERVERDATA_RESPONSE_VALUE,
                    "Unknown RCON request type",
                )
                .await?;
            }
        }
    }
}
