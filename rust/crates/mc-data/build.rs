use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
struct BlockEntry {
    states: Vec<BlockStateEntry>,
}

#[derive(Deserialize)]
struct BlockStateEntry {
    id: u32,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RegistryEntry {
    protocol_id: i32,
}

#[derive(Deserialize)]
struct RegistryDump {
    entries: BTreeMap<String, RegistryEntry>,
}

#[derive(Deserialize)]
struct PacketEntry {
    protocol_id: i32,
}

fn to_const_ident(mc_id: &str) -> String {
    let name = mc_id.strip_prefix("minecraft:").unwrap_or(mc_id);
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let reports_dir = repo_root.join("generated").join("reports");

    let blocks_path = reports_dir.join("blocks.json");
    let registries_path = reports_dir.join("registries.json");
    let packets_path = reports_dir.join("packets.json");

    println!("cargo:rerun-if-changed={}", blocks_path.display());
    println!("cargo:rerun-if-changed={}", registries_path.display());
    println!("cargo:rerun-if-changed={}", packets_path.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_file = File::create(out_dir.join("generated_data.rs")).unwrap();
    let mut w = BufWriter::new(out_file);

    writeln!(w, "pub const VERSION_NAME: &str = \"26.3\";").unwrap();
    writeln!(w, "pub const PROTOCOL_VERSION: i32 = 777;").unwrap();
    writeln!(w, "pub const WORLD_VERSION: i32 = 5023;").unwrap();

    // 1. Parse blocks.json
    let blocks_str = fs::read_to_string(&blocks_path)
        .expect("Run `net.minecraft.data.Main --reports --output generated` first");
    let blocks: BTreeMap<String, BlockEntry> = serde_json::from_str(&blocks_str).unwrap();

    let mut max_state_id: u32 = 0;
    for entry in blocks.values() {
        for s in &entry.states {
            if s.id > max_state_id {
                max_state_id = s.id;
            }
        }
    }
    let total_states = (max_state_id + 1) as usize;
    let mut state_to_block = vec!["minecraft:air"; total_states];
    let mut default_states: Vec<(&str, u32)> = Vec::with_capacity(blocks.len());
    let mut key_to_state: Vec<(String, u32)> = Vec::with_capacity(total_states);

    let mut air_id = 0u32;
    let mut cave_air_id = 0u32;
    let mut void_air_id = 0u32;
    let mut stone_id = 1u32;
    let mut grass_block_id = 0u32;
    let mut dirt_id = 0u32;
    let mut bedrock_id = 0u32;
    let mut water_id = 0u32;
    let mut lava_id = 0u32;

    for (block_name, entry) in &blocks {
        let mut def_id = entry.states[0].id;
        for s in &entry.states {
            state_to_block[s.id as usize] = block_name.as_str();
            if s.default {
                def_id = s.id;
            }
            if !s.properties.is_empty() {
                let mut key = String::with_capacity(block_name.len() + 32);
                key.push_str(block_name);
                key.push('|');
                for (idx, (pk, pv)) in s.properties.iter().enumerate() {
                    if idx > 0 {
                        key.push(',');
                    }
                    key.push_str(pk);
                    key.push('=');
                    key.push_str(pv);
                }
                key_to_state.push((key, s.id));
            }
        }
        default_states.push((block_name.as_str(), def_id));
        match block_name.as_str() {
            "minecraft:air" => air_id = def_id,
            "minecraft:cave_air" => cave_air_id = def_id,
            "minecraft:void_air" => void_air_id = def_id,
            "minecraft:stone" => stone_id = def_id,
            "minecraft:grass_block" => grass_block_id = def_id,
            "minecraft:dirt" => dirt_id = def_id,
            "minecraft:bedrock" => bedrock_id = def_id,
            "minecraft:water" => water_id = def_id,
            "minecraft:lava" => lava_id = def_id,
            _ => {}
        }
    }

    default_states.sort_unstable_by(|a, b| a.0.cmp(b.0));
    key_to_state.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    writeln!(w, "pub const TOTAL_BLOCK_STATES: usize = {total_states};").unwrap();
    writeln!(w, "pub const AIR_STATE_ID: u32 = {air_id};").unwrap();
    writeln!(w, "pub const CAVE_AIR_STATE_ID: u32 = {cave_air_id};").unwrap();
    writeln!(w, "pub const VOID_AIR_STATE_ID: u32 = {void_air_id};").unwrap();
    writeln!(w, "pub const STONE_STATE_ID: u32 = {stone_id};").unwrap();
    writeln!(w, "pub const GRASS_BLOCK_STATE_ID: u32 = {grass_block_id};").unwrap();
    writeln!(w, "pub const DIRT_STATE_ID: u32 = {dirt_id};").unwrap();
    writeln!(w, "pub const BEDROCK_STATE_ID: u32 = {bedrock_id};").unwrap();
    writeln!(w, "pub const WATER_STATE_ID: u32 = {water_id};").unwrap();
    writeln!(w, "pub const LAVA_STATE_ID: u32 = {lava_id};").unwrap();

    writeln!(w, "pub static BLOCK_DEFAULT_STATE: &[(&str, u32)] = &[").unwrap();
    for (name, id) in &default_states {
        writeln!(w, "    ({name:?}, {id}),").unwrap();
    }
    writeln!(w, "];").unwrap();

    writeln!(w, "pub static BLOCK_STATE_BY_KEY: &[(&str, u32)] = &[").unwrap();
    for (key, id) in &key_to_state {
        writeln!(w, "    ({key:?}, {id}),").unwrap();
    }
    writeln!(w, "];").unwrap();

    // Compact table mapping state id -> default block index (u16)
    let mut block_names_vec: Vec<&str> = default_states.iter().map(|(n, _)| *n).collect();
    block_names_vec.sort_unstable();
    writeln!(w, "pub static BLOCK_NAMES: &[&str] = &[").unwrap();
    for name in &block_names_vec {
        writeln!(w, "    {name:?},").unwrap();
    }
    writeln!(w, "];").unwrap();

    writeln!(w, "pub static STATE_TO_BLOCK_IDX: &[u16] = &[").unwrap();
    for bname in &state_to_block {
        let bidx = block_names_vec.binary_search(bname).unwrap();
        writeln!(w, "    {bidx},").unwrap();
    }
    writeln!(w, "];").unwrap();

    // 2. Parse packets.json
    let packets_str = fs::read_to_string(&packets_path).unwrap();
    let packets: BTreeMap<String, BTreeMap<String, BTreeMap<String, PacketEntry>>> =
        serde_json::from_str(&packets_str).unwrap();

    writeln!(w, "pub mod packet_ids {{").unwrap();
    for (phase, dirs) in &packets {
        writeln!(w, "    pub mod {phase} {{").unwrap();
        for (dir, entries) in dirs {
            writeln!(w, "        pub mod {dir} {{").unwrap();
            for (pkt_name, info) in entries {
                let ident = to_const_ident(pkt_name);
                writeln!(w, "            pub const {ident}: i32 = {};", info.protocol_id).unwrap();
            }
            writeln!(w, "        }}").unwrap();
        }
        writeln!(w, "    }}").unwrap();
    }
    writeln!(w, "}}").unwrap();

    // 3. Parse registries.json
    let registries_str = fs::read_to_string(&registries_path).unwrap();
    let registries: BTreeMap<String, RegistryDump> = serde_json::from_str(&registries_str).unwrap();

    let player_entity_id = registries
        .get("minecraft:entity_type")
        .and_then(|r| r.entries.get("minecraft:player"))
        .map(|e| e.protocol_id)
        .unwrap_or(0);
    writeln!(w, "pub const ENTITY_TYPE_PLAYER_ID: i32 = {player_entity_id};").unwrap();

    writeln!(w, "pub static STATIC_REGISTRIES: &[(&str, &[(&str, i32)])] = &[").unwrap();
    for (reg_name, dump) in &registries {
        writeln!(w, "    ({reg_name:?}, &[").unwrap();
        for (entry_name, info) in &dump.entries {
            writeln!(w, "        ({entry_name:?}, {}),", info.protocol_id).unwrap();
        }
        writeln!(w, "    ]),").unwrap();
    }
    writeln!(w, "];").unwrap();

    w.flush().unwrap();
}
