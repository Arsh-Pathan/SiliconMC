use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// Persistent client talking NDJSON over stdin/stdout to Java's `com.siliconmc.oracle.OracleMain`.
pub struct OracleClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl OracleClient {
    /// Locate the SiliconMC repository root (parent of `rust/`).
    pub fn repo_root() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // rust/crates/mc-oracle -> rust/crates -> rust -> SiliconMC
        manifest_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("mc-oracle must reside inside SiliconMC/rust/crates/mc-oracle")
            .to_path_buf()
    }

    pub fn spawn() -> Result<Self> {
        let root = Self::repo_root();
        let classes = root.join("target").join("classes");
        let shaded = root
            .join("target")
            .join("siliconmc-26.3-SNAPSHOT-shaded.jar");
        if !shaded.exists() {
            bail!(
                "Java oracle shaded JAR not found at {}. Run `mvn package` first.",
                shaded.display()
            );
        }

        let sep = if cfg!(windows) { ";" } else { ":" };
        let cp = format!("{}{}{}", classes.display(), sep, shaded.display());

        let mut child = Command::new("java")
            .arg("-Xmx1G")
            .arg("-cp")
            .arg(&cp)
            .arg("com.siliconmc.oracle.OracleMain")
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn Java OracleMain")?;

        let stdin = child.stdin.take().context("Missing oracle stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("Missing oracle stdout")?);

        let mut client = Self {
            child,
            stdin,
            stdout,
        };
        let pong = client.call(json!({ "op": "ping" }))?;
        if !pong.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            bail!("Oracle ping failed: {pong}");
        }
        Ok(client)
    }

    pub fn call(&mut self, request: Value) -> Result<Value> {
        let line = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line)?;
        if n == 0 {
            bail!("Java oracle exited unexpectedly while handling {line}");
        }
        let val: Value = serde_json::from_str(resp_line.trim())
            .with_context(|| format!("Invalid JSON from Java oracle: {resp_line}"))?;
        if !val.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let err = val
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("Java oracle error for {}: {}", request["op"], err);
        }
        Ok(val)
    }

    pub fn packet_roundtrip(
        &mut self,
        phase: &str,
        direction: &str,
        bytes: &[u8],
    ) -> Result<(String, String, Vec<u8>)> {
        let resp = self.call(json!({
            "op": "packet_roundtrip",
            "phase": phase,
            "direction": direction,
            "bytes_b64": B64.encode(bytes),
        }))?;
        let ptype = resp["packet_type"]
            .as_str()
            .context("missing packet_type")?
            .to_string();
        let jclass = resp["java_class"]
            .as_str()
            .context("missing java_class")?
            .to_string();
        let out_b64 = resp["bytes_b64"].as_str().context("missing bytes_b64")?;
        let out_bytes = B64.decode(out_b64)?;
        Ok((ptype, jclass, out_bytes))
    }

    pub fn nbt_roundtrip(&mut self, mode: &str, bytes: &[u8]) -> Result<(Vec<u8>, String)> {
        let resp = self.call(json!({
            "op": "nbt_roundtrip",
            "mode": mode,
            "bytes_b64": B64.encode(bytes),
        }))?;
        let out_bytes = B64.decode(resp["bytes_b64"].as_str().context("missing bytes_b64")?)?;
        let snbt = resp["snbt"].as_str().context("missing snbt")?.to_string();
        Ok((out_bytes, snbt))
    }
}

impl Drop for OracleClient {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{{\"op\":\"quit\"}}");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

static SHARED_ORACLE: OnceLock<Mutex<OracleClient>> = OnceLock::new();

/// Execute a closure against a process-wide shared Java OracleMain instance.
pub fn with_oracle<T>(f: impl FnOnce(&mut OracleClient) -> Result<T>) -> Result<T> {
    let mutex = SHARED_ORACLE.get_or_init(|| {
        Mutex::new(OracleClient::spawn().expect("Failed to start Java OracleMain"))
    });
    let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_ping_matches_minecraft_26_3() {
        let resp = with_oracle(|o| o.call(json!({ "op": "ping" }))).unwrap();
        assert_eq!(resp["version"].as_str(), Some("26.3"));
        assert_eq!(resp["protocol"].as_i64(), Some(777));
        assert_eq!(resp["world_version"].as_i64(), Some(5023));
    }
}
