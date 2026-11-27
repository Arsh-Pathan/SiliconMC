use crate::ItemStack;
use mc_data::gameplay_tables::{
    ENCHANTMENTS, EnchantmentSpec, data_components, enchantment_by_id, enchantment_index,
    enchantment_is_primary_for_item, enchantment_supports_item, enchantments_compatible,
    is_valid_repair_item, item_enchantability, match_brewing, match_smithing,
    potion_name_by_protocol_id, potion_protocol_id,
};
use mc_data::items::{item_by_id, item_by_name};
use mc_data::loot_and_tools::item_combat_stats;
use mc_data::recipes::{STONECUTTING_RECIPES, StonecuttingRecipe};
use mc_math::{LegacyRandomSource, RandomSource};
use mc_nbt::{NbtCompound, NbtTag};

// ============================================================================
// Wire-exact ItemStack component helpers (Minecraft 26.3 DataComponentPatch)
// ============================================================================

fn encode_varint(mut v: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
    out
}

fn decode_varint(bytes: &[u8]) -> Option<(i32, usize)> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (idx, &b) in bytes.iter().enumerate() {
        result |= ((b & 0x7F) as u32) << shift;
        if (b & 0x80) == 0 {
            return Some((result as i32, idx + 1));
        }
        shift += 7;
        if shift >= 35 {
            return None;
        }
    }
    None
}

fn get_component_bytes(stack: &ItemStack, type_id: i32) -> Option<&[u8]> {
    stack
        .added_components
        .iter()
        .find(|(id, _)| *id == type_id)
        .map(|(_, b)| b.as_slice())
}

fn set_component_bytes(stack: &mut ItemStack, type_id: i32, data: Vec<u8>) {
    stack.removed_components.retain(|&id| id != type_id);
    if let Some((_, existing)) = stack
        .added_components
        .iter_mut()
        .find(|(id, _)| *id == type_id)
    {
        *existing = data;
    } else {
        stack.added_components.push((type_id, data));
    }
}

fn remove_component(stack: &mut ItemStack, type_id: i32) {
    stack.added_components.retain(|(id, _)| *id != type_id);
}

/// Read the `minecraft:damage` component value from an `ItemStack` (0 if undamaged).
pub fn item_damage(stack: &ItemStack) -> i32 {
    get_component_bytes(stack, data_components::DAMAGE)
        .and_then(|b| decode_varint(b).map(|(v, _)| v.max(0)))
        .unwrap_or(0)
}

/// Write the `minecraft:damage` component value on an `ItemStack` (removes component if 0).
pub fn set_item_damage(stack: &mut ItemStack, damage: i32) {
    if damage <= 0 {
        remove_component(stack, data_components::DAMAGE);
    } else {
        set_component_bytes(stack, data_components::DAMAGE, encode_varint(damage as u32));
    }
}

/// Read the `minecraft:repair_cost` (anvil prior-work penalty) from an `ItemStack`.
pub fn item_repair_cost(stack: &ItemStack) -> i32 {
    get_component_bytes(stack, data_components::REPAIR_COST)
        .and_then(|b| decode_varint(b).map(|(v, _)| v.max(0)))
        .unwrap_or(0)
}

/// Write the `minecraft:repair_cost` component on an `ItemStack`.
pub fn set_item_repair_cost(stack: &mut ItemStack, cost: i32) {
    if cost <= 0 {
        remove_component(stack, data_components::REPAIR_COST);
    } else {
        set_component_bytes(
            stack,
            data_components::REPAIR_COST,
            encode_varint(cost as u32),
        );
    }
}

/// Read a literal `minecraft:custom_name` text component (network NBT `TAG_String`).
pub fn item_custom_name(stack: &ItemStack) -> Option<String> {
    let bytes = get_component_bytes(stack, data_components::CUSTOM_NAME)?;
    if bytes.len() < 3 || bytes[0] != 8 {
        return None;
    }
    let len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
    if bytes.len() < 3 + len {
        return None;
    }
    String::from_utf8(bytes[3..3 + len].to_vec()).ok()
}

/// Write or clear `minecraft:custom_name` as a Minecraft 26.3 network NBT `TAG_String`.
pub fn set_item_custom_name(stack: &mut ItemStack, name: Option<&str>) {
    match name {
        Some(text) if !text.is_empty() => {
            let raw = text.as_bytes();
            let len = raw.len().min(u16::MAX as usize);
            let mut buf = Vec::with_capacity(3 + len);
            buf.push(8); // TAG_String
            buf.extend_from_slice(&(len as u16).to_be_bytes());
            buf.extend_from_slice(&raw[..len]);
            set_component_bytes(stack, data_components::CUSTOM_NAME, buf);
        }
        _ => remove_component(stack, data_components::CUSTOM_NAME),
    }
}

/// Read the potion ID (e.g., `"minecraft:water"`, `"minecraft:awkward"`) from `minecraft:potion_contents`.
pub fn item_potion_id(stack: &ItemStack) -> Option<&'static str> {
    let bytes = get_component_bytes(stack, data_components::POTION_CONTENTS)?;
    if bytes.is_empty() || bytes[0] != 1 {
        return None;
    }
    let (proto_id, _) = decode_varint(&bytes[1..])?;
    potion_name_by_protocol_id(proto_id)
}

/// Encode `minecraft:potion_contents` (`PotionContents.STREAM_CODEC`) onto an `ItemStack`.
pub fn set_item_potion(stack: &mut ItemStack, potion_id: &str) -> bool {
    let Some(proto_id) = potion_protocol_id(potion_id) else {
        return false;
    };
    let mut buf = Vec::with_capacity(8);
    buf.push(1); // Optional<Holder<Potion>> = present
    buf.extend_from_slice(&encode_varint(proto_id as u32));
    buf.push(0); // Optional<Integer> customColor = empty
    buf.push(0); // List<MobEffectInstance> customEffects = len 0
    buf.push(0); // Optional<String> customName = empty
    set_component_bytes(stack, data_components::POTION_CONTENTS, buf);
    true
}

/// Convenience builder for a Minecraft 26.3 potion `ItemStack` (e.g., `"minecraft:potion"`, `"minecraft:water"`).
pub fn make_potion_stack(item_name: &str, potion_id: &str) -> Option<ItemStack> {
    let mut stack = ItemStack::from_name(item_name, 1)?;
    if !set_item_potion(&mut stack, potion_id) {
        return None;
    }
    Some(stack)
}

// ============================================================================
// 1. Enchanting Table (vanilla EnchantmentMenu + EnchantmentHelper)
// ============================================================================

/// One of the 3 enchanting table slots (`0` = top, `1` = middle, `2` = bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentOffer {
    /// Required player XP levels (0 if slot is unavailable/empty).
    pub xp_cost: i32,
    /// Protocol/table index of the previewed clue enchantment (`-1` if none).
    pub clue_enchantment_id: i32,
    /// Canonical Minecraft 26.3 ID of the previewed clue enchantment (if any).
    pub clue_enchantment: Option<&'static str>,
    /// Previewed clue enchantment level (`-1` if none).
    pub clue_level: i32,
}

impl Default for EnchantmentOffer {
    fn default() -> Self {
        Self {
            xp_cost: 0,
            clue_enchantment_id: -1,
            clue_enchantment: None,
            clue_level: -1,
        }
    }
}

fn weighted_pick_enchantment<'a>(
    rng: &mut LegacyRandomSource,
    candidates: &[(&'static EnchantmentSpec, u8)],
) -> Option<(&'static EnchantmentSpec, u8)> {
    let total_weight: i32 = candidates.iter().map(|(s, _)| s.weight as i32).sum();
    if total_weight <= 0 {
        return None;
    }
    let mut pick = rng.next_int_bounded(total_weight);
    for &(spec, level) in candidates {
        pick -= spec.weight as i32;
        if pick < 0 {
            return Some((spec, level));
        }
    }
    None
}

fn select_enchantments_with_rng(
    rng: &mut LegacyRandomSource,
    item_id: u16,
    xp_cost: i32,
) -> Vec<(&'static str, u8)> {
    let enchantability = item_enchantability(item_id) as i32;
    if enchantability <= 0 || xp_cost <= 0 {
        return Vec::new();
    }

    // Exact EnchantmentHelper.selectEnchantment power perturbation
    let mut power = xp_cost
        + 1
        + rng.next_int_bounded(enchantability / 4 + 1)
        + rng.next_int_bounded(enchantability / 4 + 1);
    let random_span = (rng.next_float() + rng.next_float() - 1.0_f32) * 0.15_f32;
    power = ((power as f32 + power as f32 * random_span).round() as i32).max(1);

    let is_book = item_by_id(item_id)
        .map(|i| i.name == "minecraft:book")
        .unwrap_or(false);

    // Exact EnchantmentHelper.getAvailableEnchantmentResults using #minecraft:in_enchanting_table (!is_treasure)
    let mut available: Vec<(&'static EnchantmentSpec, u8)> = Vec::new();
    for spec in ENCHANTMENTS {
        if spec.is_treasure {
            continue;
        }
        if !is_book && !enchantment_is_primary_for_item(spec.id, item_id) {
            continue;
        }
        for level in (1..=spec.max_level).rev() {
            if power >= spec.min_cost(level) && power <= spec.max_cost(level) {
                available.push((spec, level));
                break;
            }
        }
    }

    let mut chosen: Vec<(&'static EnchantmentSpec, u8)> = Vec::new();
    if !available.is_empty() {
        if let Some(first) = weighted_pick_enchantment(rng, &available) {
            chosen.push(first);
        }

        while rng.next_int_bounded(50) <= power {
            if let Some(last) = chosen.last() {
                let last_id = last.0.id;
                available.retain(|(spec, _)| enchantments_compatible(last_id, spec.id));
            }
            if available.is_empty() {
                break;
            }
            if let Some(next) = weighted_pick_enchantment(rng, &available) {
                chosen.push(next);
            }
            power /= 2;
        }
    }

    // Exact EnchantmentMenu.getEnchantmentList book penalty: drops 1 random enchantment if > 1 rolled
    if is_book && chosen.len() > 1 {
        let drop_idx = rng.next_int_bounded(chosen.len() as i32) as usize;
        chosen.remove(drop_idx);
    }

    chosen.into_iter().map(|(s, lvl)| (s.id, lvl)).collect()
}

/// Compute the 3 Enchanting Table offers matching vanilla Minecraft 26.3 `EnchantmentMenu.slotsChanged`
/// and `EnchantmentHelper.getEnchantmentCost`.
pub fn compute_enchanting_offers(
    enchantment_seed: i32,
    item_id: u16,
    bookshelves: u8,
) -> [EnchantmentOffer; 3] {
    let mut offers = [EnchantmentOffer::default(); 3];
    if item_enchantability(item_id) == 0 {
        return offers;
    }

    let bookcases = (bookshelves as i32).min(15);
    let mut rng = LegacyRandomSource::new(enchantment_seed as i64);

    for (slot, offer) in offers.iter_mut().enumerate() {
        let selected =
            rng.next_int_bounded(8) + 1 + (bookcases >> 1) + rng.next_int_bounded(bookcases + 1);
        let mut cost = match slot {
            0 => (selected / 3).max(1),
            1 => selected * 2 / 3 + 1,
            _ => selected.max(bookcases * 2),
        };
        if cost < slot as i32 + 1 {
            cost = 0;
        }
        offer.xp_cost = cost;
    }

    for (slot, offer) in offers.iter_mut().enumerate() {
        if offer.xp_cost > 0 {
            rng.set_seed(enchantment_seed.wrapping_add(slot as i32) as i64);
            let rolled = select_enchantments_with_rng(&mut rng, item_id, offer.xp_cost);
            if !rolled.is_empty() {
                let clue_idx = rng.next_int_bounded(rolled.len() as i32) as usize;
                let (ench_id, level) = rolled[clue_idx];
                offer.clue_enchantment_id = enchantment_index(ench_id).map(|i| i as i32).unwrap_or(-1);
                offer.clue_enchantment = Some(ench_id);
                offer.clue_level = level as i32;
            }
        }
    }

    offers
}

/// Roll the exact list of `(enchantment_id, level)` pairs produced when selecting offer `slot` (`0..=2`)
/// at an Enchanting Table with `xp_cost` (matching vanilla `EnchantmentMenu.getEnchantmentList`).
pub fn pick_enchantments_for_offer(
    enchantment_seed: i32,
    item_id: u16,
    slot: u8,
    xp_cost: i32,
) -> Vec<(&'static str, u8)> {
    if slot >= 3 || xp_cost <= 0 {
        return Vec::new();
    }
    let mut rng = LegacyRandomSource::new(enchantment_seed.wrapping_add(slot as i32) as i64);
    select_enchantments_with_rng(&mut rng, item_id, xp_cost)
}

// ============================================================================
// 2. Anvil (vanilla AnvilMenu.createResult)
// ============================================================================

/// Output produced by an Anvil combine, repair, or rename operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnvilResult {
    pub output: ItemStack,
    pub xp_cost: i32,
    pub repair_items_used: u8,
}

/// Vanilla `AnvilMenu.calculateIncreasedRepairCost`: `min(base * 2 + 1, i32::MAX)`.
#[inline]
pub fn anvil_next_repair_cost(base_cost: i32) -> i32 {
    ((base_cost as i64).saturating_mul(2).saturating_add(1)).min(i32::MAX as i64) as i32
}

fn find_enchant_entry(list: &[(String, u8)], query: &str) -> Option<usize> {
    if let Some(q_idx) = enchantment_index(query) {
        list.iter()
            .position(|(id, _)| enchantment_index(id) == Some(q_idx))
    } else {
        list.iter().position(|(id, _)| id == query)
    }
}

/// Compute the vanilla Minecraft 26.3 Anvil output, XP level cost, repair material count,
/// and resulting combined enchantment list.
///
/// Implements:
/// - Material repair (up to 25% durability per material, +1 XP level per item consumed)
/// - Same-item durability combining (remaining1 + remaining2 + 12% bonus, +2 XP levels if repaired)
/// - Enchantment combining from enchanted books or matching items (equal levels -> `level + 1` capped at `max_level`,
///   half anvil cost for enchanted books, `exclusive_set` conflict checks with +1 incompatibility penalty)
/// - Renaming (+1 XP level, max 50 chars, capped at 39 levels when rename-only)
/// - Prior work penalty (`minecraft:repair_cost`) accumulation and 40-level survival limit.
pub fn compute_anvil_output(
    left: &ItemStack,
    right: &ItemStack,
    rename: Option<&str>,
    left_enchants: &[(String, u8)],
    right_enchants: &[(String, u8)],
) -> Option<(AnvilResult, Vec<(String, u8)>)> {
    if left.is_empty() {
        return None;
    }

    let mut output = left.clone();
    let mut combined: Vec<(String, u8)> = left_enchants.to_vec();

    let left_tax = item_repair_cost(left) as i64;
    let right_tax = if right.is_empty() {
        0
    } else {
        item_repair_cost(right) as i64
    };
    let tax = left_tax + right_tax;

    let mut price: i32 = 0;
    let mut naming_cost: i32 = 0;
    let mut repair_items_used: u8 = 0;

    let left_max_damage = item_combat_stats(left.item_id).max_damage as i32;
    let left_is_damageable = left_max_damage > 0;
    let left_is_book = item_by_id(left.item_id)
        .map(|i| i.name == "minecraft:enchanted_book")
        .unwrap_or(false);

    if !right.is_empty() {
        let right_is_book = item_by_id(right.item_id)
            .map(|i| i.name == "minecraft:enchanted_book")
            .unwrap_or(false);
        let using_book = right_is_book && !right_enchants.is_empty();

        if left_is_damageable && is_valid_repair_item(left.item_id, right.item_id) {
            let mut dmg = item_damage(&output);
            let mut repair_amount = dmg.min(left_max_damage / 4);
            if repair_amount <= 0 {
                return None;
            }
            let mut used = 0u8;
            while repair_amount > 0 && used < right.count {
                dmg -= repair_amount;
                price += 1;
                used += 1;
                repair_amount = dmg.min(left_max_damage / 4);
            }
            set_item_damage(&mut output, dmg);
            repair_items_used = used;
        } else {
            if !using_book && (left.item_id != right.item_id || !left_is_damageable) {
                return None;
            }

            if left_is_damageable && !using_book {
                let left_dmg = item_damage(left);
                let right_dmg = item_damage(right);
                let rem1 = (left_max_damage - left_dmg).max(0);
                let rem2 = (left_max_damage - right_dmg).max(0);
                let additional = rem2 + left_max_damage * 12 / 100;
                let result_dmg = (left_max_damage - (rem1 + additional)).max(0);
                if result_dmg < left_dmg {
                    set_item_damage(&mut output, result_dmg);
                    price += 2;
                }
            }

            let mut any_compatible = false;
            let mut any_incompatible = false;

            for (r_id, r_lvl) in right_enchants {
                let existing_pos = find_enchant_entry(&combined, r_id);
                let current_lvl = existing_pos.map(|p| combined[p].1).unwrap_or(0);
                let mut new_lvl = if current_lvl == *r_lvl {
                    current_lvl.saturating_add(1)
                } else {
                    current_lvl.max(*r_lvl)
                };

                let spec = enchantment_by_id(r_id);
                let mut compatible = left_is_book || enchantment_supports_item(r_id, left.item_id);

                for (other_idx, (other_id, _)) in combined.iter().enumerate() {
                    if Some(other_idx) != existing_pos && !enchantments_compatible(r_id, other_id) {
                        compatible = false;
                        price += 1;
                    }
                }

                if !compatible {
                    any_incompatible = true;
                } else {
                    any_compatible = true;
                    if let Some(s) = spec {
                        if new_lvl > s.max_level {
                            new_lvl = s.max_level;
                        }
                    }

                    let canonical_key = spec.map(|s| s.id.to_string()).unwrap_or_else(|| r_id.clone());
                    if let Some(pos) = existing_pos {
                        combined[pos].1 = new_lvl;
                    } else {
                        combined.push((canonical_key, new_lvl));
                    }

                    let mut fee = spec.map(|s| s.anvil_cost as i32).unwrap_or(1);
                    if using_book {
                        fee = (fee / 2).max(1);
                    }
                    price += fee * (new_lvl as i32);
                    if left.count > 1 {
                        price = 40;
                    }
                }
            }

            if any_incompatible && !any_compatible {
                return None;
            }
        }
    }

    if let Some(raw_name) = rename {
        if raw_name.chars().count() > 50 {
            return None;
        }
        let existing_name = item_custom_name(left);
        if !raw_name.trim().is_empty() {
            if existing_name.as_deref() != Some(raw_name) {
                naming_cost = 1;
                price += 1;
                set_item_custom_name(&mut output, Some(raw_name));
            }
        } else if existing_name.is_some() {
            naming_cost = 1;
            price += 1;
            set_item_custom_name(&mut output, None);
        }
    }

    if price <= 0 {
        return None;
    }

    let mut final_cost = (tax + price as i64).clamp(0, i32::MAX as i64) as i32;
    let only_renaming = naming_cost == price && naming_cost > 0;
    if only_renaming && final_cost >= 40 {
        final_cost = 39;
    }
    if final_cost >= 40 {
        return None;
    }

    let mut base_repair_cost = item_repair_cost(left);
    if !right.is_empty() {
        base_repair_cost = base_repair_cost.max(item_repair_cost(right));
    }
    if !only_renaming {
        base_repair_cost = anvil_next_repair_cost(base_repair_cost);
    }
    set_item_repair_cost(&mut output, base_repair_cost);

    Some((
        AnvilResult {
            output,
            xp_cost: final_cost,
            repair_items_used,
        },
        combined,
    ))
}

// ============================================================================
// 3. Brewing Stand (vanilla BrewingStandBlockEntity)
// ============================================================================

pub const BREWING_DURATION_TICKS: i16 = 400;
pub const BLAZE_POWDER_FUEL_CHARGES: u8 = 20;

/// Full server-side state for a Minecraft 26.3 Brewing Stand (`BrewingStandBlockEntity`).
///
/// Slot layout:
/// - `0..=2`: potion bottle slots
/// - `3`: ingredient / reagent slot
/// - `4`: fuel slot (`minecraft:blaze_powder`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewingStandState {
    pub slots: [ItemStack; 5],
    pub brew_time: i16,
    pub fuel: u8,
    pub active_ingredient: u16,
    pub bottle_potions: [Option<String>; 3],
}

impl Default for BrewingStandState {
    fn default() -> Self {
        Self::new()
    }
}

impl BrewingStandState {
    pub fn new() -> Self {
        Self {
            slots: [
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
            ],
            brew_time: 0,
            fuel: 0,
            active_ingredient: 0,
            bottle_potions: [None, None, None],
        }
    }

    /// Resolve the active potion type for bottle slot `idx` (`0..=2`), reading the item's
    /// `minecraft:potion_contents` component first and falling back to `bottle_potions[idx]`.
    pub fn bottle_potion(&self, idx: usize) -> Option<&str> {
        if idx >= 3 || self.slots[idx].is_empty() {
            return None;
        }
        item_potion_id(&self.slots[idx]).or(self.bottle_potions[idx].as_deref())
    }

    /// Place a bottle into slot `idx` (`0..=2`), syncing both `slots[idx]` and `bottle_potions[idx]`.
    pub fn set_bottle(&mut self, idx: usize, mut stack: ItemStack, potion_id: Option<&str>) {
        if idx >= 3 {
            return;
        }
        if stack.is_empty() {
            self.slots[idx] = ItemStack::EMPTY;
            self.bottle_potions[idx] = None;
            return;
        }
        if let Some(pid) = potion_id {
            set_item_potion(&mut stack, pid);
            self.bottle_potions[idx] = Some(pid.to_string());
        } else {
            self.bottle_potions[idx] = item_potion_id(&stack).map(|s| s.to_string());
        }
        self.slots[idx] = stack;
    }

    /// Returns `true` if the current reagent can brew at least one bottle in slots `0..=2`.
    pub fn is_brewable(&self) -> bool {
        let ingredient = &self.slots[3];
        if ingredient.is_empty() {
            return false;
        }
        for i in 0..3 {
            if !self.slots[i].is_empty() {
                if let Some(pot) = self.bottle_potion(i) {
                    if match_brewing(self.slots[i].item_id, pot, ingredient.item_id).is_some() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Returns the 3-bit mask of occupied bottle slots (for `BrewingStandBlock.HAS_BOTTLE`).
    pub fn bottle_mask(&self) -> [bool; 3] {
        [
            !self.slots[0].is_empty(),
            !self.slots[1].is_empty(),
            !self.slots[2].is_empty(),
        ]
    }

    /// Advance the brewing stand by 1 server tick (exact `BrewingStandBlockEntity.serverTick`).
    ///
    /// - Consumes 1 `minecraft:blaze_powder` from slot 4 when `fuel == 0`, refilling `fuel = 20`.
    /// - Starts a 400-tick brew cycle when brewable and `fuel > 0` (consuming 1 fuel charge).
    /// - Transforms all matching bottles in slots `0..=2` and consumes 1 reagent when `brew_time` hits 0.
    /// - Returns `true` if any state changed.
    pub fn tick_brewing(&mut self) -> bool {
        let mut changed = false;

        let blaze_powder_id = item_by_name("minecraft:blaze_powder")
            .map(|i| i.id)
            .unwrap_or(0);
        if self.fuel == 0
            && blaze_powder_id != 0
            && !self.slots[4].is_empty()
            && self.slots[4].item_id == blaze_powder_id
        {
            self.fuel = BLAZE_POWDER_FUEL_CHARGES;
            if self.slots[4].count <= 1 {
                self.slots[4] = ItemStack::EMPTY;
            } else {
                self.slots[4].count -= 1;
            }
            changed = true;
        }

        let brewable = self.is_brewable();
        let ingredient_id = if self.slots[3].is_empty() {
            0
        } else {
            self.slots[3].item_id
        };

        if self.brew_time > 0 {
            self.brew_time -= 1;
            changed = true;
            if self.brew_time == 0 && brewable {
                self.do_brew();
            } else if !brewable || ingredient_id != self.active_ingredient {
                self.brew_time = 0;
                self.active_ingredient = 0;
            }
        } else if brewable && self.fuel > 0 {
            self.fuel -= 1;
            self.brew_time = BREWING_DURATION_TICKS;
            self.active_ingredient = ingredient_id;
            changed = true;
        }

        changed
    }

    fn do_brew(&mut self) {
        let ingredient_id = self.slots[3].item_id;
        for i in 0..3 {
            if self.slots[i].is_empty() {
                continue;
            }
            let Some(in_pot) = self.bottle_potion(i).map(|s| s.to_string()) else {
                continue;
            };
            if let Some(recipe) = match_brewing(self.slots[i].item_id, &in_pot, ingredient_id) {
                let mut next = ItemStack::new(recipe.result_item, 1);
                set_item_potion(&mut next, recipe.output_potion);
                self.slots[i] = next;
                self.bottle_potions[i] = Some(recipe.output_potion.to_string());
            }
        }
        if self.slots[3].count <= 1 {
            self.slots[3] = ItemStack::EMPTY;
        } else {
            self.slots[3].count -= 1;
        }
        self.active_ingredient = 0;
    }

    /// Serialize this brewing stand in Minecraft 26.3 `BrewingStandBlockEntity` NBT format.
    pub fn save_to_nbt(&self) -> NbtCompound {
        let mut c = NbtCompound::new();
        c.insert("id", NbtTag::String("minecraft:brewing_stand".to_string()));
        c.insert("BrewTime", NbtTag::Int(self.brew_time as i32));
        c.insert("total_brew_time", NbtTag::Int(BREWING_DURATION_TICKS as i32));
        c.insert("Fuel", NbtTag::Int(self.fuel as i32));
        c.insert("total_fuel", NbtTag::Int(BLAZE_POWDER_FUEL_CHARGES as i32));
        c.insert("speed_multiplier", NbtTag::Float(1.0));

        let mut list = Vec::new();
        for (slot, stack) in self.slots.iter().enumerate() {
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
            if slot < 3 {
                if let Some(pot) = self.bottle_potion(slot) {
                    let mut pot_comp = NbtCompound::new();
                    pot_comp.insert("potion", NbtTag::String(pot.to_string()));
                    let mut comps = NbtCompound::new();
                    comps.insert("minecraft:potion_contents", NbtTag::Compound(pot_comp));
                    entry.insert("components", NbtTag::Compound(comps));
                }
            }
            list.push(NbtTag::Compound(entry));
        }
        c.insert("Items", NbtTag::List(list));
        c
    }

    /// Deserialize a `BrewingStandState` from Minecraft 26.3 `BrewingStandBlockEntity` NBT.
    pub fn load_from_nbt(nbt: &NbtCompound) -> Self {
        let mut state = Self::new();
        state.brew_time = nbt
            .get_int("BrewTime")
            .or_else(|| nbt.get_short("BrewTime").map(|s| s as i32))
            .unwrap_or(0)
            .clamp(0, i16::MAX as i32) as i16;
        state.fuel = nbt
            .get_int("Fuel")
            .or_else(|| nbt.get_byte("Fuel").map(|b| b as i32))
            .unwrap_or(0)
            .clamp(0, u8::MAX as i32) as u8;

        if let Some(list) = nbt.get_list("Items") {
            for tag in list {
                let Some(ec) = tag.as_compound() else {
                    continue;
                };
                let slot = ec.get_byte("Slot").unwrap_or(-1) as u8 as usize;
                if slot >= 5 {
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
                let Some(info) = item_by_name(name) else {
                    continue;
                };
                let stack = ItemStack::new(info.id, count);
                if slot < 3 {
                    let potion = ec
                        .get_compound("components")
                        .and_then(|c| c.get_compound("minecraft:potion_contents"))
                        .and_then(|pc| pc.get_string("potion"));
                    state.set_bottle(slot, stack, potion);
                } else {
                    state.slots[slot] = stack;
                }
            }
        }

        if state.brew_time > 0 && !state.slots[3].is_empty() {
            state.active_ingredient = state.slots[3].item_id;
        }
        state
    }
}

// ============================================================================
// 4. Smithing Table (vanilla SmithingTransformRecipe & SmithingTrimRecipe)
// ============================================================================

/// Compute the Minecraft 26.3 Smithing Table output for `(template, base, addition)`.
///
/// Preserves all components on `base` (enchantments, durability damage, custom name, repair cost).
pub fn compute_smithing_output(
    template: &ItemStack,
    base: &ItemStack,
    addition: &ItemStack,
) -> Option<ItemStack> {
    if template.is_empty() || base.is_empty() || addition.is_empty() {
        return None;
    }
    let result_id = match_smithing(template.item_id, base.item_id, addition.item_id)?;
    let mut out = base.clone();
    out.item_id = result_id;
    out.count = 1;
    Some(out)
}

/// Execute a smithing craft, consuming 1 item from each input slot on success.
pub fn craft_smithing(
    template: &mut ItemStack,
    base: &mut ItemStack,
    addition: &mut ItemStack,
) -> Option<ItemStack> {
    let out = compute_smithing_output(template, base, addition)?;
    for slot in [template, base, addition] {
        if slot.count <= 1 {
            *slot = ItemStack::EMPTY;
        } else {
            slot.count -= 1;
        }
    }
    Some(out)
}

// ============================================================================
// 5. Stonecutter (backed by mc_data::recipes::STONECUTTING_RECIPES)
// ============================================================================

/// Return all Minecraft 26.3 `StonecuttingRecipe` entries whose ingredient matches `input_item_id`.
pub fn stonecutting_for_input(input_item_id: u16) -> Vec<&'static StonecuttingRecipe> {
    if input_item_id == 0 {
        return Vec::new();
    }
    STONECUTTING_RECIPES
        .iter()
        .filter(|r| r.ingredient.matches(input_item_id))
        .collect()
}

/// List all valid Stonecutter output `ItemStack`s for `input`.
pub fn stonecutter_outputs(input: &ItemStack) -> Vec<ItemStack> {
    if input.is_empty() {
        return Vec::new();
    }
    stonecutting_for_input(input.item_id)
        .into_iter()
        .map(|r| ItemStack::new(r.result_item, r.result_count))
        .collect()
}

/// Craft the `recipe_idx`-th matching Stonecutter output, consuming 1 item from `input`.
pub fn craft_stonecutter(input: &mut ItemStack, recipe_idx: usize) -> Option<ItemStack> {
    if input.is_empty() {
        return None;
    }
    let matches = stonecutting_for_input(input.item_id);
    let recipe = *matches.get(recipe_idx)?;
    if input.count <= 1 {
        *input = ItemStack::EMPTY;
    } else {
        input.count -= 1;
    }
    Some(ItemStack::new(recipe.result_item, recipe.result_count))
}

// ============================================================================
// 6. Villager Trading (MerchantMenu / VillagerTrades)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VillagerProfession {
    Farmer,
    Librarian,
    Armorer,
    Toolsmith,
    Weaponsmith,
    Cleric,
    Butcher,
    Mason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MerchantTradeOffer {
    pub cost_a: ItemStack,
    pub cost_b: ItemStack,
    pub result: ItemStack,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub price_multiplier: f32,
}

impl MerchantTradeOffer {
    #[inline]
    pub fn is_out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    #[inline]
    pub fn restock(&mut self) {
        self.uses = 0;
    }
}

fn trade(
    cost_a: (&str, u8),
    cost_b: Option<(&str, u8)>,
    result: (&str, u8),
    max_uses: i32,
    xp: i32,
    price_multiplier: f32,
) -> Option<MerchantTradeOffer> {
    let a = ItemStack::from_name(cost_a.0, cost_a.1)?;
    let b = match cost_b {
        Some((n, c)) => ItemStack::from_name(n, c)?,
        None => ItemStack::EMPTY,
    };
    let r = ItemStack::from_name(result.0, result.1)?;
    Some(MerchantTradeOffer {
        cost_a: a,
        cost_b: b,
        result: r,
        uses: 0,
        max_uses,
        xp,
        price_multiplier,
    })
}

/// Return authentic Minecraft 26.3 villager trade offers for `prof` up to tier `level` (`1..=5`:
/// 1=Novice, 2=Apprentice, 3=Journeyman, 4=Expert, 5=Master).
pub fn default_offers_for_profession(
    prof: VillagerProfession,
    level: u8,
) -> Vec<MerchantTradeOffer> {
    let max_level = level.min(5);
    if max_level == 0 {
        return Vec::new();
    }

    let mut offers = Vec::new();
    let mut push = |opt: Option<MerchantTradeOffer>| {
        if let Some(o) = opt {
            offers.push(o);
        }
    };

    match prof {
        VillagerProfession::Farmer => {
            if max_level >= 1 {
                push(trade(("minecraft:wheat", 20), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:bread", 6), 16, 1, 0.05));
            }
            if max_level >= 2 {
                push(trade(("minecraft:pumpkin", 6), None, ("minecraft:emerald", 1), 12, 10, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:apple", 4), 16, 5, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:melon", 4), None, ("minecraft:emerald", 1), 12, 20, 0.05));
                push(trade(("minecraft:emerald", 3), None, ("minecraft:cookie", 18), 12, 10, 0.05));
            }
            if max_level >= 4 {
                push(trade(("minecraft:emerald", 3), None, ("minecraft:cake", 1), 12, 15, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:suspicious_stew", 1), 12, 15, 0.05));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 3), None, ("minecraft:golden_carrot", 3), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:glistering_melon_slice", 3), 12, 30, 0.05));
            }
        }
        VillagerProfession::Librarian => {
            if max_level >= 1 {
                push(trade(("minecraft:paper", 24), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:emerald", 9), None, ("minecraft:bookshelf", 1), 12, 1, 0.05));
            }
            if max_level >= 2 {
                push(trade(("minecraft:book", 4), None, ("minecraft:emerald", 1), 12, 10, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:lantern", 1), 12, 5, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:ink_sac", 5), None, ("minecraft:emerald", 1), 12, 20, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:glass", 4), 12, 10, 0.05));
            }
            if max_level >= 4 {
                push(trade(
                    ("minecraft:emerald", 10),
                    Some(("minecraft:book", 1)),
                    ("minecraft:enchanted_book", 1),
                    12,
                    15,
                    0.2,
                ));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:compass", 1), 12, 15, 0.05));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 5), None, ("minecraft:clock", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 3), None, ("minecraft:red_candle", 1), 12, 30, 0.05));
            }
        }
        VillagerProfession::Armorer => {
            if max_level >= 1 {
                push(trade(("minecraft:emerald", 5), None, ("minecraft:iron_helmet", 1), 12, 1, 0.2));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:iron_boots", 1), 12, 1, 0.2));
            }
            if max_level >= 2 {
                push(trade(("minecraft:emerald", 3), None, ("minecraft:chainmail_leggings", 1), 12, 5, 0.2));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:chainmail_boots", 1), 12, 5, 0.2));
            }
            if max_level >= 3 {
                push(trade(("minecraft:diamond", 1), None, ("minecraft:emerald", 1), 12, 20, 0.05));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:chainmail_chestplate", 1), 12, 10, 0.2));
            }
            if max_level >= 4 {
                push(trade(("minecraft:emerald", 8), None, ("minecraft:diamond_boots", 1), 3, 15, 0.2));
                push(trade(("minecraft:emerald", 14), None, ("minecraft:diamond_leggings", 1), 3, 15, 0.2));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 13), None, ("minecraft:diamond_helmet", 1), 3, 30, 0.2));
                push(trade(("minecraft:emerald", 21), None, ("minecraft:diamond_chestplate", 1), 3, 30, 0.2));
            }
        }
        VillagerProfession::Toolsmith => {
            if max_level >= 1 {
                push(trade(("minecraft:emerald", 1), None, ("minecraft:stone_pickaxe", 1), 12, 1, 0.2));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:stone_axe", 1), 12, 1, 0.2));
            }
            if max_level >= 2 {
                push(trade(("minecraft:iron_ingot", 4), None, ("minecraft:emerald", 1), 12, 10, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:iron_shovel", 1), 12, 5, 0.2));
            }
            if max_level >= 3 {
                push(trade(("minecraft:emerald", 3), None, ("minecraft:iron_pickaxe", 1), 3, 10, 0.2));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:diamond_hoe", 1), 3, 10, 0.2));
            }
            if max_level >= 4 {
                push(trade(("minecraft:diamond", 1), None, ("minecraft:emerald", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 12), None, ("minecraft:diamond_axe", 1), 3, 15, 0.2));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 13), None, ("minecraft:diamond_pickaxe", 1), 3, 30, 0.2));
            }
        }
        VillagerProfession::Weaponsmith => {
            if max_level >= 1 {
                push(trade(("minecraft:emerald", 2), None, ("minecraft:iron_sword", 1), 12, 1, 0.2));
                push(trade(("minecraft:emerald", 3), None, ("minecraft:iron_axe", 1), 12, 1, 0.2));
            }
            if max_level >= 2 {
                push(trade(("minecraft:iron_ingot", 4), None, ("minecraft:emerald", 1), 12, 10, 0.05));
                push(trade(("minecraft:coal", 15), None, ("minecraft:emerald", 1), 16, 2, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:flint", 24), None, ("minecraft:emerald", 1), 12, 20, 0.05));
            }
            if max_level >= 4 {
                push(trade(("minecraft:diamond", 1), None, ("minecraft:emerald", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 12), None, ("minecraft:diamond_axe", 1), 3, 15, 0.2));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 8), None, ("minecraft:diamond_sword", 1), 3, 30, 0.2));
            }
        }
        VillagerProfession::Cleric => {
            if max_level >= 1 {
                push(trade(("minecraft:rotten_flesh", 32), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:redstone", 2), 12, 1, 0.05));
            }
            if max_level >= 2 {
                push(trade(("minecraft:gold_ingot", 3), None, ("minecraft:emerald", 1), 12, 10, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:lapis_lazuli", 1), 12, 5, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:rabbit_foot", 2), None, ("minecraft:emerald", 1), 12, 20, 0.05));
                push(trade(("minecraft:emerald", 4), None, ("minecraft:glowstone", 1), 12, 10, 0.05));
            }
            if max_level >= 4 {
                push(trade(("minecraft:glass_bottle", 9), None, ("minecraft:emerald", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 5), None, ("minecraft:ender_pearl", 1), 12, 15, 0.05));
            }
            if max_level >= 5 {
                push(trade(("minecraft:nether_wart", 22), None, ("minecraft:emerald", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 3), None, ("minecraft:experience_bottle", 1), 12, 30, 0.05));
            }
        }
        VillagerProfession::Butcher => {
            if max_level >= 1 {
                push(trade(("minecraft:chicken", 14), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:porkchop", 7), None, ("minecraft:emerald", 1), 16, 2, 0.05));
            }
            if max_level >= 2 {
                push(trade(("minecraft:coal", 15), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:cooked_porkchop", 5), 16, 5, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:beef", 10), None, ("minecraft:emerald", 1), 16, 20, 0.05));
                push(trade(("minecraft:mutton", 7), None, ("minecraft:emerald", 1), 16, 20, 0.05));
            }
            if max_level >= 4 {
                push(trade(("minecraft:dried_kelp_block", 10), None, ("minecraft:emerald", 1), 12, 30, 0.05));
            }
            if max_level >= 5 {
                push(trade(("minecraft:sweet_berries", 10), None, ("minecraft:emerald", 1), 12, 30, 0.05));
            }
        }
        VillagerProfession::Mason => {
            if max_level >= 1 {
                push(trade(("minecraft:clay_ball", 10), None, ("minecraft:emerald", 1), 16, 2, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:brick", 10), 16, 1, 0.05));
            }
            if max_level >= 2 {
                push(trade(("minecraft:stone", 20), None, ("minecraft:emerald", 1), 16, 10, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:chiseled_stone_bricks", 4), 16, 5, 0.05));
            }
            if max_level >= 3 {
                push(trade(("minecraft:andesite", 16), None, ("minecraft:emerald", 1), 16, 20, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:dripstone_block", 4), 16, 10, 0.05));
            }
            if max_level >= 4 {
                push(trade(("minecraft:emerald", 1), None, ("minecraft:black_terracotta", 1), 12, 15, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:black_glazed_terracotta", 1), 12, 15, 0.05));
            }
            if max_level >= 5 {
                push(trade(("minecraft:emerald", 1), None, ("minecraft:quartz_block", 1), 12, 30, 0.05));
                push(trade(("minecraft:emerald", 1), None, ("minecraft:quartz_pillar", 1), 12, 30, 0.05));
            }
        }
    }

    offers
}

/// Execute a villager trade if `offer` is in stock and `pay_a` / `pay_b` satisfy the required items.
///
/// Consumes payment items, increments `offer.uses`, and returns `Some(offer.result.clone())`.
pub fn execute_trade(
    offer: &mut MerchantTradeOffer,
    pay_a: &mut ItemStack,
    pay_b: &mut ItemStack,
) -> Option<ItemStack> {
    if offer.is_out_of_stock() || offer.cost_a.is_empty() || offer.result.is_empty() {
        return None;
    }
    if pay_a.is_empty() || pay_a.item_id != offer.cost_a.item_id || pay_a.count < offer.cost_a.count
    {
        return None;
    }
    if !offer.cost_b.is_empty()
        && (pay_b.is_empty()
            || pay_b.item_id != offer.cost_b.item_id
            || pay_b.count < offer.cost_b.count)
    {
        return None;
    }

    if pay_a.count == offer.cost_a.count {
        *pay_a = ItemStack::EMPTY;
    } else {
        pay_a.count -= offer.cost_a.count;
    }

    if !offer.cost_b.is_empty() {
        if pay_b.count == offer.cost_b.count {
            *pay_b = ItemStack::EMPTY;
        } else {
            pay_b.count -= offer.cost_b.count;
        }
    }

    offer.uses += 1;
    Some(offer.result.clone())
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enchanting_table_0_vs_15_bookshelves() {
        let diamond_sword = item_by_name("minecraft:diamond_sword").unwrap().id;
        let stone = item_by_name("minecraft:stone").unwrap().id;

        // Non-enchantable block yields zero offers
        let stone_offers = compute_enchanting_offers(42, stone, 15);
        assert!(stone_offers.iter().all(|o| o.xp_cost == 0 && o.clue_enchantment.is_none()));

        // 0 bookshelves vs 15 bookshelves on a diamond sword
        let offers_0 = compute_enchanting_offers(12345, diamond_sword, 0);
        let offers_15 = compute_enchanting_offers(12345, diamond_sword, 15);

        // Vanilla 0 bookshelves: slot 2 max is 8; 15 bookshelves: slot 2 is always >= 30
        assert!(offers_0[2].xp_cost <= 8 && offers_0[2].xp_cost >= 1);
        assert_eq!(offers_15[2].xp_cost, 30);
        assert!(offers_15[2].xp_cost > offers_0[2].xp_cost);
        assert!(offers_15[2].clue_enchantment.is_some());
        assert!(offers_15[2].clue_level >= 1);

        // Picking enchantments for slot 2 at level 30 must include the exact previewed clue
        let rolled = pick_enchantments_for_offer(12345, diamond_sword, 2, offers_15[2].xp_cost);
        assert!(!rolled.is_empty());
        let clue = (
            offers_15[2].clue_enchantment.unwrap(),
            offers_15[2].clue_level as u8,
        );
        assert!(rolled.contains(&clue));
        // Never rolls treasure enchantments at the enchanting table
        for (eid, _) in &rolled {
            assert!(!enchantment_by_id(eid).unwrap().is_treasure);
        }
    }

    #[test]
    fn anvil_combining_level_up_conflicts_repair_and_rename() {
        let mut sword_a = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        let sword_b = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        let ench_book = ItemStack::from_name("minecraft:enchanted_book", 1).unwrap();

        // 1. Combining two Sharpness III swords -> Sharpness IV + rename
        let (res, enchants) = compute_anvil_output(
            &sword_a,
            &sword_b,
            Some("Excalibur"),
            &[("minecraft:sharpness".to_string(), 3)],
            &[("minecraft:sharpness".to_string(), 3)],
        )
        .expect("valid anvil combine");
        assert!(enchants.contains(&("minecraft:sharpness".to_string(), 4)));
        assert_eq!(item_custom_name(&res.output).as_deref(), Some("Excalibur"));
        assert_eq!(res.xp_cost, 5); // Sharpness IV (1 * 4) + rename (1)
        assert_eq!(item_repair_cost(&res.output), 1);

        // 2. Conflicting enchantment (Sharpness vs Smite) rejected if no compatible enchant applies
        let conflict = compute_anvil_output(
            &sword_a,
            &ench_book,
            None,
            &[("minecraft:sharpness".to_string(), 4)],
            &[("minecraft:smite".to_string(), 3)],
        );
        assert!(conflict.is_none());

        // 3. Material repair: damaged diamond sword repaired by diamonds (25% per diamond)
        set_item_damage(&mut sword_a, 800);
        let diamonds = ItemStack::from_name("minecraft:diamond", 4).unwrap();
        let (rep, _) = compute_anvil_output(&sword_a, &diamonds, None, &[], &[])
            .expect("valid diamond repair");
        assert_eq!(rep.repair_items_used, 3); // 1561/4 = 390 per diamond -> 390+390+20 = 800
        assert_eq!(item_damage(&rep.output), 0);
        assert_eq!(rep.xp_cost, 3);
    }

    #[test]
    fn brewing_stand_400_tick_brew_and_nbt_roundtrip() {
        let mut stand = BrewingStandState::new();
        let water = make_potion_stack("minecraft:potion", "minecraft:water").unwrap();
        stand.set_bottle(0, water.clone(), None);
        stand.set_bottle(1, water, None);
        stand.slots[3] = ItemStack::from_name("minecraft:nether_wart", 2).unwrap();
        stand.slots[4] = ItemStack::from_name("minecraft:blaze_powder", 1).unwrap();

        // Tick 1: consumes blaze powder -> 20 fuel, starts brew -> fuel=19, brew_time=400
        assert!(stand.tick_brewing());
        assert!(stand.slots[4].is_empty());
        assert_eq!(stand.fuel, 19);
        assert_eq!(stand.brew_time, 400);

        // Mid-brew NBT roundtrip at tick 100
        for _ in 0..100 {
            stand.tick_brewing();
        }
        assert_eq!(stand.brew_time, 300);
        let saved = stand.save_to_nbt();
        let mut stand = BrewingStandState::load_from_nbt(&saved);
        assert_eq!(stand.brew_time, 300);
        assert_eq!(stand.fuel, 19);
        assert_eq!(stand.bottle_potion(0), Some("minecraft:water"));

        // Finish remaining 300 ticks -> exact 400 ticks total -> awkward potions!
        for _ in 0..300 {
            stand.tick_brewing();
        }
        assert_eq!(stand.brew_time, 0);
        assert_eq!(stand.bottle_potion(0), Some("minecraft:awkward"));
        assert_eq!(stand.bottle_potion(1), Some("minecraft:awkward"));
        assert_eq!(stand.slots[3].count, 1); // 1 nether wart consumed
    }

    #[test]
    fn smithing_netherite_upgrade_preserves_components() {
        let mut template =
            ItemStack::from_name("minecraft:netherite_upgrade_smithing_template", 1).unwrap();
        let mut base = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        set_item_custom_name(&mut base, Some("Netherbane"));
        set_item_damage(&mut base, 42);
        let mut addition = ItemStack::from_name("minecraft:netherite_ingot", 2).unwrap();

        let out = craft_smithing(&mut template, &mut base, &mut addition)
            .expect("netherite sword upgrade");
        assert_eq!(
            out.item_id,
            item_by_name("minecraft:netherite_sword").unwrap().id
        );
        assert_eq!(item_custom_name(&out).as_deref(), Some("Netherbane"));
        assert_eq!(item_damage(&out), 42);
        assert!(template.is_empty());
        assert!(base.is_empty());
        assert_eq!(addition.count, 1);
    }

    #[test]
    fn stonecutter_lists_and_crafts_stone_variants() {
        let mut stone = ItemStack::from_name("minecraft:stone", 5).unwrap();
        let outputs = stonecutter_outputs(&stone);
        assert!(outputs.len() >= 5, "stone should have many stonecutting outputs");
        let expected_first = outputs[0].clone();

        let crafted = craft_stonecutter(&mut stone, 0).expect("craft first stonecutting recipe");
        assert_eq!(crafted, expected_first);
        assert_eq!(stone.count, 4);
    }

    #[test]
    fn villager_trading_executes_and_respects_max_uses() {
        let mut offers = default_offers_for_profession(VillagerProfession::Farmer, 1);
        assert!(!offers.is_empty());
        let wheat_offer = offers
            .iter_mut()
            .find(|o| o.cost_a.item_id == item_by_name("minecraft:wheat").unwrap().id)
            .expect("farmer wheat trade");

        let mut pay_a = ItemStack::from_name("minecraft:wheat", 45).unwrap();
        let mut pay_b = ItemStack::EMPTY;

        let out1 = execute_trade(wheat_offer, &mut pay_a, &mut pay_b).expect("trade 1");
        assert_eq!(out1.item_id, item_by_name("minecraft:emerald").unwrap().id);
        assert_eq!(pay_a.count, 25);
        assert_eq!(wheat_offer.uses, 1);

        let _out2 = execute_trade(wheat_offer, &mut pay_a, &mut pay_b).expect("trade 2");
        assert_eq!(pay_a.count, 5);
        assert_eq!(wheat_offer.uses, 2);

        // Insufficient wheat fails cleanly
        assert!(execute_trade(wheat_offer, &mut pay_a, &mut pay_b).is_none());

        // Out-of-stock offer fails cleanly even with enough items
        wheat_offer.uses = wheat_offer.max_uses;
        let mut fresh_wheat = ItemStack::from_name("minecraft:wheat", 64).unwrap();
        assert!(execute_trade(wheat_offer, &mut fresh_wheat, &mut pay_b).is_none());
        assert_eq!(fresh_wheat.count, 64);

        // Two-slot trade (Librarian L4 enchanted book: emeralds + book)
        let mut lib_offers = default_offers_for_profession(VillagerProfession::Librarian, 4);
        let book_offer = lib_offers
            .iter_mut()
            .find(|o| !o.cost_b.is_empty())
            .expect("librarian 2-input trade");
        let mut ems = ItemStack::from_name("minecraft:emerald", 10).unwrap();
        let mut books = ItemStack::from_name("minecraft:book", 1).unwrap();
        let got = execute_trade(book_offer, &mut ems, &mut books).expect("2-input trade");
        assert_eq!(
            got.item_id,
            item_by_name("minecraft:enchanted_book").unwrap().id
        );
        assert!(ems.is_empty());
        assert!(books.is_empty());

        // Every profession at level 5 has valid non-empty Minecraft 26.3 offers
        for prof in [
            VillagerProfession::Farmer,
            VillagerProfession::Librarian,
            VillagerProfession::Armorer,
            VillagerProfession::Toolsmith,
            VillagerProfession::Weaponsmith,
            VillagerProfession::Cleric,
            VillagerProfession::Butcher,
            VillagerProfession::Mason,
        ] {
            let all_offers = default_offers_for_profession(prof, 5);
            assert!(all_offers.len() >= 7, "{prof:?} should have master-level offers");
            for offer in &all_offers {
                assert!(!offer.cost_a.is_empty() && !offer.result.is_empty());
            }
        }
    }

    #[test]
    fn smithing_armor_trim_and_multi_stage_brewing() {
        // Armor trim smithing recipe
        let trim_tpl =
            ItemStack::from_name("minecraft:bolt_armor_trim_smithing_template", 1).unwrap();
        let chest = ItemStack::from_name("minecraft:diamond_chestplate", 1).unwrap();
        let emerald = ItemStack::from_name("minecraft:emerald", 1).unwrap();
        let trimmed = compute_smithing_output(&trim_tpl, &chest, &emerald)
            .expect("valid armor trim smithing");
        assert_eq!(trimmed.item_id, chest.item_id);

        // Multi-stage brewing: awkward potion + blaze_powder -> strength -> + gunpowder -> splash_potion
        let mut stand = BrewingStandState::new();
        stand.fuel = 5;
        let awkward = make_potion_stack("minecraft:potion", "minecraft:awkward").unwrap();
        stand.set_bottle(0, awkward, None);
        stand.slots[3] = ItemStack::from_name("minecraft:blaze_powder", 1).unwrap();
        for _ in 0..=400 {
            stand.tick_brewing();
        }
        assert_eq!(stand.bottle_potion(0), Some("minecraft:strength"));
        assert_eq!(
            stand.slots[0].item_id,
            item_by_name("minecraft:potion").unwrap().id
        );

        stand.slots[3] = ItemStack::from_name("minecraft:gunpowder", 1).unwrap();
        for _ in 0..=400 {
            stand.tick_brewing();
        }
        assert_eq!(stand.bottle_potion(0), Some("minecraft:strength"));
        assert_eq!(
            stand.slots[0].item_id,
            item_by_name("minecraft:splash_potion").unwrap().id
        );
    }

    #[test]
    fn anvil_rename_only_and_too_expensive_cap() {
        let mut pick = ItemStack::from_name("minecraft:diamond_pickaxe", 1).unwrap();
        // Rename-only works and does NOT increase prior work penalty
        let (renamed, _) = compute_anvil_output(&pick, &ItemStack::EMPTY, Some("Tunnel Bore"), &[], &[])
            .expect("rename only");
        assert_eq!(renamed.xp_cost, 1);
        assert_eq!(item_repair_cost(&renamed.output), 0);
        assert_eq!(
            item_custom_name(&renamed.output).as_deref(),
            Some("Tunnel Bore")
        );

        // High prior-work penalty (>= 40) blocks combining, but rename-only caps at 39
        set_item_repair_cost(&mut pick, 39);
        let book = ItemStack::from_name("minecraft:enchanted_book", 1).unwrap();
        assert!(
            compute_anvil_output(
                &pick,
                &book,
                None,
                &[],
                &[("minecraft:efficiency".to_string(), 5)],
            )
            .is_none()
        );
        let (capped_rename, _) =
            compute_anvil_output(&pick, &ItemStack::EMPTY, Some("Old Faithful"), &[], &[])
                .expect("rename caps at 39");
        assert_eq!(capped_rename.xp_cost, 39);
    }
}
