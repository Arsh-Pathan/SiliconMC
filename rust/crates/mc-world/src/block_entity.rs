use crate::ItemStack;
use mc_data::items::{item_by_id, item_by_name};
use mc_data::recipes::{
    COOK_BLASTING, COOK_SMELTING, COOK_SMOKING, block_entity_types, fuel_burn_time, match_cooking,
};
use mc_math::BlockPos;
use mc_nbt::{NbtCompound, NbtTag};

#[derive(Debug, Clone)]
pub enum BlockEntityKind {
    Chest {
        items: Vec<ItemStack>, // 27 slots
    },
    Barrel {
        items: Vec<ItemStack>, // 27 slots
    },
    Furnace {
        /// `COOK_SMELTING`, `COOK_BLASTING`, or `COOK_SMOKING`
        cook_kind: u8,
        /// 3 slots: 0=input, 1=fuel, 2=result
        items: Vec<ItemStack>,
        lit_time: i16,
        lit_duration: i16,
        cook_progress: i16,
        cook_total: i16,
    },
    Hopper {
        items: Vec<ItemStack>, // 5 slots
        transfer_cooldown: i32,
    },
    /// Lossless pass-through for any other vanilla block entity (signs, spawners, banners, etc.).
    Raw {
        id: String,
        nbt: NbtCompound,
    },
}

#[derive(Debug, Clone)]
pub struct BlockEntity {
    pub pos: BlockPos,
    pub kind: BlockEntityKind,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FurnaceTickResult {
    pub changed: bool,
    pub lit_changed_to: Option<bool>,
    pub slots_changed: bool,
    pub data_changed: bool,
}

impl BlockEntity {
    pub fn new_chest(pos: BlockPos) -> Self {
        Self {
            pos,
            kind: BlockEntityKind::Chest {
                items: vec![ItemStack::EMPTY; 27],
            },
        }
    }

    pub fn new_barrel(pos: BlockPos) -> Self {
        Self {
            pos,
            kind: BlockEntityKind::Barrel {
                items: vec![ItemStack::EMPTY; 27],
            },
        }
    }

    pub fn new_furnace(pos: BlockPos, cook_kind: u8) -> Self {
        Self {
            pos,
            kind: BlockEntityKind::Furnace {
                cook_kind,
                items: vec![ItemStack::EMPTY; 3],
                lit_time: 0,
                lit_duration: 0,
                cook_progress: 0,
                cook_total: if cook_kind == COOK_SMELTING { 200 } else { 100 },
            },
        }
    }

    pub fn new_hopper(pos: BlockPos) -> Self {
        Self {
            pos,
            kind: BlockEntityKind::Hopper {
                items: vec![ItemStack::EMPTY; 5],
                transfer_cooldown: 0,
            },
        }
    }

    pub fn type_id_str(&self) -> &str {
        match &self.kind {
            BlockEntityKind::Chest { .. } => "minecraft:chest",
            BlockEntityKind::Barrel { .. } => "minecraft:barrel",
            BlockEntityKind::Furnace { cook_kind, .. } => match *cook_kind {
                COOK_BLASTING => "minecraft:blast_furnace",
                COOK_SMOKING => "minecraft:smoker",
                _ => "minecraft:furnace",
            },
            BlockEntityKind::Hopper { .. } => "minecraft:hopper",
            BlockEntityKind::Raw { id, .. } => id.as_str(),
        }
    }

    pub fn protocol_type_id(&self) -> Option<i32> {
        match &self.kind {
            BlockEntityKind::Chest { .. } => Some(block_entity_types::CHEST),
            BlockEntityKind::Barrel { .. } => Some(block_entity_types::BARREL),
            BlockEntityKind::Furnace { cook_kind, .. } => Some(match *cook_kind {
                COOK_BLASTING => block_entity_types::BLAST_FURNACE,
                COOK_SMOKING => block_entity_types::SMOKER,
                _ => block_entity_types::FURNACE,
            }),
            BlockEntityKind::Hopper { .. } => Some(block_entity_types::HOPPER),
            BlockEntityKind::Raw { id, .. } => {
                mc_data::static_registry_protocol_id("minecraft:block_entity_type", id)
            }
        }
    }

    pub fn items_slice(&self) -> Option<&[ItemStack]> {
        match &self.kind {
            BlockEntityKind::Chest { items }
            | BlockEntityKind::Barrel { items }
            | BlockEntityKind::Furnace { items, .. }
            | BlockEntityKind::Hopper { items, .. } => Some(items),
            BlockEntityKind::Raw { .. } => None,
        }
    }

    pub fn items_slice_mut(&mut self) -> Option<&mut [ItemStack]> {
        match &mut self.kind {
            BlockEntityKind::Chest { items }
            | BlockEntityKind::Barrel { items }
            | BlockEntityKind::Furnace { items, .. }
            | BlockEntityKind::Hopper { items, .. } => Some(items),
            BlockEntityKind::Raw { .. } => None,
        }
    }

    /// Exact Minecraft `AbstractContainerMenu.getRedstoneSignalFromContainer` analog output (0..=15).
    pub fn comparator_signal(&self) -> u8 {
        let Some(slots) = self.items_slice() else {
            return 0;
        };
        if slots.is_empty() {
            return 0;
        }
        let mut fullness = 0.0_f32;
        let mut non_empty = 0usize;
        for s in slots {
            if !s.is_empty() {
                let max_st = s.max_stack_size().max(1) as f32;
                fullness += (s.count as f32) / max_st;
                non_empty += 1;
            }
        }
        if non_empty == 0 {
            return 0;
        }
        let ratio = fullness / (slots.len() as f32);
        ((ratio * 14.0).floor() as u8 + 1).min(15)
    }

    /// Try inserting 1 item of `stack` into this container (respecting furnace face rules:
    /// `from_dir == 1` (from above) -> input slot 0; side -> fuel slot 1).
    pub fn try_insert_one(&mut self, item: &ItemStack, from_top: bool) -> bool {
        if item.is_empty() {
            return false;
        }
        let allowed_slots: Vec<usize> = match &self.kind {
            BlockEntityKind::Furnace { .. } => {
                if from_top {
                    vec![0]
                } else if fuel_burn_time(item.item_id) > 0 {
                    vec![1]
                } else {
                    vec![]
                }
            }
            BlockEntityKind::Chest { items }
            | BlockEntityKind::Barrel { items }
            | BlockEntityKind::Hopper { items, .. } => (0..items.len()).collect(),
            BlockEntityKind::Raw { .. } => return false,
        };
        let Some(slots) = self.items_slice_mut() else {
            return false;
        };
        let max_st = item.max_stack_size();
        for &idx in &allowed_slots {
            if !slots[idx].is_empty()
                && slots[idx].item_id == item.item_id
                && !slots[idx].has_components()
                && !item.has_components()
                && slots[idx].count < max_st
            {
                slots[idx].count += 1;
                return true;
            }
        }
        for &idx in &allowed_slots {
            if slots[idx].is_empty() {
                let mut one = item.clone();
                one.count = 1;
                slots[idx] = one;
                return true;
            }
        }
        false
    }

    /// Try extracting 1 item from this container (from below: furnaces yield result slot 2).
    pub fn try_extract_one(&mut self) -> Option<ItemStack> {
        let allowed_slots: Vec<usize> = match &self.kind {
            BlockEntityKind::Furnace { .. } => vec![2],
            BlockEntityKind::Chest { items }
            | BlockEntityKind::Barrel { items }
            | BlockEntityKind::Hopper { items, .. } => (0..items.len()).collect(),
            BlockEntityKind::Raw { .. } => return None,
        };
        let slots = self.items_slice_mut()?;
        for idx in allowed_slots {
            if !slots[idx].is_empty() {
                let mut one = slots[idx].clone();
                one.count = 1;
                if slots[idx].count <= 1 {
                    slots[idx] = ItemStack::EMPTY;
                } else {
                    slots[idx].count -= 1;
                }
                return Some(one);
            }
        }
        None
    }

    /// Tick a furnace block entity using exact Minecraft 26.3 `AbstractFurnaceBlockEntity.serverTick` logic.
    pub fn tick_furnace(&mut self) -> FurnaceTickResult {
        let BlockEntityKind::Furnace {
            cook_kind,
            items,
            lit_time,
            lit_duration,
            cook_progress,
            cook_total,
        } = &mut self.kind
        else {
            return FurnaceTickResult::default();
        };

        let mut res = FurnaceTickResult::default();
        let prev_lit = *lit_time;
        let prev_dur = *lit_duration;
        let prev_prog = *cook_progress;
        let prev_total = *cook_total;

        let was_lit = *lit_time > 0;
        if *lit_time > 0 {
            *lit_time -= 1;
        }
        let mut is_lit = *lit_time > 0;

        let has_input = !items[0].is_empty();
        let has_fuel = !items[1].is_empty();

        if is_lit || (has_fuel && has_input) {
            if has_input {
                if let Some(recipe) = match_cooking(*cook_kind, items[0].item_id) {
                    let can_burn = if items[2].is_empty() {
                        true
                    } else if items[2].item_id == recipe.result_item && !items[2].has_components() {
                        let max_st = item_by_id(recipe.result_item)
                            .map(|i| i.max_stack_size)
                            .unwrap_or(64);
                        items[2].count.saturating_add(recipe.result_count) <= max_st
                    } else {
                        false
                    };

                    if can_burn {
                        *cook_total = recipe.cook_time as i16;
                        if !is_lit && has_fuel {
                            let base_burn = fuel_burn_time(items[1].item_id);
                            let burn = if *cook_kind == COOK_SMELTING {
                                base_burn
                            } else {
                                base_burn / 2
                            };
                            if burn > 0 {
                                *lit_time = burn.min(i16::MAX as u16) as i16;
                                *lit_duration = *lit_time;
                                is_lit = true;
                                res.changed = true;
                                res.slots_changed = true;

                                // Consume 1 fuel (lava_bucket -> empty bucket!)
                                let fuel_is_lava = item_by_id(items[1].item_id)
                                    .map(|i| i.name == "minecraft:lava_bucket")
                                    .unwrap_or(false);
                                if items[1].count <= 1 {
                                    if fuel_is_lava {
                                        items[1] = ItemStack::from_name("minecraft:bucket", 1)
                                            .unwrap_or(ItemStack::EMPTY);
                                    } else {
                                        items[1] = ItemStack::EMPTY;
                                    }
                                } else {
                                    items[1].count -= 1;
                                }
                            }
                        }

                        if is_lit {
                            *cook_progress += 1;
                            if *cook_progress >= *cook_total {
                                *cook_progress = 0;
                                // Produce result and consume 1 input
                                if items[2].is_empty() {
                                    items[2] =
                                        ItemStack::new(recipe.result_item, recipe.result_count);
                                } else {
                                    items[2].count =
                                        items[2].count.saturating_add(recipe.result_count);
                                }
                                if items[0].count <= 1 {
                                    items[0] = ItemStack::EMPTY;
                                } else {
                                    items[0].count -= 1;
                                }
                                res.changed = true;
                                res.slots_changed = true;
                            }
                        } else {
                            *cook_progress = 0;
                        }
                    } else {
                        *cook_progress = 0;
                    }
                } else {
                    *cook_progress = 0;
                }
            } else {
                *cook_progress = 0;
            }
        } else if *cook_progress > 0 {
            *cook_progress = (*cook_progress - 2).clamp(0, *cook_total);
        }

        if was_lit != is_lit {
            res.changed = true;
            res.lit_changed_to = Some(is_lit);
        }
        if *lit_time != prev_lit
            || *lit_duration != prev_dur
            || *cook_progress != prev_prog
            || *cook_total != prev_total
        {
            res.data_changed = true;
            res.changed = true;
        }
        res
    }

    /// Save in exact Minecraft 26.3 Anvil `block_entities` NBT format.
    pub fn save_to_nbt(&self) -> NbtCompound {
        if let BlockEntityKind::Raw { nbt, .. } = &self.kind {
            return nbt.clone();
        }
        let mut c = NbtCompound::new();
        c.insert("id", NbtTag::String(self.type_id_str().to_string()));
        c.insert("x", NbtTag::Int(self.pos.x));
        c.insert("y", NbtTag::Int(self.pos.y));
        c.insert("z", NbtTag::Int(self.pos.z));
        c.insert("keepPacked", NbtTag::Byte(0));

        match &self.kind {
            BlockEntityKind::Chest { items } | BlockEntityKind::Barrel { items } => {
                c.insert("Items", save_items_nbt(items));
            }
            BlockEntityKind::Furnace {
                items,
                lit_time,
                lit_duration,
                cook_progress,
                cook_total,
                ..
            } => {
                c.insert("cooking_time_spent", NbtTag::Int(*cook_progress as i32));
                c.insert("cooking_total_time", NbtTag::Int(*cook_total as i32));
                c.insert("lit_time_remaining", NbtTag::Int(*lit_time as i32));
                c.insert("lit_total_time", NbtTag::Int(*lit_duration as i32));
                c.insert("speed_multiplier", NbtTag::Float(1.0));
                c.insert("Items", save_items_nbt(items));
            }
            BlockEntityKind::Hopper {
                items,
                transfer_cooldown,
            } => {
                c.insert("TransferCooldown", NbtTag::Int(*transfer_cooldown));
                c.insert("Items", save_items_nbt(items));
            }
            BlockEntityKind::Raw { .. } => unreachable!(),
        }
        c
    }

    /// Parse a vanilla Minecraft 26.3 `block_entities` NBT entry.
    pub fn load_from_nbt(c: &NbtCompound) -> Option<Self> {
        let id = c.get_string("id")?.to_string();
        let x = c.get_int("x")?;
        let y = c.get_int("y")?;
        let z = c.get_int("z")?;
        let pos = BlockPos::new(x, y, z);

        let kind = match id.as_str() {
            "minecraft:chest" | "minecraft:trapped_chest" => BlockEntityKind::Chest {
                items: load_items_nbt(c, 27),
            },
            "minecraft:barrel" => BlockEntityKind::Barrel {
                items: load_items_nbt(c, 27),
            },
            "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => {
                let cook_kind = match id.as_str() {
                    "minecraft:blast_furnace" => COOK_BLASTING,
                    "minecraft:smoker" => COOK_SMOKING,
                    _ => COOK_SMELTING,
                };
                BlockEntityKind::Furnace {
                    cook_kind,
                    items: load_items_nbt(c, 3),
                    lit_time: c.get_int("lit_time_remaining").unwrap_or(0) as i16,
                    lit_duration: c.get_int("lit_total_time").unwrap_or(0) as i16,
                    cook_progress: c.get_int("cooking_time_spent").unwrap_or(0) as i16,
                    cook_total: c
                        .get_int("cooking_total_time")
                        .unwrap_or(if cook_kind == COOK_SMELTING { 200 } else { 100 })
                        as i16,
                }
            }
            "minecraft:hopper" => BlockEntityKind::Hopper {
                items: load_items_nbt(c, 5),
                transfer_cooldown: c.get_int("TransferCooldown").unwrap_or(0),
            },
            _ => BlockEntityKind::Raw { id, nbt: c.clone() },
        };
        Some(Self { pos, kind })
    }
}

fn save_items_nbt(items: &[ItemStack]) -> NbtTag {
    let mut list = Vec::new();
    for (slot, stack) in items.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let Some(info) = item_by_id(stack.item_id) else {
            continue;
        };
        let mut entry = NbtCompound::new();
        entry.insert("Slot", NbtTag::Byte(slot as i8));
        entry.insert("id", NbtTag::String(info.name.to_string()));
        entry.insert("count", NbtTag::Int(stack.count as i32));
        list.push(NbtTag::Compound(entry));
    }
    NbtTag::List(list)
}

fn load_items_nbt(c: &NbtCompound, size: usize) -> Vec<ItemStack> {
    let mut out = vec![ItemStack::EMPTY; size];
    if let Some(list) = c.get_list("Items") {
        for tag in list {
            let Some(ec) = tag.as_compound() else {
                continue;
            };
            let slot = ec.get_byte("Slot").unwrap_or(-1) as u8 as usize;
            if slot >= size {
                continue;
            }
            let Some(name) = ec.get_string("id") else {
                continue;
            };
            let count = ec
                .get_int("count")
                .or_else(|| ec.get_byte("Count").map(|b| b as i32))
                .unwrap_or(1)
                .clamp(0, 99) as u8;
            if let Some(info) = item_by_name(name) {
                out[slot] = ItemStack::new(info.id, count);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furnace_smelts_raw_iron_with_coal_in_200_ticks_and_roundtrips_nbt() {
        let mut be = BlockEntity::new_furnace(BlockPos::new(10, 64, -5), COOK_SMELTING);
        let raw_iron = ItemStack::from_name("minecraft:raw_iron", 2).unwrap();
        let coal = ItemStack::from_name("minecraft:coal", 1).unwrap();
        let iron_ingot_id = item_by_name("minecraft:iron_ingot").unwrap().id;

        {
            let slots = be.items_slice_mut().unwrap();
            slots[0] = raw_iron;
            slots[1] = coal;
        }

        // Tick 1: ignites coal (1600 ticks) and lights furnace
        let r1 = be.tick_furnace();
        assert_eq!(r1.lit_changed_to, Some(true));
        assert!(be.items_slice().unwrap()[1].is_empty());

        // Ticks 2..=200: completes first iron ingot on tick 200
        for _ in 2..=200 {
            be.tick_furnace();
        }
        let slots = be.items_slice().unwrap();
        assert_eq!(slots[0].count, 1);
        assert_eq!(slots[2].item_id, iron_ingot_id);
        assert_eq!(slots[2].count, 1);

        // NBT save & reload preserves items and remaining burn time (1600 - 199 = 1401)
        let nbt = be.save_to_nbt();
        let reloaded = BlockEntity::load_from_nbt(&nbt).unwrap();
        assert_eq!(reloaded.pos, BlockPos::new(10, 64, -5));
        assert_eq!(reloaded.items_slice().unwrap()[2].item_id, iron_ingot_id);
        if let BlockEntityKind::Furnace { lit_time, .. } = reloaded.kind {
            assert_eq!(lit_time, 1401);
        } else {
            panic!("Expected furnace");
        }
    }
}
