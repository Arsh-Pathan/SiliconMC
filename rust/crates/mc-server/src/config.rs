use anyhow::Result;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub port: u16,
    pub motd: String,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub online_mode: bool,
    pub compression_threshold: i32,
    pub gamemode: i32,
    pub level_seed: i64,
    pub generator_type: String,
    pub white_list: bool,
    pub enable_rcon: bool,
    pub rcon_port: u16,
    pub rcon_password: String,
    pub management_server_enabled: bool,
    pub management_server_port: u16,
    pub management_server_secret: String,
    pub world_dir: PathBuf,
    pub data_dir: PathBuf,
    pub network_tags_path: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0".to_string(),
            port: 25565,
            motd: "SiliconMC Rust 26.3 Server".to_string(),
            max_players: 20,
            view_distance: 6,
            simulation_distance: 6,
            online_mode: false,
            compression_threshold: 256,
            gamemode: 1, // Creative
            level_seed: 263_2026,
            generator_type: "normal".to_string(),
            white_list: false,
            enable_rcon: false,
            rcon_port: 25575,
            rcon_password: String::new(),
            management_server_enabled: false,
            management_server_port: 25585,
            management_server_secret: String::new(),
            world_dir: PathBuf::from("world"),
            data_dir: find_default_data_dir(),
            network_tags_path: find_default_tags_path(),
        }
    }
}

fn find_default_tags_path() -> Option<PathBuf> {
    for candidate in [
        PathBuf::from("rust/fixtures/vanilla_network_tags.bin"),
        PathBuf::from("fixtures/vanilla_network_tags.bin"),
        PathBuf::from("../fixtures/vanilla_network_tags.bin"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_default_data_dir() -> PathBuf {
    for candidate in [
        PathBuf::from("src/main/resources/data"),
        PathBuf::from("../src/main/resources/data"),
        PathBuf::from("data"),
    ] {
        if candidate.join("minecraft").is_dir() {
            return candidate;
        }
    }
    PathBuf::from("src/main/resources/data")
}

impl ServerConfig {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let mut cfg = Self::default();
        if !path.exists() {
            let content = format!(
                "# SiliconMC Rust 26.3 server.properties\n\
                 server-ip={}\n\
                 server-port={}\n\
                 motd={}\n\
                 max-players={}\n\
                 view-distance={}\n\
                 simulation-distance={}\n\
                 online-mode={}\n\
                 network-compression-threshold={}\n\
                 gamemode=creative\n\
                 level-seed={}\n\
                 level-type={}\n\
                 level-name={}\n",
                cfg.bind_addr,
                cfg.port,
                cfg.motd,
                cfg.max_players,
                cfg.view_distance,
                cfg.simulation_distance,
                cfg.online_mode,
                cfg.compression_threshold,
                cfg.level_seed,
                cfg.generator_type,
                cfg.world_dir.display(),
            );
            fs::write(path, content)?;
            return Ok(cfg);
        }

        let raw = fs::read_to_string(path)?;
        let mut props = BTreeMap::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                props.insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        if let Some(v) = props.get("server-ip") {
            if !v.is_empty() {
                cfg.bind_addr = v.clone();
            }
        }
        if let Some(v) = props.get("server-port").and_then(|s| s.parse().ok()) {
            cfg.port = v;
        }
        if let Some(v) = props.get("motd") {
            cfg.motd = v.clone();
        }
        if let Some(v) = props.get("max-players").and_then(|s| s.parse().ok()) {
            cfg.max_players = v;
        }
        if let Some(v) = props.get("view-distance").and_then(|s| s.parse::<i32>().ok()) {
            cfg.view_distance = v.clamp(2, 32);
        }
        if let Some(v) = props
            .get("simulation-distance")
            .and_then(|s| s.parse::<i32>().ok())
        {
            cfg.simulation_distance = v.clamp(2, 32);
        }
        if let Some(v) = props.get("online-mode").and_then(|s| s.parse().ok()) {
            cfg.online_mode = v;
        }
        if let Some(v) = props
            .get("network-compression-threshold")
            .and_then(|s| s.parse().ok())
        {
            cfg.compression_threshold = v;
        }
        if let Some(v) = props.get("gamemode") {
            cfg.gamemode = match v.as_str() {
                "survival" | "0" => 0,
                "creative" | "1" => 1,
                "adventure" | "2" => 2,
                "spectator" | "3" => 3,
                _ => 1,
            };
        }
        if let Some(v) = props.get("level-name") {
            cfg.world_dir = PathBuf::from(v);
        }
        if let Some(v) = props.get("level-seed") {
            if !v.is_empty() {
                cfg.level_seed = v
                    .parse::<i64>()
                    .unwrap_or_else(|_| mc_math::java_string_hash_code(v) as i64);
            }
        }
        if let Some(v) = props.get("level-type") {
            if !v.is_empty() {
                cfg.generator_type = v.to_ascii_lowercase();
            }
        }
        if let Some(v) = props.get("white-list").and_then(|s| s.parse().ok()) {
            cfg.white_list = v;
        }
        if let Some(v) = props.get("enable-rcon").and_then(|s| s.parse().ok()) {
            cfg.enable_rcon = v;
        }
        if let Some(v) = props.get("rcon.port").and_then(|s| s.parse().ok()) {
            cfg.rcon_port = v;
        }
        if let Some(v) = props.get("rcon.password") {
            cfg.rcon_password = v.clone();
        }
        if let Some(v) = props
            .get("management-server-enabled")
            .and_then(|s| s.parse().ok())
        {
            cfg.management_server_enabled = v;
        }
        if let Some(v) = props
            .get("management-server-port")
            .and_then(|s| s.parse().ok())
        {
            cfg.management_server_port = v;
        }
        if let Some(v) = props.get("management-server-secret") {
            cfg.management_server_secret = v.clone();
        }

        Ok(cfg)
    }
}
