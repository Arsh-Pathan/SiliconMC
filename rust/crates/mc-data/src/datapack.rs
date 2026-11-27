use indexmap::IndexMap;
use mc_nbt::NbtTag;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatapackError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// Ordered list of synchronized dynamic registries in Minecraft 26.3 (`RegistryDataLoader.SYNCHRONIZED_REGISTRIES`),
/// mapping `(registry_id, datapack_relative_dir)`.
pub const SYNCED_REGISTRY_DIRS: &[(&str, &str)] = &[
    ("minecraft:worldgen/biome", "worldgen/biome"),
    ("minecraft:chat_type", "chat_type"),
    ("minecraft:trim_pattern", "trim_pattern"),
    ("minecraft:trim_material", "trim_material"),
    ("minecraft:wolf_variant", "wolf_variant"),
    ("minecraft:wolf_sound_variant", "wolf_sound_variant"),
    ("minecraft:pig_variant", "pig_variant"),
    ("minecraft:pig_sound_variant", "pig_sound_variant"),
    ("minecraft:frog_variant", "frog_variant"),
    ("minecraft:cat_variant", "cat_variant"),
    ("minecraft:cat_sound_variant", "cat_sound_variant"),
    ("minecraft:cow_sound_variant", "cow_sound_variant"),
    ("minecraft:cow_variant", "cow_variant"),
    ("minecraft:chicken_sound_variant", "chicken_sound_variant"),
    ("minecraft:chicken_variant", "chicken_variant"),
    ("minecraft:zombie_nautilus_variant", "zombie_nautilus_variant"),
    ("minecraft:painting_variant", "painting_variant"),
    ("minecraft:sulfur_cube_archetype", "sulfur_cube_archetype"),
    ("minecraft:dimension_type", "dimension_type"),
    ("minecraft:damage_type", "damage_type"),
    ("minecraft:banner_pattern", "banner_pattern"),
    ("minecraft:enchantment", "enchantment"),
    ("minecraft:jukebox_song", "jukebox_song"),
    ("minecraft:instrument", "instrument"),
    ("minecraft:test_environment", "test_environment"),
    ("minecraft:test_instance", "test_instance"),
    ("minecraft:dialog", "dialog"),
    ("minecraft:world_clock", "world_clock"),
    ("minecraft:timeline", "timeline"),
    ("minecraft:decorated_pot_pattern", "decorated_pot_pattern"),
    ("minecraft:block_transformer", "block_transformer"),
    ("minecraft:worldgen/block_state_provider", "worldgen/block_state_provider"),
];

#[derive(Debug, Clone)]
pub struct DynamicRegistryEntry {
    pub id: String,
    pub data: Option<NbtTag>,
}

#[derive(Debug, Clone)]
pub struct DynamicRegistry {
    pub registry_id: String,
    pub entries: Vec<DynamicRegistryEntry>,
    pub id_to_index: IndexMap<String, u32>,
}

impl DynamicRegistry {
    pub fn index_of(&self, id: &str) -> Option<u32> {
        self.id_to_index.get(id).copied()
    }
}

#[derive(Debug, Clone)]
pub struct LoadedDatapacks {
    pub registries: IndexMap<String, DynamicRegistry>,
}

impl LoadedDatapacks {
    /// Load all synchronized dynamic registries from a vanilla/custom `data/` root directory
    /// (e.g., `src/main/resources/data`).
    pub fn load_from_data_dir(data_dir: &Path) -> Result<Self, DatapackError> {
        let mut registries = IndexMap::new();
        let mc_dir = data_dir.join("minecraft");

        for &(reg_id, rel_dir) in SYNCED_REGISTRY_DIRS {
            let dir = mc_dir.join(rel_dir);
            let mut entries = Vec::new();
            let mut id_to_index = IndexMap::new();

            if dir.is_dir() {
                let mut files = Vec::new();
                collect_json_files(&dir, &dir, &mut files)?;
                files.sort();
                for (rel_name, full_path) in files {
                    let raw = fs::read_to_string(&full_path)?;
                    let json_val: serde_json::Value =
                        serde_json::from_str(&raw).map_err(|source| DatapackError::Json {
                            path: full_path.clone(),
                            source,
                        })?;
                    let entry_id = format!("minecraft:{rel_name}");
                    let idx = entries.len() as u32;
                    id_to_index.insert(entry_id.clone(), idx);
                    entries.push(DynamicRegistryEntry {
                        id: entry_id,
                        data: Some(NbtTag::from_json(&json_val)),
                    });
                }
            }

            if !entries.is_empty() {
                registries.insert(
                    reg_id.to_string(),
                    DynamicRegistry {
                        registry_id: reg_id.to_string(),
                        entries,
                        id_to_index,
                    },
                );
            }
        }

        Ok(Self { registries })
    }

    pub fn biome_id(&self, name: &str) -> u32 {
        self.registries
            .get("minecraft:worldgen/biome")
            .and_then(|r| r.index_of(name))
            .unwrap_or(0)
    }

    pub fn biome_name(&self, id: u32) -> &str {
        self.registries
            .get("minecraft:worldgen/biome")
            .and_then(|r| r.entries.get(id as usize))
            .map(|e| e.id.as_str())
            .unwrap_or("minecraft:plains")
    }

    pub fn biome_count(&self) -> usize {
        self.registries
            .get("minecraft:worldgen/biome")
            .map(|r| r.entries.len())
            .unwrap_or(1)
    }

    pub fn dimension_type_id(&self, name: &str) -> u32 {
        self.registries
            .get("minecraft:dimension_type")
            .and_then(|r| r.index_of(name))
            .unwrap_or(0)
    }

    pub fn world_clock_id(&self, name: &str) -> u32 {
        self.registries
            .get("minecraft:world_clock")
            .and_then(|r| r.index_of(name))
            .unwrap_or(0)
    }
}

fn collect_json_files(
    base: &Path,
    cur: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(cur)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(base, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let rel = path.strip_prefix(base).unwrap();
            let mut key = rel.to_string_lossy().replace('\\', "/");
            if let Some(stripped) = key.strip_suffix(".json") {
                key = stripped.to_string();
            }
            out.push((key, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::OracleClient;
    use serde_json::json;

    #[test]
    fn diff_datapack_registry_entries_against_java_oracle() {
        let data_dir = OracleClient::repo_root()
            .join("src")
            .join("main")
            .join("resources")
            .join("data");
        let loaded = LoadedDatapacks::load_from_data_dir(&data_dir).unwrap();

        let resp = mc_oracle::with_oracle(|o| {
            o.call(json!({ "op": "dump_synced_registries", "include_data": false }))
        })
        .unwrap();
        let java_regs = resp["registries"].as_array().unwrap();
        assert_eq!(loaded.registries.len(), java_regs.len(), "Synced registry count mismatch");

        for jreg in java_regs {
            let reg_id = jreg["registry"].as_str().unwrap();
            let rust_reg = loaded
                .registries
                .get(reg_id)
                .unwrap_or_else(|| panic!("Missing synced registry {reg_id}"));
            let mut java_ids: Vec<&str> = jreg["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["id"].as_str().unwrap())
                .collect();
            java_ids.sort_unstable();
            let mut rust_ids: Vec<&str> = rust_reg.entries.iter().map(|e| e.id.as_str()).collect();
            rust_ids.sort_unstable();
            assert_eq!(rust_ids, java_ids, "Entry IDs mismatch in registry {reg_id}");
        }
    }
}
