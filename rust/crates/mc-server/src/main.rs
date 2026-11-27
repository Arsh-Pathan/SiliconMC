use anyhow::Result;
use mc_server::{MinecraftServer, ServerConfig};
use std::io::BufRead;
use std::path::Path;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = ServerConfig::load_or_create(Path::new("server.properties"))?;
    let server = MinecraftServer::bind(cfg).await?;

    // Interactive stdin server console thread (type `list`, `save-all`, `op <name>`, `stop`, etc.)
    let console = server.console_handle();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(cmd) => {
                    let trimmed = cmd.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let reply = console.exec_blocking(trimmed.to_string());
                    info!("[Console] {reply}");
                    if trimmed.eq_ignore_ascii_case("stop") {
                        break;
                    }
                }
                Err(e) => {
                    warn!("Console stdin closed: {e}");
                    break;
                }
            }
        }
    });

    server.run(None).await
}
