use anyhow::Result;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::{info, warn};

pub type JsonRpcDispatcher =
    Arc<dyn Fn(String, Value, oneshot::Sender<Result<Value, (i32, String)>>) + Send + Sync + 'static>;

/// Minecraft 26.3 JSON-RPC 2.0 Management Server (`management-server-enabled=true`).
/// Supports HTTP POST JSON-RPC 2.0 with Bearer token authentication matching
/// `generated/reports/json-rpc-api-schema.json`.
pub struct JsonRpcServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    secret: String,
    dispatcher: JsonRpcDispatcher,
}

impl JsonRpcServer {
    pub async fn bind(
        bind_addr: &str,
        port: u16,
        secret: String,
        dispatcher: JsonRpcDispatcher,
    ) -> Result<Self> {
        let listener = TcpListener::bind(format!("{bind_addr}:{port}")).await?;
        let local_addr = listener.local_addr()?;
        info!("Minecraft 26.3 JSON-RPC Management Server listening on {local_addr}");
        Ok(Self {
            listener,
            local_addr,
            secret,
            dispatcher,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run(self) {
        let secret = Arc::new(self.secret);
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    let sec = Arc::clone(&secret);
                    let disp = Arc::clone(&self.dispatcher);
                    tokio::spawn(async move {
                        if let Err(e) = handle_http_rpc(stream, &sec, disp).await {
                            warn!("JSON-RPC connection {peer} error: {e}");
                        }
                    });
                }
                Err(e) => {
                    warn!("JSON-RPC accept error: {e}");
                    break;
                }
            }
        }
    }
}

async fn handle_http_rpc(
    mut stream: TcpStream,
    expected_secret: &str,
    dispatcher: JsonRpcDispatcher,
) -> Result<()> {
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let (headers_part, body_part) = req_str.split_once("\r\n\r\n").unwrap_or((&req_str, ""));

    // Bearer authentication check when a secret is configured
    if !expected_secret.is_empty() {
        let mut authorized = false;
        for line in headers_part.lines() {
            if let Some(val) = line
                .strip_prefix("Authorization:")
                .or_else(|| line.strip_prefix("authorization:"))
            {
                if val.trim() == format!("Bearer {expected_secret}") {
                    authorized = true;
                }
            }
        }
        if !authorized {
            send_http_json(
                &mut stream,
                401,
                &json!({"jsonrpc":"2.0","error":{"code":-32001,"message":"Unauthorized"},"id":null}),
            )
            .await?;
            return Ok(());
        }
    }

    let parsed: Value = match serde_json::from_str(body_part.trim()) {
        Ok(v) => v,
        Err(e) => {
            send_http_json(
                &mut stream,
                400,
                &json!({"jsonrpc":"2.0","error":{"code":-32700,"message":format!("Parse error: {e}")},"id":null}),
            )
            .await?;
            return Ok(());
        }
    };

    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = parsed.get("method").and_then(|v| v.as_str()) else {
        send_http_json(
            &mut stream,
            200,
            &json!({"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":id}),
        )
        .await?;
        return Ok(());
    };
    let params = parsed.get("params").cloned().unwrap_or(Value::Null);

    let response = if method == "rpc.discover" {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "openrpc": "1.3.2",
                "info": { "title": "SiliconMC 26.3 Management API", "version": mc_data::VERSION_NAME },
                "methods": [
                    "minecraft:server/status",
                    "minecraft:server/save",
                    "minecraft:server/stop",
                    "minecraft:server/system_message",
                    "minecraft:players",
                    "minecraft:operators",
                    "minecraft:operators/add",
                    "minecraft:operators/remove",
                    "minecraft:allowlist",
                    "minecraft:allowlist/add",
                    "minecraft:allowlist/remove",
                    "minecraft:bans",
                    "minecraft:bans/add",
                    "minecraft:bans/remove"
                ]
            }
        })
    } else {
        let (tx, rx) = oneshot::channel();
        (dispatcher)(method.to_string(), params, tx);
        match rx.await {
            Ok(Ok(res)) => json!({ "jsonrpc": "2.0", "id": id, "result": res }),
            Ok(Err((code, msg))) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": msg }
            }),
            Err(_) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": "Internal error" }
            }),
        }
    };

    send_http_json(&mut stream, 200, &response).await?;
    Ok(())
}

async fn send_http_json(stream: &mut TcpStream, status: u16, body: &Value) -> Result<()> {
    let payload = serde_json::to_string(body)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    Ok(())
}
