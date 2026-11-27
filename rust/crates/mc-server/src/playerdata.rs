use crate::world_loader::DimensionId;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mc_data::WORLD_VERSION;
use mc_data::items::{item_by_id, item_by_name};
use mc_math::Vec3;
use mc_nbt::{NbtCompound, NbtTag};
use mc_world::{ItemStack, PlayerInventory};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use uuid::Uuid;

/// Persistent player state stored in exact Minecraft 26.3 `world/playerdata/<uuid>.dat` gzipped NBT.
#[derive(Debug, Clone)]
pub struct SavedPlayerData {
    pub dimension: DimensionId,
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub gamemode: i32,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub food_exhaustion: f32,
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    pub selected_hotbar: u8,
    pub slots: Vec<ItemStack>,
}

fn stack_to_nbt(stack: &ItemStack) -> Option<NbtCompound> {
    if stack.is_empty() {
        return None;
    }
    let info = item_by_id(stack.item_id)?;
    let mut c = NbtCompound::new();
    c.insert("id", NbtTag::String(info.name.to_string()));
    c.insert("count", NbtTag::Int(stack.count as i32));
    Some(c)
}

fn stack_from_nbt(c: &NbtCompound) -> Option<ItemStack> {
    let name = c.get_string("id")?;
    let count = c
        .get_int("count")
        .or_else(|| c.get_byte("Count").map(|b| b as i32))
        .unwrap_or(1)
        .clamp(0, 99) as u8;
    if count == 0 {
        return None;
    }
    let info = item_by_name(name)?;
    Some(ItemStack::new(info.id, count))
}

/// Save a player's state to `world/playerdata/<uuid>.dat` in vanilla Minecraft 26.3 gzipped NBT format.
pub fn save_player_data(
    world_dir: &Path,
    uuid: Uuid,
    data: &SavedPlayerData,
) -> Result<(), String> {
    let dir = world_dir.join("playerdata");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{uuid}.dat"));
    let tmp_path = dir.join(format!("{uuid}.dat.tmp"));

    let mut root = NbtCompound::new();
    root.insert("DataVersion", NbtTag::Int(WORLD_VERSION));
    root.insert("Dimension", NbtTag::String(data.dimension.name().to_string()));
    root.insert(
        "Pos",
        NbtTag::List(vec![
            NbtTag::Double(data.pos.x),
            NbtTag::Double(data.pos.y),
            NbtTag::Double(data.pos.z),
        ]),
    );
    root.insert(
        "Rotation",
        NbtTag::List(vec![NbtTag::Float(data.yaw), NbtTag::Float(data.pitch)]),
    );
    root.insert("playerGameType", NbtTag::Int(data.gamemode));
    root.insert("Health", NbtTag::Float(data.health));
    root.insert("foodLevel", NbtTag::Int(data.food));
    root.insert("foodSaturationLevel", NbtTag::Float(data.saturation));
    root.insert("foodExhaustionLevel", NbtTag::Float(data.food_exhaustion));
    root.insert("XpLevel", NbtTag::Int(data.xp_level));
    root.insert("XpP", NbtTag::Float(data.xp_progress));
    root.insert("XpTotal", NbtTag::Int(data.xp_total));
    root.insert("SelectedItemSlot", NbtTag::Int(data.selected_hotbar as i32));

    // Vanilla Inventory list: slots 0..8 = hotbar (menu 36..44), 9..35 = main (menu 9..35)
    let mut inv_list = Vec::new();
    for h in 0..9usize {
        if let Some(mut c) = data.slots.get(36 + h).and_then(stack_to_nbt) {
            c.insert("Slot", NbtTag::Byte(h as i8));
            inv_list.push(NbtTag::Compound(c));
        }
    }
    for m in 9..36usize {
        if let Some(mut c) = data.slots.get(m).and_then(stack_to_nbt) {
            c.insert("Slot", NbtTag::Byte(m as i8));
            inv_list.push(NbtTag::Compound(c));
        }
    }
    root.insert("Inventory", NbtTag::List(inv_list));

    // Vanilla 26.3 EntityEquipment map (`equipment: {head, chest, legs, feet, offhand}`)
    let mut equip = NbtCompound::new();
    for (menu_idx, key) in [
        (5usize, "head"),
        (6, "chest"),
        (7, "legs"),
        (8, "feet"),
        (PlayerInventory::OFFHAND_SLOT, "offhand"),
    ] {
        if let Some(c) = data.slots.get(menu_idx).and_then(stack_to_nbt) {
            equip.insert(key, NbtTag::Compound(c));
        }
    }
    if !equip.is_empty() {
        root.insert("equipment", NbtTag::Compound(equip));
    }

    let raw_bytes = mc_nbt::write_file_bytes("", &root).map_err(|e| e.to_string())?;
    {
        let file = File::create(&tmp_path).map_err(|e| e.to_string())?;
        let mut gz = GzEncoder::new(file, Compression::default());
        gz.write_all(&raw_bytes).map_err(|e| e.to_string())?;
        gz.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;
    Ok(())
}

fn tag_as_f64(t: &NbtTag) -> Option<f64> {
    match *t {
        NbtTag::Double(v) => Some(v),
        NbtTag::Float(v) => Some(v as f64),
        _ => None,
    }
}

fn tag_as_f32(t: &NbtTag) -> Option<f32> {
    match *t {
        NbtTag::Float(v) => Some(v),
        NbtTag::Double(v) => Some(v as f32),
        _ => None,
    }
}

/// Load a player's saved state from `world/playerdata/<uuid>.dat` if present.
pub fn load_player_data(world_dir: &Path, uuid: Uuid) -> Option<SavedPlayerData> {
    let path = world_dir.join("playerdata").join(format!("{uuid}.dat"));
    if !path.is_file() {
        return None;
    }
    let file = File::open(&path).ok()?;
    let mut gz = GzDecoder::new(file);
    let mut raw = Vec::new();
    gz.read_to_end(&mut raw).ok()?;
    let (_, root) = mc_nbt::read_file_bytes(&raw).ok()?;

    let dimension = root
        .get_string("Dimension")
        .and_then(DimensionId::from_name)
        .unwrap_or(DimensionId::Overworld);

    let pos = root
        .get_list("Pos")
        .and_then(|l| {
            if l.len() >= 3 {
                Some(Vec3::new(
                    tag_as_f64(&l[0])?,
                    tag_as_f64(&l[1])?,
                    tag_as_f64(&l[2])?,
                ))
            } else {
                None
            }
        })
        .unwrap_or(Vec3::ZERO);

    let (yaw, pitch) = root
        .get_list("Rotation")
        .and_then(|l| {
            if l.len() >= 2 {
                Some((tag_as_f32(&l[0])?, tag_as_f32(&l[1])?))
            } else {
                None
            }
        })
        .unwrap_or((0.0, 0.0));

    let gamemode = root.get_int("playerGameType").unwrap_or(0).clamp(0, 3);
    let health = root.get_float("Health").unwrap_or(20.0).clamp(0.0, 20.0);
    let food = root.get_int("foodLevel").unwrap_or(20).clamp(0, 20);
    let saturation = root.get_float("foodSaturationLevel").unwrap_or(5.0).max(0.0);
    let food_exhaustion = root.get_float("foodExhaustionLevel").unwrap_or(0.0).max(0.0);
    let xp_level = root.get_int("XpLevel").unwrap_or(0).max(0);
    let xp_progress = root.get_float("XpP").unwrap_or(0.0).clamp(0.0, 1.0);
    let xp_total = root.get_int("XpTotal").unwrap_or(0).max(0);
    let selected_hotbar = root.get_int("SelectedItemSlot").unwrap_or(0).clamp(0, 8) as u8;

    let mut slots = PlayerInventory::new().slots;
    if let Some(inv) = root.get_list("Inventory") {
        for tag in inv {
            let Some(ec) = tag.as_compound() else {
                continue;
            };
            let Some(slot_byte) = ec.get_byte("Slot") else {
                continue;
            };
            let Some(stack) = stack_from_nbt(ec) else {
                continue;
            };
            let menu_idx = match slot_byte {
                0..=8 => Some(36 + slot_byte as usize),
                9..=35 => Some(slot_byte as usize),
                100 => Some(8),  // legacy feet
                101 => Some(7),  // legacy legs
                102 => Some(6),  // legacy chest
                103 => Some(5),  // legacy head
                -106 => Some(PlayerInventory::OFFHAND_SLOT),
                _ => None,
            };
            if let Some(mi) = menu_idx {
                slots[mi] = stack;
            }
        }
    }
    if let Some(equip) = root.get_compound("equipment") {
        for (menu_idx, key) in [
            (5usize, "head"),
            (6, "chest"),
            (7, "legs"),
            (8, "feet"),
            (PlayerInventory::OFFHAND_SLOT, "offhand"),
        ] {
            if let Some(stack) = equip.get_compound(key).and_then(stack_from_nbt) {
                slots[menu_idx] = stack;
            }
        }
    }

    Some(SavedPlayerData {
        dimension,
        pos,
        yaw,
        pitch,
        gamemode,
        health,
        food,
        saturation,
        food_exhaustion,
        xp_level,
        xp_progress,
        xp_total,
        selected_hotbar,
        slots,
    })
}

/// Vanilla Minecraft `Player.getXpNeededForNextLevel` formula.
#[inline]
pub fn xp_needed_for_next_level(level: i32) -> i32 {
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

/// Vanilla Minecraft `Player.giveExperiencePoints` state update.
pub fn add_experience_points(level: &mut i32, progress: &mut f32, total: &mut i32, points: i32) {
    let needed = xp_needed_for_next_level(*level) as f32;
    *progress += (points as f32) / needed;
    *total = total.saturating_add(points).max(0);

    while *progress < 0.0 {
        let rem = *progress * (xp_needed_for_next_level(*level) as f32);
        if *level > 0 {
            *level -= 1;
            *progress = 1.0 + rem / (xp_needed_for_next_level(*level) as f32);
        } else {
            *level = 0;
            *progress = 0.0;
        }
    }
    while *progress >= 1.0 {
        *progress = (*progress - 1.0) * (xp_needed_for_next_level(*level) as f32);
        *level = level.saturating_add(1);
        *progress /= xp_needed_for_next_level(*level) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playerdata_roundtrips_gzipped_nbt_inventory_and_xp() {
        let tmp = std::env::temp_dir().join("siliconmc_playerdata_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let uuid = Uuid::new_v4();

        let mut slots = PlayerInventory::new().slots;
        slots[36] = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        slots[6] = ItemStack::from_name("minecraft:iron_chestplate", 1).unwrap();
        slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::from_name("minecraft:shield", 1).unwrap();

        let mut lvl = 0;
        let mut prog = 0.0_f32;
        let mut tot = 0;
        add_experience_points(&mut lvl, &mut prog, &mut tot, 55);
        assert_eq!(lvl, 5); // 7 + 9 + 11 + 13 + 15 = 55 exact level 5!

        let saved = SavedPlayerData {
            dimension: DimensionId::Nether,
            pos: Vec3::new(12.5, 64.0, -33.25),
            yaw: 90.0,
            pitch: -15.0,
            gamemode: 0,
            health: 16.5,
            food: 18,
            saturation: 4.0,
            food_exhaustion: 1.25,
            xp_level: lvl,
            xp_progress: prog,
            xp_total: tot,
            selected_hotbar: 2,
            slots,
        };
        save_player_data(&tmp, uuid, &saved).unwrap();
        let loaded = load_player_data(&tmp, uuid).expect("playerdata file must load");
        assert_eq!(loaded.dimension, DimensionId::Nether);
        assert_eq!(loaded.pos, saved.pos);
        assert_eq!(loaded.health, 16.5);
        assert_eq!(loaded.xp_level, 5);
        assert_eq!(loaded.xp_total, 55);
        assert_eq!(loaded.selected_hotbar, 2);
        assert_eq!(loaded.slots[36].item_id, saved.slots[36].item_id);
        assert_eq!(loaded.slots[6].item_id, saved.slots[6].item_id);
        assert_eq!(
            loaded.slots[PlayerInventory::OFFHAND_SLOT].item_id,
            saved.slots[PlayerInventory::OFFHAND_SLOT].item_id
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
