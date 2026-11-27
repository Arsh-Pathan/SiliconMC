use mc_data::WORLD_VERSION;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Per-advancement progress record persisted in `world/advancements/<uuid>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdvancementEntry {
    pub criteria: BTreeMap<String, String>,
    pub done: bool,
}

/// Built-in vanilla story advancement definitions: `(advancement_id, default_criterion, title)`.
pub const VANILLA_STORY_ADVANCEMENTS: &[(&str, &str, &str)] = &[
    ("minecraft:story/root", "crafting_table", "Minecraft"),
    ("minecraft:story/mine_stone", "get_stone", "Stone Age"),
    ("minecraft:story/upgrade_tools", "stone_pickaxe", "Getting an Upgrade"),
    ("minecraft:story/smelt_iron", "iron", "Acquire Hardware"),
    ("minecraft:story/iron_tools", "iron_pickaxe", "Isn't It Iron Pick"),
    ("minecraft:story/obtain_armor", "iron_chestplate", "Suit Up"),
    ("minecraft:story/lava_bucket", "lava_bucket", "Hot Stuff"),
    ("minecraft:story/form_obsidian", "obsidian", "Ice Bucket Challenge"),
    ("minecraft:story/mine_diamond", "diamond", "Diamonds!"),
    ("minecraft:story/shiny_gear", "diamond_chestplate", "Cover Me with Diamonds"),
    ("minecraft:story/enchant_item", "enchanted_item", "Enchanter"),
    ("minecraft:story/enter_the_nether", "entered_nether", "We Need to Go Deeper"),
    ("minecraft:story/enter_the_end", "entered_end", "The End?"),
];

fn normalize_mc_id(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

/// Format a Unix timestamp (seconds) into Minecraft's exact advancement timestamp format:
/// `yyyy-MM-dd HH:mm:ss +0000` (see `AdvancementProgress.OBTAINED_TIME_FORMAT`).
pub fn format_vanilla_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil-from-days algorithm (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y_base = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y_base + if month <= 2 { 1 } else { 0 };

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} +0000")
}

fn now_vanilla_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_vanilla_timestamp(secs)
}

/// Look up the display title of a built-in advancement ID (or return the ID if custom).
pub fn advancement_title(adv_id: &str) -> &'static str {
    for &(id, _, title) in VANILLA_STORY_ADVANCEMENTS {
        if id == adv_id {
            return title;
        }
    }
    "Advancement"
}

/// Look up the default criterion name for a built-in advancement ID.
pub fn default_criterion(adv_id: &str) -> &'static str {
    for &(id, criterion, _) in VANILLA_STORY_ADVANCEMENTS {
        if id == adv_id {
            return criterion;
        }
    }
    "granted"
}

/// Per-player advancement progress tracker persisting in exact vanilla
/// `world/advancements/<uuid>.json` format with `"DataVersion": 5023`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerAdvancements {
    pub player_uuid: Uuid,
    entries: BTreeMap<String, AdvancementEntry>,
}

impl PlayerAdvancements {
    pub fn new(player_uuid: Uuid) -> Self {
        Self {
            player_uuid,
            entries: BTreeMap::new(),
        }
    }

    pub fn entries(&self) -> &BTreeMap<String, AdvancementEntry> {
        &self.entries
    }

    pub fn completed_count(&self) -> usize {
        self.entries.values().filter(|e| e.done).count()
    }

    /// Grant a criterion on `adv_id`. Returns `true` ONLY when this call completes
    /// the advancement for the first time.
    pub fn grant_criterion(&mut self, adv_id: &str, criterion: &str) -> bool {
        let id = normalize_mc_id(adv_id);
        let entry = self.entries.entry(id).or_default();
        let was_done = entry.done;
        entry
            .criteria
            .entry(criterion.to_string())
            .or_insert_with(now_vanilla_timestamp);
        entry.done = true;
        !was_done
    }

    /// Grant an advancement by ID using its built-in default criterion (or `"granted"`).
    /// Returns `true` on first completion.
    pub fn grant_advancement(&mut self, adv_id: &str) -> bool {
        let id = normalize_mc_id(adv_id);
        let criterion = default_criterion(&id);
        self.grant_criterion(&id, criterion)
    }

    /// Grant all built-in vanilla story advancements. Returns the number newly completed.
    pub fn grant_all(&mut self) -> usize {
        let mut newly_granted = 0usize;
        for &(id, criterion, _) in VANILLA_STORY_ADVANCEMENTS {
            if self.grant_criterion(id, criterion) {
                newly_granted += 1;
            }
        }
        newly_granted
    }

    /// Revoke an advancement completely. Returns `true` if it existed and was removed.
    pub fn revoke(&mut self, adv_id: &str) -> bool {
        let id = normalize_mc_id(adv_id);
        self.entries.remove(&id).is_some()
    }

    /// Revoke all advancements. Returns the number of entries removed.
    pub fn revoke_all(&mut self) -> usize {
        let n = self.entries.len();
        self.entries.clear();
        n
    }

    /// Check whether an advancement is marked `done`.
    pub fn has_done(&self, adv_id: &str) -> bool {
        let id = normalize_mc_id(adv_id);
        self.entries.get(&id).map(|e| e.done).unwrap_or(false)
    }

    /// Check and award classic vanilla item-acquisition advancements for `item_name`.
    /// Returns newly completed `(advancement_id, display_title)` pairs.
    pub fn check_item_obtained(&mut self, item_name: &str) -> Vec<(&'static str, &'static str)> {
        let norm = normalize_mc_id(item_name);
        let mut awarded = Vec::new();

        let triggers: &[(&str, &str, &str)] = match norm.as_str() {
            "minecraft:crafting_table" => {
                &[("minecraft:story/root", "crafting_table", "Minecraft")]
            }
            "minecraft:cobblestone"
            | "minecraft:cobbled_deepslate"
            | "minecraft:blackstone" => {
                &[("minecraft:story/mine_stone", "get_stone", "Stone Age")]
            }
            "minecraft:stone_pickaxe" => &[
                ("minecraft:story/upgrade_tools", "stone_pickaxe", "Getting an Upgrade"),
            ],
            "minecraft:iron_ingot" => {
                &[("minecraft:story/smelt_iron", "iron", "Acquire Hardware")]
            }
            "minecraft:iron_pickaxe" => {
                &[("minecraft:story/iron_tools", "iron_pickaxe", "Isn't It Iron Pick")]
            }
            "minecraft:iron_helmet" => {
                &[("minecraft:story/obtain_armor", "iron_helmet", "Suit Up")]
            }
            "minecraft:iron_chestplate" => {
                &[("minecraft:story/obtain_armor", "iron_chestplate", "Suit Up")]
            }
            "minecraft:iron_leggings" => {
                &[("minecraft:story/obtain_armor", "iron_leggings", "Suit Up")]
            }
            "minecraft:iron_boots" => {
                &[("minecraft:story/obtain_armor", "iron_boots", "Suit Up")]
            }
            "minecraft:lava_bucket" => {
                &[("minecraft:story/lava_bucket", "lava_bucket", "Hot Stuff")]
            }
            "minecraft:obsidian" => &[
                ("minecraft:story/form_obsidian", "obsidian", "Ice Bucket Challenge"),
            ],
            "minecraft:diamond" => {
                &[("minecraft:story/mine_diamond", "diamond", "Diamonds!")]
            }
            "minecraft:diamond_helmet" => &[
                ("minecraft:story/shiny_gear", "diamond_helmet", "Cover Me with Diamonds"),
            ],
            "minecraft:diamond_chestplate" => &[
                ("minecraft:story/shiny_gear", "diamond_chestplate", "Cover Me with Diamonds"),
            ],
            "minecraft:diamond_leggings" => &[
                ("minecraft:story/shiny_gear", "diamond_leggings", "Cover Me with Diamonds"),
            ],
            "minecraft:diamond_boots" => &[
                ("minecraft:story/shiny_gear", "diamond_boots", "Cover Me with Diamonds"),
            ],
            "minecraft:enchanted_book" => {
                &[("minecraft:story/enchant_item", "enchanted_item", "Enchanter")]
            }
            _ => &[],
        };

        for &(adv_id, criterion, title) in triggers {
            if self.grant_criterion(adv_id, criterion) {
                awarded.push((adv_id, title));
            }
        }
        awarded
    }

    /// Award `minecraft:story/enchant_item` when a player enchants an item.
    pub fn check_enchanted_item(&mut self) -> Option<(&'static str, &'static str)> {
        if self.grant_criterion("minecraft:story/enchant_item", "enchanted_item") {
            Some(("minecraft:story/enchant_item", "Enchanter"))
        } else {
            None
        }
    }

    /// Check and award vanilla dimension-entry advancements (`minecraft:story/enter_the_nether`,
    /// `minecraft:story/enter_the_end`). Returns newly completed `(advancement_id, display_title)`.
    pub fn check_dimension_entered(&mut self, dim: &str) -> Vec<(&'static str, &'static str)> {
        let mut awarded = Vec::new();
        match dim.trim().to_ascii_lowercase().as_str() {
            "minecraft:the_nether" | "the_nether" | "nether" => {
                if self.grant_criterion("minecraft:story/enter_the_nether", "entered_nether") {
                    awarded.push(("minecraft:story/enter_the_nether", "We Need to Go Deeper"));
                }
            }
            "minecraft:the_end" | "the_end" | "end" => {
                if self.grant_criterion("minecraft:story/enter_the_end", "entered_end") {
                    awarded.push(("minecraft:story/enter_the_end", "The End?"));
                }
            }
            _ => {}
        }
        awarded
    }

    /// Serialize to exact vanilla Minecraft 26.3 JSON (`DataVersion: 5023`).
    pub fn to_json_value(&self) -> Value {
        let mut root = Map::new();
        for (adv_id, entry) in &self.entries {
            let mut criteria_map = Map::new();
            for (k, ts) in &entry.criteria {
                criteria_map.insert(k.clone(), Value::String(ts.clone()));
            }
            root.insert(
                adv_id.clone(),
                json!({
                    "criteria": criteria_map,
                    "done": entry.done,
                }),
            );
        }
        root.insert("DataVersion".to_string(), Value::from(WORLD_VERSION));
        Value::Object(root)
    }

    /// Parse from vanilla Minecraft JSON object.
    pub fn from_json_value(player_uuid: Uuid, value: &Value) -> Result<Self, String> {
        let Some(obj) = value.as_object() else {
            return Err("Advancements root JSON must be an object".into());
        };
        let mut out = Self::new(player_uuid);
        for (key, val) in obj {
            if key == "DataVersion" {
                continue;
            }
            let Some(entry_obj) = val.as_object() else {
                continue;
            };
            let mut criteria = BTreeMap::new();
            if let Some(cmap) = entry_obj.get("criteria").and_then(|v| v.as_object()) {
                for (ck, cv) in cmap {
                    if let Some(ts) = cv.as_str() {
                        criteria.insert(ck.clone(), ts.to_string());
                    }
                }
            }
            let done = entry_obj
                .get("done")
                .and_then(|v| v.as_bool())
                .unwrap_or(!criteria.is_empty());
            out.entries.insert(key.clone(), AdvancementEntry { criteria, done });
        }
        Ok(out)
    }

    /// Save atomically to `world_dir/advancements/<uuid>.json`.
    pub fn save(&self, world_dir: &Path) -> Result<(), String> {
        let dir = world_dir.join("advancements");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.json", self.player_uuid));
        let tmp = dir.join(format!("{}.json.tmp", self.player_uuid));
        let payload =
            serde_json::to_string_pretty(&self.to_json_value()).map_err(|e| e.to_string())?;
        fs::write(&tmp, payload).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load from `world_dir/advancements/<uuid>.json`, returning an empty store if missing.
    pub fn load(world_dir: &Path, player_uuid: Uuid) -> Self {
        let path = world_dir
            .join("advancements")
            .join(format!("{player_uuid}.json"));
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::new(player_uuid);
        };
        let Ok(val) = serde_json::from_str::<Value>(&raw) else {
            return Self::new(player_uuid);
        };
        Self::from_json_value(player_uuid, &val).unwrap_or_else(|_| Self::new(player_uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_timestamp_formatting() {
        assert_eq!(format_vanilla_timestamp(0), "1970-01-01 00:00:00 +0000");
        // 2024-02-29 12:34:56 UTC = 1709210096
        assert_eq!(
            format_vanilla_timestamp(1_709_210_096),
            "2024-02-29 12:34:56 +0000"
        );
    }

    #[test]
    fn grant_revoke_item_and_dimension_triggers() {
        let uuid = Uuid::new_v4();
        let mut adv = PlayerAdvancements::new(uuid);

        assert!(adv.grant_criterion("minecraft:story/root", "crafting_table"));
        // Second call returns false (already done)
        assert!(!adv.grant_criterion("minecraft:story/root", "crafting_table"));
        assert!(adv.has_done("story/root"));

        assert_eq!(
            adv.check_item_obtained("cobblestone"),
            vec![("minecraft:story/mine_stone", "Stone Age")]
        );
        // Repeat item does not re-award
        assert!(adv.check_item_obtained("minecraft:cobblestone").is_empty());

        assert_eq!(
            adv.check_item_obtained("minecraft:stone_pickaxe"),
            vec![("minecraft:story/upgrade_tools", "Getting an Upgrade")]
        );
        assert_eq!(
            adv.check_item_obtained("minecraft:iron_ingot"),
            vec![("minecraft:story/smelt_iron", "Acquire Hardware")]
        );
        assert_eq!(
            adv.check_item_obtained("minecraft:diamond"),
            vec![("minecraft:story/mine_diamond", "Diamonds!")]
        );
        assert_eq!(
            adv.check_item_obtained("minecraft:enchanted_book"),
            vec![("minecraft:story/enchant_item", "Enchanter")]
        );

        assert_eq!(
            adv.check_dimension_entered("minecraft:the_nether"),
            vec![("minecraft:story/enter_the_nether", "We Need to Go Deeper")]
        );
        assert_eq!(
            adv.check_dimension_entered("the_end"),
            vec![("minecraft:story/enter_the_end", "The End?")]
        );

        assert!(adv.revoke("minecraft:story/mine_diamond"));
        assert!(!adv.has_done("minecraft:story/mine_diamond"));
        assert!(!adv.revoke("minecraft:story/mine_diamond"));
    }

    #[test]
    fn save_and_load_exact_vanilla_json_format() {
        let tmp = std::env::temp_dir().join("siliconmc_advancements_test");
        let _ = fs::remove_dir_all(&tmp);

        let uuid = Uuid::new_v4();
        let mut adv = PlayerAdvancements::new(uuid);
        adv.check_item_obtained("minecraft:cobblestone");
        adv.check_dimension_entered("minecraft:the_nether");
        adv.save(&tmp).unwrap();

        let file_path = tmp.join("advancements").join(format!("{uuid}.json"));
        let raw = fs::read_to_string(&file_path).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["DataVersion"], json!(5023));
        assert_eq!(parsed["minecraft:story/mine_stone"]["done"], json!(true));
        assert!(
            parsed["minecraft:story/mine_stone"]["criteria"]["get_stone"]
                .as_str()
                .unwrap()
                .ends_with(" +0000")
        );

        let loaded = PlayerAdvancements::load(&tmp, uuid);
        assert_eq!(loaded, adv);
        assert!(loaded.has_done("minecraft:story/mine_stone"));
        assert!(loaded.has_done("minecraft:story/enter_the_nether"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
