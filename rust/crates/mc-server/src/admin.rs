use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpEntry {
    pub uuid: String,
    pub name: String,
    pub level: i32,
    #[serde(rename = "bypassesPlayerLimit")]
    pub bypasses_player_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanEntry {
    pub uuid: String,
    pub name: String,
    pub source: String,
    pub expires: String,
    pub reason: String,
}

/// Manages Minecraft 26.3 persistent server administration files:
/// `ops.json`, `whitelist.json`, and `banned-players.json`.
#[derive(Debug, Clone)]
pub struct AdminStore {
    root_dir: PathBuf,
    pub whitelist_enabled: bool,
    pub ops: Vec<OpEntry>,
    pub whitelist: Vec<WhitelistEntry>,
    pub bans: Vec<BanEntry>,
}

impl AdminStore {
    pub fn load(root_dir: &Path, whitelist_enabled: bool) -> Self {
        let ops = Self::read_json(&root_dir.join("ops.json")).unwrap_or_default();
        let whitelist = Self::read_json(&root_dir.join("whitelist.json")).unwrap_or_default();
        let bans = Self::read_json(&root_dir.join("banned-players.json")).unwrap_or_default();
        Self {
            root_dir: root_dir.to_path_buf(),
            whitelist_enabled,
            ops,
            whitelist,
            bans,
        }
    }

    fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_json<T: Serialize>(&self, filename: &str, val: &T) {
        let _ = fs::create_dir_all(&self.root_dir);
        if let Ok(text) = serde_json::to_string_pretty(val) {
            let _ = fs::write(self.root_dir.join(filename), text);
        }
    }

    pub fn is_op(&self, name: &str) -> bool {
        self.ops.iter().any(|o| o.name.eq_ignore_ascii_case(name))
    }

    pub fn is_whitelisted(&self, name: &str) -> bool {
        !self.whitelist_enabled
            || self.is_op(name)
            || self
                .whitelist
                .iter()
                .any(|w| w.name.eq_ignore_ascii_case(name))
    }

    pub fn ban_reason(&self, name: &str) -> Option<&str> {
        self.bans
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
            .map(|b| b.reason.as_str())
    }

    pub fn add_op(&mut self, uuid: Uuid, name: &str) -> bool {
        if self.is_op(name) {
            return false;
        }
        self.ops.push(OpEntry {
            uuid: uuid.to_string(),
            name: name.to_string(),
            level: 4,
            bypasses_player_limit: false,
        });
        self.write_json("ops.json", &self.ops);
        true
    }

    pub fn remove_op(&mut self, name: &str) -> bool {
        let before = self.ops.len();
        self.ops.retain(|o| !o.name.eq_ignore_ascii_case(name));
        if self.ops.len() != before {
            self.write_json("ops.json", &self.ops);
            true
        } else {
            false
        }
    }

    pub fn add_whitelist(&mut self, uuid: Uuid, name: &str) -> bool {
        if self
            .whitelist
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.whitelist.push(WhitelistEntry {
            uuid: uuid.to_string(),
            name: name.to_string(),
        });
        self.write_json("whitelist.json", &self.whitelist);
        true
    }

    pub fn remove_whitelist(&mut self, name: &str) -> bool {
        let before = self.whitelist.len();
        self.whitelist
            .retain(|w| !w.name.eq_ignore_ascii_case(name));
        if self.whitelist.len() != before {
            self.write_json("whitelist.json", &self.whitelist);
            true
        } else {
            false
        }
    }

    pub fn add_ban(&mut self, uuid: Uuid, name: &str, reason: &str, source: &str) -> bool {
        if self.bans.iter().any(|b| b.name.eq_ignore_ascii_case(name)) {
            return false;
        }
        self.bans.push(BanEntry {
            uuid: uuid.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            expires: "forever".to_string(),
            reason: reason.to_string(),
        });
        self.write_json("banned-players.json", &self.bans);
        true
    }

    pub fn remove_ban(&mut self, name: &str) -> bool {
        let before = self.bans.len();
        self.bans.retain(|b| !b.name.eq_ignore_ascii_case(name));
        if self.bans.len() != before {
            self.write_json("banned-players.json", &self.bans);
            true
        } else {
            false
        }
    }
}
