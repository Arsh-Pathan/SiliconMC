pub mod datapack;
pub mod generated;
pub use datapack::*;
pub use generated::{
    block_behaviors, block_extra, block_props, gameplay_tables, items, loot_and_tools, mob_spawns,
    noises, overworld_biomes, recipes,
};

use block_props::{BLOCK_STATE_NBT_PROPS, BLOCK_STATE_PROPS, BlockStateProps};
use mc_nbt::{NbtCompound, NbtTag};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/generated_data.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockStateId(pub u32);

impl BlockStateId {
    pub const AIR: Self = Self(AIR_STATE_ID);
    pub const STONE: Self = Self(STONE_STATE_ID);
    pub const GRASS_BLOCK: Self = Self(GRASS_BLOCK_STATE_ID);
    pub const DIRT: Self = Self(DIRT_STATE_ID);
    pub const BEDROCK: Self = Self(BEDROCK_STATE_ID);

    #[inline]
    pub fn block_name(self) -> &'static str {
        let idx = STATE_TO_BLOCK_IDX
            .get(self.0 as usize)
            .copied()
            .unwrap_or(0) as usize;
        BLOCK_NAMES.get(idx).copied().unwrap_or("minecraft:air")
    }

    #[inline]
    pub fn props(self) -> BlockStateProps {
        BLOCK_STATE_PROPS
            .get(self.0 as usize)
            .copied()
            .unwrap_or(BLOCK_STATE_PROPS[0])
    }

    #[inline]
    pub fn is_air(self) -> bool {
        self.props().is_air()
    }

    #[inline]
    pub fn is_fluid_block(self) -> bool {
        self.props().has_fluid()
    }

    /// Convert this exact 26.3 `BlockStateId` back into a vanilla Anvil palette NBT compound
    /// (`{Name: "minecraft:...", Properties: {...}}`).
    pub fn to_nbt(self) -> NbtCompound {
        let mut c = NbtCompound::new();
        c.insert("Name", NbtTag::String(self.block_name().to_string()));
        let prop_str = BLOCK_STATE_NBT_PROPS
            .get(self.0 as usize)
            .copied()
            .unwrap_or("");
        if !prop_str.is_empty() {
            let mut props = NbtCompound::new();
            for pair in prop_str.split(',') {
                if let Some((k, v)) = pair.split_once('=') {
                    props.insert(k, NbtTag::String(v.to_string()));
                }
            }
            c.insert("Properties", NbtTag::Compound(props));
        }
        c
    }
}

pub fn default_state_id(block_name: &str) -> Option<BlockStateId> {
    BLOCK_DEFAULT_STATE
        .binary_search_by_key(&block_name, |(n, _)| *n)
        .ok()
        .map(|i| BlockStateId(BLOCK_DEFAULT_STATE[i].1))
}

/// Resolve a vanilla NBT palette entry (`{Name: "minecraft:...", Properties: {...}}` or
/// 26.3 lowercase `{id: "minecraft:...", properties: {...}}`) to its exact 26.3 `BlockStateId`.
pub fn lookup_block_state_from_nbt(entry: &NbtCompound) -> Option<BlockStateId> {
    let name = entry.get_string("Name").or_else(|| entry.get_string("id"))?;
    let Some(props) = entry
        .get_compound("Properties")
        .or_else(|| entry.get_compound("properties"))
    else {
        return default_state_id(name);
    };
    if props.is_empty() {
        return default_state_id(name);
    }
    let mut sorted: BTreeMap<&str, &str> = BTreeMap::new();
    for (k, v) in props.iter() {
        if let mc_nbt::NbtTag::String(s) = v {
            sorted.insert(k.as_str(), s.as_str());
        }
    }
    let mut key = String::with_capacity(name.len() + 32);
    key.push_str(name);
    key.push('|');
    for (idx, (k, v)) in sorted.iter().enumerate() {
        if idx > 0 {
            key.push(',');
        }
        key.push_str(k);
        key.push('=');
        key.push_str(v);
    }
    BLOCK_STATE_BY_KEY
        .binary_search_by_key(&key.as_str(), |(k, _)| *k)
        .ok()
        .map(|i| BlockStateId(BLOCK_STATE_BY_KEY[i].1))
        .or_else(|| default_state_id(name))
}

pub fn static_registry_protocol_id(registry: &str, entry: &str) -> Option<i32> {
    let reg_idx = STATIC_REGISTRIES
        .binary_search_by_key(&registry, |(n, _)| *n)
        .ok()?;
    let entries = STATIC_REGISTRIES[reg_idx].1;
    let entry_idx = entries.binary_search_by_key(&entry, |(n, _)| *n).ok()?;
    Some(entries[entry_idx].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::NbtTag;

    #[test]
    fn generated_block_states_and_packets_sanity() {
        assert_eq!(PROTOCOL_VERSION, 777);
        assert_eq!(WORLD_VERSION, 5023);
        assert!(TOTAL_BLOCK_STATES > 20_000);
        assert_eq!(BlockStateId::AIR.block_name(), "minecraft:air");
        assert!(BlockStateId::AIR.is_air());
        assert_eq!(BlockStateId::STONE.block_name(), "minecraft:stone");
        assert_eq!(packet_ids::handshake::serverbound::INTENTION, 0);
        assert!(ENTITY_TYPE_PLAYER_ID > 0);

        let mut entry = NbtCompound::new();
        entry.insert("Name", NbtTag::String("minecraft:acacia_button".into()));
        let mut props = NbtCompound::new();
        props.insert("powered", NbtTag::String("true".into()));
        props.insert("face", NbtTag::String("floor".into()));
        props.insert("facing", NbtTag::String("north".into()));
        entry.insert("Properties", NbtTag::Compound(props));
        let id = lookup_block_state_from_nbt(&entry).unwrap();
        assert_eq!(id.0, 12514);
        assert_eq!(id.block_name(), "minecraft:acacia_button");

        // Verify Python-compiled 26.3 crafting, smelting, fuels, and foods!
        let oak_log = items::item_by_name("minecraft:oak_log").unwrap().id;
        let oak_planks = items::item_by_name("minecraft:oak_planks").unwrap().id;
        let craft_tbl = items::item_by_name("minecraft:crafting_table").unwrap().id;
        let cobble = items::item_by_name("minecraft:cobblestone").unwrap().id;
        let furnace = items::item_by_name("minecraft:furnace").unwrap().id;
        let raw_iron = items::item_by_name("minecraft:raw_iron").unwrap().id;
        let iron_ingot = items::item_by_name("minecraft:iron_ingot").unwrap().id;
        let coal = items::item_by_name("minecraft:coal").unwrap().id;
        let bread = items::item_by_name("minecraft:bread").unwrap().id;

        assert_eq!(recipes::match_crafting(2, 2, &[oak_log, 0, 0, 0]), Some((oak_planks, 4)));
        assert_eq!(
            recipes::match_crafting(2, 2, &[oak_planks, oak_planks, oak_planks, oak_planks]),
            Some((craft_tbl, 1))
        );
        assert_eq!(
            recipes::match_crafting(3, 3, &[cobble, cobble, cobble, cobble, 0, cobble, cobble, cobble, cobble]),
            Some((furnace, 1))
        );
        let smelt = recipes::match_cooking(recipes::COOK_SMELTING, raw_iron).unwrap();
        assert_eq!(smelt.result_item, iron_ingot);
        assert_eq!(smelt.cook_time, 200);
        assert_eq!(recipes::fuel_burn_time(coal), 1600);
        assert_eq!(recipes::food_info(bread).unwrap().nutrition, 5);

        // Verify Python-compiled 26.3 loot tables & combat/armor stats!
        let diamond = items::item_by_name("minecraft:diamond").unwrap().id;
        let porkchop = items::item_by_name("minecraft:porkchop").unwrap().id;
        let diamond_sword = items::item_by_name("minecraft:diamond_sword").unwrap().id;
        let diamond_chest = items::item_by_name("minecraft:diamond_chestplate").unwrap().id;
        assert_eq!(loot_and_tools::block_loot("minecraft:diamond_ore").unwrap().item_id, diamond);
        assert_eq!(loot_and_tools::block_loot("minecraft:stone").unwrap().item_id, cobble);
        assert_eq!(loot_and_tools::entity_loot("minecraft:pig")[0].item_id, porkchop);
        assert_eq!(loot_and_tools::item_combat_stats(diamond_sword).attack_damage, 6.0);
        assert_eq!(loot_and_tools::item_combat_stats(diamond_chest).armor, 8.0);
    }
}
