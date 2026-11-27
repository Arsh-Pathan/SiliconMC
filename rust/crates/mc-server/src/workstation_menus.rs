//! Interactive server-side workstation menu handlers for Minecraft 26.3 (Protocol 777).
//!
//! Covers all 6 non-storage interactive workstations:
//! 1. Enchanting Table (`menu_types::ENCHANTMENT`)
//! 2. Anvil (`menu_types::ANVIL`)
//! 3. Brewing Stand (`menu_types::BREWING_STAND`, backed by `BrewingStandState`)
//! 4. Smithing Table (`menu_types::SMITHING`)
//! 5. Stonecutter (`menu_types::STONECUTTER`)
//! 6. Villager Trading / Merchant (`menu_types::MERCHANT`)
//!
//! Slot layouts, `ContainerSetData` indices, button IDs, and click semantics match the
//! decompiled Minecraft 26.3 `net.minecraft.world.inventory.*Menu` classes.

use mc_data::gameplay_tables::{
    ENCHANTMENTS, SMITHING_RECIPES, SMITHING_TRIM_MATERIALS, SMITHING_TRIM_TEMPLATES,
    SMITHING_TRIMMABLE_ARMOR, data_components, enchantment_index, is_brewing_potion_input,
    is_brewing_reagent, item_enchantability,
};
use mc_data::items::{item_by_id, item_by_name};
use mc_data::recipes::menu_types;
use mc_math::BlockPos;
use mc_proto::{
    ItemCost, MerchantOffer, encode_play_container_set_content, encode_play_container_set_data,
    encode_play_merchant_offers, encode_play_open_screen,
};
use mc_world::{
    BLAZE_POWDER_FUEL_CHARGES, BREWING_DURATION_TICKS, BlockWorld, BrewingStandState,
    ContainerClickType, EnchantmentOffer, ItemStack, MerchantTradeOffer, PlayerInventory,
    VillagerProfession, compute_anvil_output, compute_enchanting_offers, compute_smithing_output,
    craft_smithing, craft_stonecutter, default_offers_for_profession, execute_trade,
    item_custom_name, item_potion_id, pick_enchantments_for_offer, stonecutting_for_input,
};
use rand::RngCore;
use std::ops::Range;

/// Alias for `mc_world::MerchantTradeOffer` for ergonomic integration.
pub type VillagerTradeOffer = MerchantTradeOffer;

// ============================================================================
// Wire-exact Minecraft 26.3 ItemEnchantments DataComponentPatch helpers
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

#[inline]
fn is_enchanted_book(item_id: u16) -> bool {
    item_by_id(item_id)
        .map(|i| i.name == "minecraft:enchanted_book")
        .unwrap_or(false)
}

#[inline]
fn enchantment_component_type(item_id: u16) -> i32 {
    if is_enchanted_book(item_id) {
        data_components::STORED_ENCHANTMENTS
    } else {
        data_components::ENCHANTMENTS
    }
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

/// Decode enchantments stored in `minecraft:enchantments` (or `minecraft:stored_enchantments`
/// for `minecraft:enchanted_book`) matching Minecraft 26.3 `ItemEnchantments.STREAM_CODEC`.
pub fn read_item_enchantments(stack: &ItemStack) -> Vec<(String, u8)> {
    if stack.is_empty() {
        return Vec::new();
    }
    let comp_id = enchantment_component_type(stack.item_id);
    let Some((_, bytes)) = stack.added_components.iter().find(|(id, _)| *id == comp_id) else {
        return Vec::new();
    };
    let Some((count, mut offset)) = decode_varint(bytes) else {
        return Vec::new();
    };
    if count <= 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((count as usize).min(64));
    for _ in 0..count {
        let Some((ench_idx, used1)) = decode_varint(&bytes[offset..]) else {
            break;
        };
        offset += used1;
        let Some((level, used2)) = decode_varint(&bytes[offset..]) else {
            break;
        };
        offset += used2;
        if ench_idx >= 0 && level > 0 {
            if let Some(spec) = ENCHANTMENTS.get(ench_idx as usize) {
                out.push((spec.id.to_string(), level.clamp(1, 255) as u8));
            }
        }
    }
    out
}

/// Encode enchantments onto `stack` in exact Minecraft 26.3 `ItemEnchantments.STREAM_CODEC` format
/// (`VarInt count`, then `(VarInt enchantment_registry_id, VarInt level)` pairs).
///
/// Writes `minecraft:stored_enchantments` for `minecraft:enchanted_book`, and `minecraft:enchantments`
/// for all other items.
pub fn write_item_enchantments(stack: &mut ItemStack, enchants: &[(String, u8)]) {
    if stack.is_empty() {
        return;
    }
    let comp_id = enchantment_component_type(stack.item_id);
    let mut resolved: Vec<(i32, i32)> = Vec::new();
    for (id, level) in enchants {
        if *level == 0 {
            continue;
        }
        if let Some(idx) = enchantment_index(id) {
            let idx_i32 = idx as i32;
            if let Some(existing) = resolved.iter_mut().find(|(k, _)| *k == idx_i32) {
                existing.1 = (*level).min(255) as i32;
            } else {
                resolved.push((idx_i32, (*level).min(255) as i32));
            }
        }
    }
    if resolved.is_empty() {
        remove_component(stack, comp_id);
        return;
    }
    let mut buf = Vec::with_capacity(1 + resolved.len() * 2);
    buf.extend_from_slice(&encode_varint(resolved.len() as u32));
    for (idx, level) in resolved {
        buf.extend_from_slice(&encode_varint(idx as u32));
        buf.extend_from_slice(&encode_varint(level as u32));
    }
    set_component_bytes(stack, comp_id, buf);
}

/// Roll and apply enchanting-table offer `slot` (`0..=2`) onto `stack` in-place.
///
/// Transmutes `minecraft:book` into `minecraft:enchanted_book` with `minecraft:stored_enchantments`
/// (matching vanilla `EnchantmentMenu.clickMenuButton`). Returns the applied `(id, level)` list.
pub fn apply_enchanting_offer(
    stack: &mut ItemStack,
    seed: i32,
    slot: u8,
    xp_cost: i32,
) -> Vec<(&'static str, u8)> {
    if stack.is_empty() || slot >= 3 || xp_cost <= 0 || !read_item_enchantments(stack).is_empty() {
        return Vec::new();
    }
    let rolled = pick_enchantments_for_offer(seed, stack.item_id, slot, xp_cost);
    if rolled.is_empty() {
        return Vec::new();
    }
    let is_plain_book = item_by_id(stack.item_id)
        .map(|i| i.name == "minecraft:book")
        .unwrap_or(false);
    if is_plain_book {
        if let Some(ench_book) = item_by_name("minecraft:enchanted_book") {
            stack.item_id = ench_book.id;
            stack.count = 1;
        }
    }
    let owned: Vec<(String, u8)> = rolled.iter().map(|&(id, lvl)| (id.to_string(), lvl)).collect();
    write_item_enchantments(stack, &owned);
    rolled
}

// ============================================================================
// Bookshelf Scanner (vanilla EnchantingTableBlock.BOOKSHELF_OFFSETS)
// ============================================================================

/// Count valid bookshelves surrounding an Enchanting Table at `table_pos`, capped at 15.
///
/// Exact vanilla Minecraft 26.3 `EnchantingTableBlock.BOOKSHELF_OFFSETS` + `isValidBookShelf`:
/// - `dx` in `-2..=2`, `dz` in `-2..=2` where `|dx| == 2 || |dz| == 2`
/// - `dy` in `0..=1`
/// - Target block at `(x + dx, y + dy, z + dz)` is `minecraft:bookshelf` (`#minecraft:enchantment_power_provider`)
/// - Intermediate block at `(x + dx/2, y + dy, z + dz/2)` is air or replaceable (`#minecraft:enchantment_power_transmitter`)
pub fn count_bookshelves_around<W: BlockWorld>(world: &mut W, table_pos: BlockPos) -> u8 {
    let mut count = 0u8;
    for dy in 0..=1 {
        for dx in -2..=2_i32 {
            for dz in -2..=2_i32 {
                if dx.abs() != 2 && dz.abs() != 2 {
                    continue;
                }
                let shelf_pos = table_pos.offset(dx, dy, dz);
                if world.get_block_state(shelf_pos).block_name() != "minecraft:bookshelf" {
                    continue;
                }
                let mid_pos = table_pos.offset(dx / 2, dy, dz / 2);
                let mid_state = world.get_block_state(mid_pos);
                if mid_state.is_air() || mid_state.props().can_be_replaced() {
                    count = count.saturating_add(1);
                    if count >= 15 {
                        return 15;
                    }
                }
            }
        }
    }
    count
}

// ============================================================================
// Outcome & WorkstationMenu Types
// ============================================================================

/// Result of handling a workstation slot click or menu button click.
#[derive(Debug, Clone, Default)]
pub struct MenuClickOutcome {
    /// Ready-to-send Play clientbound packets (`ContainerSetContent`, `ContainerSetData`, `MerchantOffers`).
    pub packets: Vec<Vec<u8>>,
    /// Change to apply to the player's XP level (negative when XP levels are spent).
    pub xp_level_delta: i32,
    /// Rotated enchantment seed to persist on the player after enchanting an item.
    pub new_enchant_seed: Option<i32>,
    /// Items dropped into the world (e.g., via `Throw` or clicking outside window `-999`).
    pub dropped_items: Vec<ItemStack>,
    /// `true` if a survival anvil use triggered the 12% anvil damage check.
    pub anvil_damaged: bool,
    /// `true` if a villager trade was completed during this click.
    pub completed_trade: bool,
}

/// Interactive server-side workstation menu state for a player.
#[derive(Debug, Clone)]
pub enum WorkstationMenu {
    Enchanting {
        container_id: i32,
        pos: BlockPos,
        slots: [ItemStack; 2],
        seed: i32,
        bookshelves: u8,
        offers: [EnchantmentOffer; 3],
    },
    Anvil {
        container_id: i32,
        pos: BlockPos,
        slots: [ItemStack; 3],
        rename_text: String,
        xp_cost: i32,
        repair_material_count: u8,
    },
    /// Backed by a world `BrewingStandState` block entity; use `brewing_open_packets`,
    /// `brewing_sync_packets`, and `brewing_handle_click` with the block entity state.
    BrewingStand {
        container_id: i32,
        pos: BlockPos,
    },
    Smithing {
        container_id: i32,
        slots: [ItemStack; 4],
    },
    Stonecutter {
        container_id: i32,
        slots: [ItemStack; 2],
        selected_recipe: i32,
    },
    Merchant {
        container_id: i32,
        villager_eid: i32,
        profession: VillagerProfession,
        level: i32,
        xp: i32,
        offers: Vec<MerchantTradeOffer>,
        selected_trade: i32,
        slots: [ItemStack; 3],
    },
}

impl WorkstationMenu {
    pub fn new_enchanting(container_id: i32, pos: BlockPos, seed: i32, bookshelves: u8) -> Self {
        Self::Enchanting {
            container_id,
            pos,
            slots: [ItemStack::EMPTY, ItemStack::EMPTY],
            seed,
            bookshelves: bookshelves.min(15),
            offers: [EnchantmentOffer::default(); 3],
        }
    }

    pub fn new_anvil(container_id: i32, pos: BlockPos) -> Self {
        Self::Anvil {
            container_id,
            pos,
            slots: [ItemStack::EMPTY, ItemStack::EMPTY, ItemStack::EMPTY],
            rename_text: String::new(),
            xp_cost: 0,
            repair_material_count: 0,
        }
    }

    pub fn new_brewing_stand(container_id: i32, pos: BlockPos) -> Self {
        Self::BrewingStand { container_id, pos }
    }

    pub fn new_smithing(container_id: i32) -> Self {
        Self::Smithing {
            container_id,
            slots: [
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
            ],
        }
    }

    pub fn new_stonecutter(container_id: i32) -> Self {
        Self::Stonecutter {
            container_id,
            slots: [ItemStack::EMPTY, ItemStack::EMPTY],
            selected_recipe: -1,
        }
    }

    pub fn new_merchant(
        container_id: i32,
        villager_eid: i32,
        profession: VillagerProfession,
        level: i32,
        xp: i32,
        offers: Vec<MerchantTradeOffer>,
    ) -> Self {
        Self::Merchant {
            container_id,
            villager_eid,
            profession,
            level: level.clamp(1, 5),
            xp: xp.max(0),
            offers,
            selected_trade: -1,
            slots: [ItemStack::EMPTY, ItemStack::EMPTY, ItemStack::EMPTY],
        }
    }

    pub fn new_merchant_default(
        container_id: i32,
        villager_eid: i32,
        profession: VillagerProfession,
        level: i32,
        xp: i32,
    ) -> Self {
        let clamped_level = level.clamp(1, 5);
        let offers = default_offers_for_profession(profession, clamped_level as u8);
        Self::new_merchant(container_id, villager_eid, profession, clamped_level, xp, offers)
    }

    #[inline]
    pub fn container_id(&self) -> i32 {
        match self {
            Self::Enchanting { container_id, .. }
            | Self::Anvil { container_id, .. }
            | Self::BrewingStand { container_id, .. }
            | Self::Smithing { container_id, .. }
            | Self::Stonecutter { container_id, .. }
            | Self::Merchant { container_id, .. } => *container_id,
        }
    }

    #[inline]
    pub fn block_pos(&self) -> Option<BlockPos> {
        match self {
            Self::Enchanting { pos, .. }
            | Self::Anvil { pos, .. }
            | Self::BrewingStand { pos, .. } => Some(*pos),
            Self::Smithing { .. } | Self::Stonecutter { .. } | Self::Merchant { .. } => None,
        }
    }

    #[inline]
    pub fn menu_type_id(&self) -> i32 {
        match self {
            Self::Enchanting { .. } => menu_types::ENCHANTMENT,
            Self::Anvil { .. } => menu_types::ANVIL,
            Self::BrewingStand { .. } => menu_types::BREWING_STAND,
            Self::Smithing { .. } => menu_types::SMITHING,
            Self::Stonecutter { .. } => menu_types::STONECUTTER,
            Self::Merchant { .. } => menu_types::MERCHANT,
        }
    }

    #[inline]
    pub fn title(&self) -> &'static str {
        match self {
            Self::Enchanting { .. } => "Enchant",
            Self::Anvil { .. } => "Repair & Name",
            Self::BrewingStand { .. } => "Brewing Stand",
            Self::Smithing { .. } => "Upgrade Gear",
            Self::Stonecutter { .. } => "Stonecutter",
            Self::Merchant { profession, .. } => profession_title(*profession),
        }
    }

    /// Build all initial Play packets to open this workstation screen on the client:
    /// `OpenScreen` + `ContainerSetContent` + all `ContainerSetData` (+ `MerchantOffers` for `Merchant`).
    ///
    /// Note: For `WorkstationMenu::BrewingStand`, prefer `brewing_open_packets(&state, cid, inv)`
    /// so the real world `BrewingStandState` slots/fuel/brew_time are transmitted.
    pub fn open_packets(&self, inv: &PlayerInventory) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        if let Ok(open_pkt) =
            encode_play_open_screen(self.container_id(), self.menu_type_id(), self.title())
        {
            packets.push(open_pkt);
        }
        packets.extend(self.sync_packets(inv));
        if let Self::Merchant {
            container_id,
            level,
            xp,
            offers,
            ..
        } = self
        {
            if let Some(offers_pkt) =
                build_merchant_offers_packet(*container_id, offers, *level, *xp, true, true)
            {
                packets.push(offers_pkt);
            }
        }
        packets
    }

    /// Build Play sync packets (`ContainerSetContent` + all `ContainerSetData` values) for this menu.
    pub fn sync_packets(&self, inv: &PlayerInventory) -> Vec<Vec<u8>> {
        let cid = self.container_id();
        let mut packets = Vec::new();
        match self {
            Self::Enchanting {
                slots,
                seed,
                offers,
                ..
            } => {
                let flat = assemble_flat_slots(slots, inv);
                packets.push(encode_play_container_set_content(
                    cid,
                    inv.state_id,
                    &flat,
                    &inv.carried,
                ));
                // Exact EnchantmentMenu.java data slots:
                // 0..=2 = costs, 3 = seed, 4..=6 = clue enchantment IDs, 7..=9 = clue levels
                for (i, offer) in offers.iter().enumerate() {
                    packets.push(encode_play_container_set_data(
                        cid,
                        i as i16,
                        offer.xp_cost as i16,
                    ));
                }
                packets.push(encode_play_container_set_data(cid, 3, *seed as i16));
                for (i, offer) in offers.iter().enumerate() {
                    packets.push(encode_play_container_set_data(
                        cid,
                        (4 + i) as i16,
                        offer.clue_enchantment_id as i16,
                    ));
                }
                for (i, offer) in offers.iter().enumerate() {
                    packets.push(encode_play_container_set_data(
                        cid,
                        (7 + i) as i16,
                        offer.clue_level as i16,
                    ));
                }
            }
            Self::Anvil { slots, xp_cost, .. } => {
                let flat = assemble_flat_slots(slots, inv);
                packets.push(encode_play_container_set_content(
                    cid,
                    inv.state_id,
                    &flat,
                    &inv.carried,
                ));
                packets.push(encode_play_container_set_data(cid, 0, *xp_cost as i16));
            }
            Self::BrewingStand { .. } => {
                let empty_stand = BrewingStandState::new();
                packets.extend(brewing_sync_packets(&empty_stand, cid, inv));
            }
            Self::Smithing { slots, .. } => {
                let flat = assemble_flat_slots(slots, inv);
                packets.push(encode_play_container_set_content(
                    cid,
                    inv.state_id,
                    &flat,
                    &inv.carried,
                ));
                let has_recipe_error = !slots[0].is_empty()
                    && !slots[1].is_empty()
                    && !slots[2].is_empty()
                    && slots[3].is_empty();
                packets.push(encode_play_container_set_data(
                    cid,
                    0,
                    if has_recipe_error { 1 } else { 0 },
                ));
            }
            Self::Stonecutter {
                slots,
                selected_recipe,
                ..
            } => {
                let flat = assemble_flat_slots(slots, inv);
                packets.push(encode_play_container_set_content(
                    cid,
                    inv.state_id,
                    &flat,
                    &inv.carried,
                ));
                packets.push(encode_play_container_set_data(
                    cid,
                    0,
                    *selected_recipe as i16,
                ));
            }
            Self::Merchant { slots, .. } => {
                let flat = assemble_flat_slots(slots, inv);
                packets.push(encode_play_container_set_content(
                    cid,
                    inv.state_id,
                    &flat,
                    &inv.carried,
                ));
            }
        }
        packets
    }

    /// Handle a `ServerboundContainerClickPacket` on this workstation menu.
    ///
    /// Note: For `WorkstationMenu::BrewingStand`, call `brewing_handle_click` directly with the
    /// world block entity's `&mut BrewingStandState`.
    pub fn handle_click(
        &mut self,
        inv: &mut PlayerInventory,
        slot_num: i16,
        button: i8,
        click_type: ContainerClickType,
        player_xp_level: i32,
        is_creative: bool,
    ) -> MenuClickOutcome {
        let mut outcome = MenuClickOutcome::default();
        let mut resend_merchant_offers = false;

        match self {
            Self::Enchanting {
                slots,
                seed,
                bookshelves,
                offers,
                ..
            } => {
                let mut flat = assemble_flat_slots(slots, inv);
                let top_count = 2;
                let hotbar_start = top_count + 27;
                if let Some(dropped) = apply_slot_click(
                    &mut flat,
                    &mut inv.carried,
                    slot_num,
                    button,
                    click_type,
                    top_count,
                    hotbar_start,
                    None,
                    is_creative,
                    enchanting_may_place,
                    enchanting_max_stack,
                    enchanting_quick_move,
                ) {
                    outcome.dropped_items.push(dropped);
                }
                slots.clone_from_slice(&flat[..top_count]);
                write_back_player_inv(inv, &flat[top_count..]);
                recompute_enchanting_offers(&slots[0], *seed, *bookshelves, offers);
            }

            Self::Anvil {
                slots,
                rename_text,
                xp_cost,
                repair_material_count,
                ..
            } => {
                let mut flat = assemble_flat_slots(slots, inv);
                let top_count = 3;
                let hotbar_start = top_count + 27;

                if slot_num == 2 {
                    let mut ctx = (xp_cost, repair_material_count, rename_text);
                    handle_result_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        is_creative,
                        false,
                        &mut outcome,
                        &mut ctx,
                        |c, f| {
                            !f[2].is_empty()
                                && *c.0 > 0
                                && (is_creative || player_xp_level >= *c.0)
                        },
                        |_c, f| {
                            if f[2].is_empty() {
                                None
                            } else {
                                Some(f[2].clone())
                            }
                        },
                        |c, f, out| {
                            let (xp_cost, repair_material_count, rename_text) = c;
                            if f[2].is_empty()
                                || **xp_cost <= 0
                                || (!is_creative && player_xp_level < **xp_cost)
                            {
                                return None;
                            }
                            let taken = std::mem::take(&mut f[2]);
                            if !is_creative {
                                out.xp_level_delta -= **xp_cost;
                                if anvil_damage_roll() {
                                    out.anvil_damaged = true;
                                }
                            }
                            let left_ench = read_item_enchantments(&f[0]);
                            let right_ench = read_item_enchantments(&f[1]);
                            let was_rename_only = !f[1].is_empty()
                                && **repair_material_count == 0
                                && compute_anvil_output(&f[0], &f[1], None, &left_ench, &right_ench)
                                    .is_none();

                            if **repair_material_count > 0 {
                                shrink_stack(&mut f[1], **repair_material_count);
                            } else if !was_rename_only {
                                f[1] = ItemStack::EMPTY;
                            }
                            f[0] = ItemStack::EMPTY;
                            **xp_cost = 0;
                            **repair_material_count = 0;
                            rename_text.clear();
                            Some(taken)
                        },
                    );
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                } else {
                    let prev_left = slots[0].clone();
                    let prev_right = slots[1].clone();
                    if let Some(dropped) = apply_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        slot_num,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        Some(2),
                        is_creative,
                        |idx, _| idx != 2,
                        |_, s| s.max_stack_size(),
                        anvil_quick_move,
                    ) {
                        outcome.dropped_items.push(dropped);
                    }
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                    if slots[0] != prev_left || slots[1] != prev_right {
                        if slots[0].is_empty() {
                            rename_text.clear();
                        }
                        recompute_anvil(
                            slots,
                            rename_text,
                            xp_cost,
                            repair_material_count,
                            false,
                        );
                    }
                }
            }

            Self::BrewingStand { .. } => {
                // World-backed: callers should invoke `brewing_handle_click` with `&mut BrewingStandState`.
                return outcome;
            }

            Self::Smithing { slots, .. } => {
                let mut flat = assemble_flat_slots(slots, inv);
                let top_count = 4;
                let hotbar_start = top_count + 27;

                if slot_num == 3 {
                    let mut ctx = ();
                    handle_result_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        is_creative,
                        true,
                        &mut outcome,
                        &mut ctx,
                        |_, f| compute_smithing_output(&f[0], &f[1], &f[2]).is_some(),
                        |_, f| compute_smithing_output(&f[0], &f[1], &f[2]),
                        |_, f, _out| {
                            let (left, rest) = f.split_at_mut(1);
                            let (mid, tail) = rest.split_at_mut(1);
                            let (add, res) = tail.split_at_mut(1);
                            let crafted = craft_smithing(&mut left[0], &mut mid[0], &mut add[0])?;
                            res[0] = compute_smithing_output(&left[0], &mid[0], &add[0])
                                .unwrap_or(ItemStack::EMPTY);
                            Some(crafted)
                        },
                    );
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                } else {
                    if let Some(dropped) = apply_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        slot_num,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        Some(3),
                        is_creative,
                        smithing_may_place,
                        |_, s| s.max_stack_size(),
                        smithing_quick_move,
                    ) {
                        outcome.dropped_items.push(dropped);
                    }
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                    slots[3] = compute_smithing_output(&slots[0], &slots[1], &slots[2])
                        .unwrap_or(ItemStack::EMPTY);
                }
            }

            Self::Stonecutter {
                slots,
                selected_recipe,
                ..
            } => {
                let mut flat = assemble_flat_slots(slots, inv);
                let top_count = 2;
                let hotbar_start = top_count + 27;

                if slot_num == 1 {
                    handle_result_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        is_creative,
                        true,
                        &mut outcome,
                        selected_recipe,
                        |sel, f| stonecutter_preview(&f[0], *sel).is_some(),
                        |sel, f| stonecutter_preview(&f[0], *sel),
                        |sel, f, _out| {
                            if *sel < 0 {
                                return None;
                            }
                            let crafted =
                                craft_stonecutter(&mut f[0], *sel as usize)?;
                            if f[0].is_empty() {
                                *sel = -1;
                                f[1] = ItemStack::EMPTY;
                            } else {
                                f[1] = stonecutter_preview(&f[0], *sel)
                                    .unwrap_or(ItemStack::EMPTY);
                            }
                            Some(crafted)
                        },
                    );
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                } else {
                    let prev_input_id = slots[0].item_id;
                    if let Some(dropped) = apply_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        slot_num,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        Some(1),
                        is_creative,
                        |idx, _| idx != 1,
                        |_, s| s.max_stack_size(),
                        stonecutter_quick_move,
                    ) {
                        outcome.dropped_items.push(dropped);
                    }
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);

                    if slots[0].item_id != prev_input_id || slots[0].is_empty() {
                        *selected_recipe = -1;
                        slots[1] = ItemStack::EMPTY;
                    } else {
                        slots[1] = stonecutter_preview(&slots[0], *selected_recipe)
                            .unwrap_or(ItemStack::EMPTY);
                    }
                }
            }

            Self::Merchant {
                offers,
                selected_trade,
                slots,
                xp,
                ..
            } => {
                let mut flat = assemble_flat_slots(slots, inv);
                let top_count = 3;
                let hotbar_start = top_count + 27;

                if slot_num == 2 {
                    let sel = *selected_trade;
                    let mut ctx = (offers.as_mut_slice(), xp, &mut resend_merchant_offers);
                    handle_result_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        is_creative,
                        true,
                        &mut outcome,
                        &mut ctx,
                        |c, f| find_active_merchant_offer(c.0, sel, &f[0], &f[1]).is_some(),
                        |c, f| {
                            let (idx, _) =
                                find_active_merchant_offer(c.0, sel, &f[0], &f[1])?;
                            Some(c.0[idx].result.clone())
                        },
                        |c, f, out| {
                            let (idx, swapped) =
                                find_active_merchant_offer(c.0, sel, &f[0], &f[1])?;
                            let offer = &mut c.0[idx];
                            let (s0, rest) = f.split_at_mut(1);
                            let (s1, s2) = rest.split_at_mut(1);
                            let traded = if swapped {
                                execute_trade(offer, &mut s1[0], &mut s0[0])
                            } else {
                                execute_trade(offer, &mut s0[0], &mut s1[0])
                            }?;
                            *c.1 = c.1.saturating_add(offer.xp.max(0));
                            out.completed_trade = true;
                            *c.2 = true;
                            s2[0] = compute_merchant_result(c.0, sel, &s0[0], &s1[0]);
                            Some(traded)
                        },
                    );
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                } else {
                    if let Some(dropped) = apply_slot_click(
                        &mut flat,
                        &mut inv.carried,
                        slot_num,
                        button,
                        click_type,
                        top_count,
                        hotbar_start,
                        Some(2),
                        is_creative,
                        |idx, _| idx != 2,
                        |_, s| s.max_stack_size(),
                        merchant_quick_move,
                    ) {
                        outcome.dropped_items.push(dropped);
                    }
                    slots.clone_from_slice(&flat[..top_count]);
                    write_back_player_inv(inv, &flat[top_count..]);
                    slots[2] = compute_merchant_result(offers, *selected_trade, &slots[0], &slots[1]);
                }
            }
        }

        inv.next_state_id();
        outcome.packets = self.sync_packets(inv);
        if resend_merchant_offers {
            if let Self::Merchant {
                container_id,
                level,
                xp,
                offers,
                ..
            } = self
            {
                if let Some(pkt) =
                    build_merchant_offers_packet(*container_id, offers, *level, *xp, true, true)
                {
                    outcome.packets.push(pkt);
                }
            }
        }
        outcome
    }

    /// Handle a `ServerboundContainerButtonClickPacket` on this workstation menu:
    /// - `Enchanting`: buttons `0..=2` enchant slot 0 in-place, consume `button_id + 1` lapis and
    ///   `button_id + 1` XP levels (requiring `player_xp_level >= offer.xp_cost`), and rotate seed.
    /// - `Stonecutter`: button `button_id` selects the matching stonecutting recipe and fills slot 1.
    pub fn handle_button_click(
        &mut self,
        inv: &mut PlayerInventory,
        button_id: i32,
        player_xp_level: i32,
        is_creative: bool,
    ) -> MenuClickOutcome {
        let mut outcome = MenuClickOutcome::default();

        match self {
            Self::Enchanting {
                slots,
                seed,
                bookshelves,
                offers,
                ..
            } => {
                if !(0..=2).contains(&button_id) {
                    return outcome;
                }
                let btn = button_id as usize;
                let offer = offers[btn];
                let cost_tier = button_id + 1; // 1, 2, or 3 lapis & XP levels spent
                if offer.xp_cost <= 0 || slots[0].is_empty() {
                    return outcome;
                }
                if !is_creative {
                    if player_xp_level < offer.xp_cost || player_xp_level < cost_tier {
                        return outcome;
                    }
                    if !is_lapis_stack(&slots[1]) || slots[1].count < cost_tier as u8 {
                        return outcome;
                    }
                }

                let rolled =
                    apply_enchanting_offer(&mut slots[0], *seed, btn as u8, offer.xp_cost);
                if rolled.is_empty() {
                    return outcome;
                }

                if !is_creative {
                    shrink_stack(&mut slots[1], cost_tier as u8);
                    outcome.xp_level_delta = -cost_tier;
                }

                let next_seed = rotate_enchant_seed(*seed, slots[0].item_id, button_id);
                *seed = next_seed;
                outcome.new_enchant_seed = Some(next_seed);
                recompute_enchanting_offers(&slots[0], *seed, *bookshelves, offers);

                inv.next_state_id();
                outcome.packets = self.sync_packets(inv);
            }

            Self::Stonecutter {
                slots,
                selected_recipe,
                ..
            } => {
                if button_id < 0 || slots[0].is_empty() {
                    return outcome;
                }
                let recipes = stonecutting_for_input(slots[0].item_id);
                let Some(recipe) = recipes.get(button_id as usize) else {
                    return outcome;
                };
                *selected_recipe = button_id;
                slots[1] = ItemStack::new(recipe.result_item, recipe.result_count);

                inv.next_state_id();
                outcome.packets = self.sync_packets(inv);
            }

            _ => {}
        }

        outcome
    }

    /// Handle `ServerboundRenameItemPacket` for an open `Anvil` menu.
    pub fn handle_rename_item(&mut self, inv: &PlayerInventory, name: String) -> Vec<Vec<u8>> {
        let Self::Anvil {
            slots,
            rename_text,
            xp_cost,
            repair_material_count,
            ..
        } = self
        else {
            return Vec::new();
        };

        let filtered = filter_anvil_name(&name);
        if filtered.chars().count() > 50 {
            return self.sync_packets(inv);
        }
        *rename_text = filtered;
        recompute_anvil(slots, rename_text, xp_cost, repair_material_count, true);
        self.sync_packets(inv)
    }

    /// Handle `ServerboundSelectTradePacket` for an open `Merchant` menu.
    ///
    /// Matches vanilla `MerchantMenu.setSelectionHint` + `MerchantMenu.tryMoveItems`:
    /// returns items from slots `0`/`1` back to player inventory, moves matching payment items
    /// for the selected trade into slots `0`/`1`, and updates result slot `2`.
    pub fn handle_select_trade(&mut self, inv: &mut PlayerInventory, trade_idx: i32) -> Vec<Vec<u8>> {
        let Self::Merchant {
            offers,
            selected_trade,
            slots,
            ..
        } = self
        else {
            return Vec::new();
        };

        *selected_trade = trade_idx;
        if trade_idx >= 0 && (trade_idx as usize) < offers.len() {
            // 1. Return any existing items in payment slots 0 & 1 back to player inventory (9..45)
            let returned_a = return_stack_to_player_inv(inv, &mut slots[0]);
            let returned_b = if returned_a {
                return_stack_to_player_inv(inv, &mut slots[1])
            } else {
                false
            };

            // 2. If both payment slots are now empty, auto-fill matching items from player inventory
            if returned_a && returned_b && slots[0].is_empty() && slots[1].is_empty() {
                let offer = &offers[trade_idx as usize];
                move_matching_from_player_inv(inv, &mut slots[0], &offer.cost_a);
                if !offer.cost_b.is_empty() {
                    move_matching_from_player_inv(inv, &mut slots[1], &offer.cost_b);
                }
            }
        }

        slots[2] = compute_merchant_result(offers, *selected_trade, &slots[0], &slots[1]);
        inv.next_state_id();
        self.sync_packets(inv)
    }

    /// Drain and return all non-result input items remaining in transient workstation slots
    /// when the menu is closed, so the server can return them to the player or drop them.
    pub fn close_drops(&mut self) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        match self {
            Self::Enchanting { slots, offers, .. } => {
                for s in slots.iter_mut() {
                    if !s.is_empty() {
                        drops.push(std::mem::take(s));
                    }
                }
                *offers = [EnchantmentOffer::default(); 3];
            }
            Self::Anvil {
                slots,
                rename_text,
                xp_cost,
                repair_material_count,
                ..
            } => {
                for s in &mut slots[0..2] {
                    if !s.is_empty() {
                        drops.push(std::mem::take(s));
                    }
                }
                slots[2] = ItemStack::EMPTY;
                rename_text.clear();
                *xp_cost = 0;
                *repair_material_count = 0;
            }
            Self::BrewingStand { .. } => {
                // Persistent world block entity; nothing dropped on close.
            }
            Self::Smithing { slots, .. } => {
                for s in &mut slots[0..3] {
                    if !s.is_empty() {
                        drops.push(std::mem::take(s));
                    }
                }
                slots[3] = ItemStack::EMPTY;
            }
            Self::Stonecutter {
                slots,
                selected_recipe,
                ..
            } => {
                if !slots[0].is_empty() {
                    drops.push(std::mem::take(&mut slots[0]));
                }
                slots[1] = ItemStack::EMPTY;
                *selected_recipe = -1;
            }
            Self::Merchant { slots, .. } => {
                for s in &mut slots[0..2] {
                    if !s.is_empty() {
                        drops.push(std::mem::take(s));
                    }
                }
                slots[2] = ItemStack::EMPTY;
            }
        }
        drops
    }
}

// ============================================================================
// Standalone Brewing Stand Menu Helpers (world-backed BrewingStandState)
// ============================================================================

/// Build initial Play packets (`OpenScreen` + `ContainerSetContent` + 4 `ContainerSetData` values)
/// for opening a Brewing Stand backed by `state`.
pub fn brewing_open_packets(
    state: &BrewingStandState,
    container_id: i32,
    inv: &PlayerInventory,
) -> Vec<Vec<u8>> {
    let mut packets = Vec::with_capacity(6);
    if let Ok(open_pkt) =
        encode_play_open_screen(container_id, menu_types::BREWING_STAND, "Brewing Stand")
    {
        packets.push(open_pkt);
    }
    packets.extend(brewing_sync_packets(state, container_id, inv));
    packets
}

/// Build Play sync packets (`ContainerSetContent` + 4 Minecraft 26.3 `ContainerSetData` values)
/// for a Brewing Stand:
/// - `0`: `brew_time`
/// - `1`: `fuel`
/// - `2`: `total_brew_time` (`400`)
/// - `3`: `total_fuel` (`20`)
pub fn brewing_sync_packets(
    state: &BrewingStandState,
    container_id: i32,
    inv: &PlayerInventory,
) -> Vec<Vec<u8>> {
    let flat = assemble_flat_slots(&state.slots, inv);
    vec![
        encode_play_container_set_content(container_id, inv.state_id, &flat, &inv.carried),
        encode_play_container_set_data(container_id, 0, state.brew_time),
        encode_play_container_set_data(container_id, 1, state.fuel as i16),
        encode_play_container_set_data(container_id, 2, BREWING_DURATION_TICKS),
        encode_play_container_set_data(container_id, 3, BLAZE_POWDER_FUEL_CHARGES as i16),
    ]
}

/// Handle a `ServerboundContainerClickPacket` on a Brewing Stand menu backed by `state`.
///
/// Enforces vanilla slot constraints (0..=2 potion bottles max stack 1, 3 brewing reagents,
/// 4 blaze powder fuel) and keeps `state.bottle_potions` synchronized with slot contents.
pub fn brewing_handle_click(
    state: &mut BrewingStandState,
    container_id: i32,
    inv: &mut PlayerInventory,
    slot_num: i16,
    button: i8,
    click_type: ContainerClickType,
    is_creative: bool,
) -> MenuClickOutcome {
    let mut outcome = MenuClickOutcome::default();
    let mut flat = assemble_flat_slots(&state.slots, inv);
    let top_count = 5;
    let hotbar_start = top_count + 27;

    if let Some(dropped) = apply_slot_click(
        &mut flat,
        &mut inv.carried,
        slot_num,
        button,
        click_type,
        top_count,
        hotbar_start,
        None,
        is_creative,
        brewing_may_place,
        brewing_max_stack,
        brewing_quick_move,
    ) {
        outcome.dropped_items.push(dropped);
    }

    state.slots.clone_from_slice(&flat[..top_count]);
    for i in 0..3 {
        state.bottle_potions[i] = if state.slots[i].is_empty() {
            None
        } else {
            item_potion_id(&state.slots[i]).map(str::to_string)
        };
    }
    write_back_player_inv(inv, &flat[top_count..]);

    inv.next_state_id();
    outcome.packets = brewing_sync_packets(state, container_id, inv);
    outcome
}

// ============================================================================
// Merchant Protocol Conversion Helpers
// ============================================================================

/// Convert a server `MerchantTradeOffer` into a wire-encodable `mc_proto::MerchantOffer`.
pub fn trade_offer_to_proto(offer: &MerchantTradeOffer) -> MerchantOffer {
    let base_cost_a = ItemCost {
        item_id: offer.cost_a.item_id as i32,
        count: offer.cost_a.count as i32,
        components: offer.cost_a.added_components.clone(),
    };
    let cost_b = if offer.cost_b.is_empty() {
        None
    } else {
        Some(ItemCost {
            item_id: offer.cost_b.item_id as i32,
            count: offer.cost_b.count as i32,
            components: offer.cost_b.added_components.clone(),
        })
    };
    MerchantOffer {
        base_cost_a,
        result: offer.result.clone(),
        cost_b,
        uses: offer.uses,
        max_uses: offer.max_uses,
        xp: offer.xp,
        special_price_diff: 0,
        price_multiplier: offer.price_multiplier,
        demand: 0,
    }
}

/// Encode `ClientboundMerchantOffersPacket` from server `MerchantTradeOffer` entries.
pub fn build_merchant_offers_packet(
    container_id: i32,
    offers: &[MerchantTradeOffer],
    villager_level: i32,
    villager_xp: i32,
    show_progress: bool,
    can_restock: bool,
) -> Option<Vec<u8>> {
    let proto_offers: Vec<MerchantOffer> = offers
        .iter()
        .filter(|o| !o.result.is_empty() && !o.cost_a.is_empty())
        .map(trade_offer_to_proto)
        .collect();
    encode_play_merchant_offers(
        container_id,
        &proto_offers,
        villager_level,
        villager_xp,
        show_progress,
        can_restock,
    )
    .ok()
}

fn profession_title(profession: VillagerProfession) -> &'static str {
    match profession {
        VillagerProfession::Farmer => "Farmer",
        VillagerProfession::Librarian => "Librarian",
        VillagerProfession::Armorer => "Armorer",
        VillagerProfession::Toolsmith => "Toolsmith",
        VillagerProfession::Weaponsmith => "Weaponsmith",
        VillagerProfession::Cleric => "Cleric",
        VillagerProfession::Butcher => "Butcher",
        VillagerProfession::Mason => "Mason",
    }
}

// ============================================================================
// Internal Workstation Logic Helpers
// ============================================================================

fn assemble_flat_slots(top_slots: &[ItemStack], inv: &PlayerInventory) -> Vec<ItemStack> {
    let mut flat = Vec::with_capacity(top_slots.len() + 36);
    flat.extend_from_slice(top_slots);
    flat.extend_from_slice(&inv.slots[9..45]);
    flat
}

fn write_back_player_inv(inv: &mut PlayerInventory, inv_slice: &[ItemStack]) {
    if inv_slice.len() >= 36 {
        inv.slots[9..45].clone_from_slice(&inv_slice[..36]);
    }
}

#[inline]
fn can_merge_stacks(a: &ItemStack, b: &ItemStack) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.item_id == b.item_id
        && a.added_components == b.added_components
        && a.removed_components == b.removed_components
}

#[inline]
fn shrink_stack(stack: &mut ItemStack, amount: u8) {
    if stack.is_empty() || amount == 0 {
        return;
    }
    if stack.count <= amount {
        *stack = ItemStack::EMPTY;
    } else {
        stack.count -= amount;
    }
}

#[inline]
fn is_lapis_stack(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && item_by_id(stack.item_id)
            .map(|i| i.name == "minecraft:lapis_lazuli")
            .unwrap_or(false)
}

#[inline]
fn is_blaze_powder(item_id: u16) -> bool {
    item_id != 0
        && item_by_id(item_id)
            .map(|i| i.name == "minecraft:blaze_powder")
            .unwrap_or(false)
}

fn rotate_enchant_seed(current_seed: i32, item_id: u16, button_id: i32) -> i32 {
    let mut x = (current_seed as u32)
        .wrapping_mul(1_664_525)
        .wrapping_add(1_013_904_223)
        ^ ((item_id as u32).wrapping_mul(0x9E37_79B9))
        ^ ((button_id as u32 + 1).wrapping_mul(0x85EB_CA6B));
    x ^= x >> 16;
    let next = x as i32;
    if next == current_seed {
        next.wrapping_add(1)
    } else {
        next
    }
}

fn recompute_enchanting_offers(
    item: &ItemStack,
    seed: i32,
    bookshelves: u8,
    offers: &mut [EnchantmentOffer; 3],
) {
    if item.is_empty()
        || item_enchantability(item.item_id) == 0
        || !read_item_enchantments(item).is_empty()
    {
        *offers = [EnchantmentOffer::default(); 3];
    } else {
        *offers = compute_enchanting_offers(seed, item.item_id, bookshelves);
    }
}

fn filter_anvil_name(raw: &str) -> String {
    raw.chars()
        .filter(|&c| c >= ' ' && c != '\u{007f}' && c != '§')
        .collect()
}

fn recompute_anvil(
    slots: &mut [ItemStack; 3],
    rename_text: &str,
    xp_cost: &mut i32,
    repair_material_count: &mut u8,
    allow_clear_name: bool,
) {
    if slots[0].is_empty() {
        slots[2] = ItemStack::EMPTY;
        *xp_cost = 0;
        *repair_material_count = 0;
        return;
    }

    let rename_arg = if !rename_text.is_empty() {
        Some(rename_text)
    } else if allow_clear_name && item_custom_name(&slots[0]).is_some() {
        Some("")
    } else {
        None
    };

    let left_ench = read_item_enchantments(&slots[0]);
    let right_ench = read_item_enchantments(&slots[1]);

    if let Some((mut res, combined_ench)) =
        compute_anvil_output(&slots[0], &slots[1], rename_arg, &left_ench, &right_ench)
    {
        if !combined_ench.is_empty() {
            write_item_enchantments(&mut res.output, &combined_ench);
        }
        slots[2] = res.output;
        *xp_cost = res.xp_cost;
        *repair_material_count = res.repair_items_used;
    } else {
        slots[2] = ItemStack::EMPTY;
        *xp_cost = 0;
        *repair_material_count = 0;
    }
}

fn anvil_damage_roll() -> bool {
    let v = (rand::thread_rng().next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32);
    v < 0.12
}

fn stonecutter_preview(input: &ItemStack, selected_recipe: i32) -> Option<ItemStack> {
    if input.is_empty() || selected_recipe < 0 {
        return None;
    }
    let recipes = stonecutting_for_input(input.item_id);
    let recipe = recipes.get(selected_recipe as usize)?;
    Some(ItemStack::new(recipe.result_item, recipe.result_count))
}

fn offer_matches_payment_items(
    offer: &MerchantTradeOffer,
    pay_a: &ItemStack,
    pay_b: &ItemStack,
) -> bool {
    if offer.cost_a.is_empty() || offer.result.is_empty() {
        return false;
    }
    if pay_a.is_empty() || pay_a.item_id != offer.cost_a.item_id || pay_a.count < offer.cost_a.count
    {
        return false;
    }
    if offer.cost_b.is_empty() {
        pay_b.is_empty()
    } else {
        !pay_b.is_empty()
            && pay_b.item_id == offer.cost_b.item_id
            && pay_b.count >= offer.cost_b.count
    }
}

/// Resolve the active merchant offer index and whether payment slots `(0, 1)` are matched in swapped order.
fn find_active_merchant_offer(
    offers: &[MerchantTradeOffer],
    selected_trade: i32,
    slot0: &ItemStack,
    slot1: &ItemStack,
) -> Option<(usize, bool)> {
    if slot0.is_empty() && slot1.is_empty() {
        return None;
    }
    let check_pair = |offer: &MerchantTradeOffer| -> Option<bool> {
        if offer_matches_payment_items(offer, slot0, slot1) {
            Some(false)
        } else if offer_matches_payment_items(offer, slot1, slot0) {
            Some(true)
        } else {
            None
        }
    };

    // Prioritize selected trade if payment slots match it (even if out of stock, do not silently
    // substitute a different same-cost offer).
    if selected_trade >= 0 {
        if let Some(offer) = offers.get(selected_trade as usize) {
            if let Some(swapped) = check_pair(offer) {
                return if offer.is_out_of_stock() {
                    None
                } else {
                    Some((selected_trade as usize, swapped))
                };
            }
        }
    }

    for (idx, offer) in offers.iter().enumerate() {
        if !offer.is_out_of_stock() {
            if let Some(swapped) = check_pair(offer) {
                return Some((idx, swapped));
            }
        }
    }
    None
}

fn compute_merchant_result(
    offers: &[MerchantTradeOffer],
    selected_trade: i32,
    slot0: &ItemStack,
    slot1: &ItemStack,
) -> ItemStack {
    find_active_merchant_offer(offers, selected_trade, slot0, slot1)
        .map(|(idx, _)| offers[idx].result.clone())
        .unwrap_or(ItemStack::EMPTY)
}

fn return_stack_to_player_inv(inv: &mut PlayerInventory, stack: &mut ItemStack) -> bool {
    if stack.is_empty() {
        return true;
    }
    let max_st = stack.max_stack_size();
    // 1. Merge into existing stacks in hotbar (36..45) then main (9..36)
    for idx in (36..45).chain(9..36) {
        if stack.is_empty() {
            return true;
        }
        if can_merge_stacks(&inv.slots[idx], stack) {
            let space = max_st.saturating_sub(inv.slots[idx].count);
            if space > 0 {
                let take = stack.count.min(space);
                inv.slots[idx].count += take;
                shrink_stack(stack, take);
            }
        }
    }
    // 2. Place into empty slots in hotbar (36..45) then main (9..36)
    for idx in (36..45).chain(9..36) {
        if stack.is_empty() {
            return true;
        }
        if inv.slots[idx].is_empty() {
            inv.slots[idx] = std::mem::take(stack);
            return true;
        }
    }
    stack.is_empty()
}

fn move_matching_from_player_inv(
    inv: &mut PlayerInventory,
    payment_slot: &mut ItemStack,
    required: &ItemStack,
) {
    if required.is_empty() {
        return;
    }
    // Scan player main inventory (9..36) then hotbar (36..45), matching MerchantMenu.moveFromInventoryToPaymentSlot
    for idx in 9..45 {
        if inv.slots[idx].is_empty() || inv.slots[idx].item_id != required.item_id {
            continue;
        }
        if !required.added_components.is_empty()
            && inv.slots[idx].added_components != required.added_components
        {
            continue;
        }
        if !payment_slot.is_empty() && !can_merge_stacks(&inv.slots[idx], payment_slot) {
            continue;
        }
        let max_st = inv.slots[idx].max_stack_size();
        let space = max_st.saturating_sub(payment_slot.count);
        if space == 0 {
            break;
        }
        let take = space.min(inv.slots[idx].count);
        if payment_slot.is_empty() {
            let mut moved = inv.slots[idx].clone();
            moved.count = take;
            *payment_slot = moved;
        } else {
            payment_slot.count += take;
        }
        shrink_stack(&mut inv.slots[idx], take);
        if payment_slot.count >= max_st {
            break;
        }
    }
}

// ============================================================================
// Per-Workstation Slot Policies (`may_place`, `max_stack`, `quick_move`)
// ============================================================================

fn enchanting_may_place(slot_idx: usize, stack: &ItemStack) -> bool {
    match slot_idx {
        0 => true,
        1 => is_lapis_stack(stack),
        _ => true,
    }
}

fn enchanting_max_stack(slot_idx: usize, stack: &ItemStack) -> u8 {
    match slot_idx {
        0 => 1,
        _ => stack.max_stack_size(),
    }
}

fn enchanting_quick_move(slots: &mut [ItemStack], idx: usize, top_count: usize, hotbar_start: usize) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx < top_count {
        move_into_range(slots, &mut moving, top_count..inv_end, true, None);
    } else if is_lapis_stack(&moving) {
        move_into_range(slots, &mut moving, 1..2, false, None);
    } else if slots[0].is_empty() && !moving.is_empty() {
        let mut single = moving.clone();
        single.count = 1;
        slots[0] = single;
        shrink_stack(&mut moving, 1);
    }
    slots[idx] = moving;
}

fn anvil_quick_move(slots: &mut [ItemStack], idx: usize, top_count: usize, hotbar_start: usize) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx < top_count {
        move_into_range(slots, &mut moving, top_count..inv_end, false, None);
    } else {
        move_into_range(slots, &mut moving, 0..2, false, None);
        if !moving.is_empty() {
            quick_move_player_internal(slots, &mut moving, idx, top_count, hotbar_start);
        }
    }
    slots[idx] = moving;
}

fn brewing_may_place(slot_idx: usize, stack: &ItemStack) -> bool {
    match slot_idx {
        0..=2 => is_brewing_potion_input(stack.item_id),
        3 => is_brewing_reagent(stack.item_id),
        4 => is_blaze_powder(stack.item_id),
        _ => true,
    }
}

fn brewing_max_stack(slot_idx: usize, stack: &ItemStack) -> u8 {
    match slot_idx {
        0..=2 => 1,
        _ => stack.max_stack_size(),
    }
}

fn brewing_quick_move(slots: &mut [ItemStack], idx: usize, top_count: usize, hotbar_start: usize) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx < top_count {
        move_into_range(slots, &mut moving, top_count..inv_end, true, None);
    } else if is_blaze_powder(moving.item_id) {
        move_into_range(slots, &mut moving, 4..5, false, None);
        if !moving.is_empty() && is_brewing_reagent(moving.item_id) {
            move_into_range(slots, &mut moving, 3..4, false, None);
        }
    } else if is_brewing_reagent(moving.item_id) {
        move_into_range(slots, &mut moving, 3..4, false, None);
    } else if is_brewing_potion_input(moving.item_id) {
        move_into_range(slots, &mut moving, 0..3, false, Some(1));
    } else {
        quick_move_player_internal(slots, &mut moving, idx, top_count, hotbar_start);
    }
    slots[idx] = moving;
}

#[inline]
fn is_smithing_template(item_id: u16) -> bool {
    item_id != 0
        && (SMITHING_RECIPES.iter().any(|r| r.template_item == item_id)
            || SMITHING_TRIM_TEMPLATES
                .binary_search_by_key(&item_id, |(id, _)| *id)
                .is_ok())
}

#[inline]
fn is_smithing_base(item_id: u16) -> bool {
    item_id != 0
        && (SMITHING_RECIPES.iter().any(|r| r.base_item == item_id)
            || SMITHING_TRIMMABLE_ARMOR.binary_search(&item_id).is_ok())
}

#[inline]
fn is_smithing_addition(item_id: u16) -> bool {
    item_id != 0
        && (SMITHING_RECIPES.iter().any(|r| r.addition_item == item_id)
            || SMITHING_TRIM_MATERIALS.binary_search(&item_id).is_ok())
}

fn smithing_may_place(slot_idx: usize, stack: &ItemStack) -> bool {
    match slot_idx {
        0 => is_smithing_template(stack.item_id),
        1 => is_smithing_base(stack.item_id),
        2 => is_smithing_addition(stack.item_id),
        3 => false,
        _ => true,
    }
}

fn smithing_quick_move(slots: &mut [ItemStack], idx: usize, top_count: usize, hotbar_start: usize) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx < 3 {
        move_into_range(slots, &mut moving, top_count..inv_end, false, None);
    } else if idx >= top_count {
        if is_smithing_template(moving.item_id) && slots[0].is_empty() {
            move_into_range(slots, &mut moving, 0..1, false, None);
        } else if is_smithing_base(moving.item_id) && slots[1].is_empty() {
            move_into_range(slots, &mut moving, 1..2, false, None);
        } else if is_smithing_addition(moving.item_id) && slots[2].is_empty() {
            move_into_range(slots, &mut moving, 2..3, false, None);
        } else {
            quick_move_player_internal(slots, &mut moving, idx, top_count, hotbar_start);
        }
    }
    slots[idx] = moving;
}

fn stonecutter_quick_move(
    slots: &mut [ItemStack],
    idx: usize,
    top_count: usize,
    hotbar_start: usize,
) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx == 0 {
        move_into_range(slots, &mut moving, top_count..inv_end, false, None);
    } else if idx >= top_count {
        if !stonecutting_for_input(moving.item_id).is_empty() {
            move_into_range(slots, &mut moving, 0..1, false, None);
        } else {
            quick_move_player_internal(slots, &mut moving, idx, top_count, hotbar_start);
        }
    }
    slots[idx] = moving;
}

fn merchant_quick_move(slots: &mut [ItemStack], idx: usize, top_count: usize, hotbar_start: usize) {
    let inv_end = hotbar_start + 9;
    let mut moving = std::mem::take(&mut slots[idx]);
    if idx < 2 {
        move_into_range(slots, &mut moving, top_count..inv_end, false, None);
    } else if idx >= top_count {
        quick_move_player_internal(slots, &mut moving, idx, top_count, hotbar_start);
    }
    slots[idx] = moving;
}

fn quick_move_player_internal(
    slots: &mut [ItemStack],
    moving: &mut ItemStack,
    idx: usize,
    top_count: usize,
    hotbar_start: usize,
) {
    let inv_end = (hotbar_start + 9).min(slots.len());
    if idx >= hotbar_start && idx < inv_end {
        move_into_range(slots, moving, top_count..hotbar_start, false, None);
    } else if idx >= top_count && idx < hotbar_start {
        move_into_range(slots, moving, hotbar_start..inv_end, false, None);
    }
}

// ============================================================================
// Core Slot-Click & Result-Slot Mechanics
// ============================================================================

fn can_fit_entire_stack(slots: &[ItemStack], range: Range<usize>, stack: &ItemStack) -> bool {
    if stack.is_empty() {
        return true;
    }
    let max_st = stack.max_stack_size();
    let mut needed = stack.count as u32;
    for idx in range {
        if slots[idx].is_empty() {
            return true;
        }
        if can_merge_stacks(&slots[idx], stack) {
            let space = max_st.saturating_sub(slots[idx].count) as u32;
            if space >= needed {
                return true;
            }
            needed -= space;
        }
    }
    false
}

fn move_into_range(
    slots: &mut [ItemStack],
    stack: &mut ItemStack,
    range: Range<usize>,
    reverse: bool,
    max_per_slot: Option<u8>,
) {
    if stack.is_empty() {
        return;
    }
    let max_st = max_per_slot
        .map(|cap| cap.min(stack.max_stack_size()))
        .unwrap_or_else(|| stack.max_stack_size());
    let indices: Vec<usize> = if reverse {
        range.rev().collect()
    } else {
        range.collect()
    };

    // 1. Merge into existing matching stacks
    if max_st > 1 {
        for &idx in &indices {
            if stack.is_empty() {
                return;
            }
            if can_merge_stacks(&slots[idx], stack) {
                let space = max_st.saturating_sub(slots[idx].count);
                if space > 0 {
                    let take = stack.count.min(space);
                    slots[idx].count += take;
                    shrink_stack(stack, take);
                }
            }
        }
    }

    // 2. Place into empty slots
    for &idx in &indices {
        if stack.is_empty() {
            return;
        }
        if slots[idx].is_empty() {
            let take = stack.count.min(max_st);
            let mut placed = stack.clone();
            placed.count = take;
            slots[idx] = placed;
            shrink_stack(stack, take);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_slot_click(
    slots: &mut [ItemStack],
    carried: &mut ItemStack,
    slot_num: i16,
    button: i8,
    click_type: ContainerClickType,
    top_count: usize,
    hotbar_start: usize,
    result_slot: Option<usize>,
    is_creative: bool,
    may_place: fn(usize, &ItemStack) -> bool,
    max_stack_for_slot: fn(usize, &ItemStack) -> u8,
    quick_move_fn: fn(&mut [ItemStack], usize, usize, usize),
) -> Option<ItemStack> {
    // Outside-window drop (-999)
    if slot_num == -999 {
        if click_type == ContainerClickType::Pickup && !carried.is_empty() {
            if button == 0 {
                return Some(std::mem::take(carried));
            } else if button == 1 {
                let mut one = carried.clone();
                one.count = 1;
                shrink_stack(carried, 1);
                return Some(one);
            }
        }
        return None;
    }

    if slot_num < 0 || (slot_num as usize) >= slots.len() {
        return None;
    }
    let idx = slot_num as usize;

    match click_type {
        ContainerClickType::Pickup => {
            if button == 0 {
                if carried.is_empty() {
                    std::mem::swap(&mut slots[idx], carried);
                } else if may_place(idx, carried) {
                    let max_st = max_stack_for_slot(idx, carried);
                    if slots[idx].is_empty() {
                        let take = carried.count.min(max_st);
                        let mut placed = carried.clone();
                        placed.count = take;
                        slots[idx] = placed;
                        shrink_stack(carried, take);
                    } else if can_merge_stacks(&slots[idx], carried) {
                        let space = max_st.saturating_sub(slots[idx].count);
                        let take = carried.count.min(space);
                        slots[idx].count += take;
                        shrink_stack(carried, take);
                    } else if carried.count <= max_st {
                        std::mem::swap(&mut slots[idx], carried);
                    }
                }
            } else if button == 1 {
                if carried.is_empty() {
                    if !slots[idx].is_empty() {
                        let take = slots[idx].count.div_ceil(2);
                        let mut taken = slots[idx].clone();
                        taken.count = take;
                        shrink_stack(&mut slots[idx], take);
                        *carried = taken;
                    }
                } else if may_place(idx, carried) {
                    let max_st = max_stack_for_slot(idx, carried);
                    if slots[idx].is_empty() {
                        if max_st >= 1 {
                            let mut one = carried.clone();
                            one.count = 1;
                            slots[idx] = one;
                            shrink_stack(carried, 1);
                        }
                    } else if can_merge_stacks(&slots[idx], carried) && slots[idx].count < max_st {
                        slots[idx].count += 1;
                        shrink_stack(carried, 1);
                    } else if carried.count <= max_st {
                        std::mem::swap(&mut slots[idx], carried);
                    }
                }
            }
        }
        ContainerClickType::QuickMove => {
            if !slots[idx].is_empty() {
                quick_move_fn(slots, idx, top_count, hotbar_start);
            }
        }
        ContainerClickType::Swap => {
            if (0..=8).contains(&button) {
                let hb = hotbar_start + button as usize;
                if hb < slots.len() {
                    let allowed = if idx < top_count && !slots[hb].is_empty() {
                        may_place(idx, &slots[hb])
                            && slots[hb].count <= max_stack_for_slot(idx, &slots[hb])
                    } else {
                        true
                    };
                    if allowed {
                        slots.swap(idx, hb);
                    }
                }
            }
        }
        ContainerClickType::Clone => {
            if is_creative && carried.is_empty() && !slots[idx].is_empty() {
                let mut cloned = slots[idx].clone();
                cloned.count = cloned.max_stack_size();
                *carried = cloned;
            }
        }
        ContainerClickType::Throw => {
            if carried.is_empty() && !slots[idx].is_empty() {
                if button == 1 {
                    return Some(std::mem::take(&mut slots[idx]));
                } else {
                    let mut one = slots[idx].clone();
                    one.count = 1;
                    shrink_stack(&mut slots[idx], 1);
                    return Some(one);
                }
            }
        }
        ContainerClickType::PickupAll => {
            if !carried.is_empty() {
                let max_st = carried.max_stack_size();
                for (i, s) in slots.iter_mut().enumerate() {
                    if Some(i) == result_slot {
                        continue;
                    }
                    if carried.count >= max_st {
                        break;
                    }
                    if can_merge_stacks(carried, s) {
                        let take = (max_st - carried.count).min(s.count);
                        carried.count += take;
                        shrink_stack(s, take);
                    }
                }
            }
        }
        ContainerClickType::QuickCraft => {}
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn handle_result_slot_click<C, FCan, FPeek, FTake>(
    flat: &mut [ItemStack],
    carried: &mut ItemStack,
    button: i8,
    click_type: ContainerClickType,
    top_count: usize,
    hotbar_start: usize,
    is_creative: bool,
    allow_multi_quick_move: bool,
    outcome: &mut MenuClickOutcome,
    ctx: &mut C,
    mut can_take: FCan,
    mut peek_result: FPeek,
    mut consume_and_take: FTake,
) where
    FCan: FnMut(&C, &[ItemStack]) -> bool,
    FPeek: FnMut(&C, &[ItemStack]) -> Option<ItemStack>,
    FTake: FnMut(&mut C, &mut [ItemStack], &mut MenuClickOutcome) -> Option<ItemStack>,
{
    match click_type {
        ContainerClickType::Pickup => {
            if !can_take(ctx, flat) {
                return;
            }
            let Some(preview) = peek_result(ctx, flat) else {
                return;
            };
            if carried.is_empty() {
                if let Some(taken) = consume_and_take(ctx, flat, outcome) {
                    *carried = taken;
                }
            } else if can_merge_stacks(carried, &preview)
                && carried.count.saturating_add(preview.count) <= preview.max_stack_size()
            {
                if let Some(taken) = consume_and_take(ctx, flat, outcome) {
                    carried.count += taken.count;
                }
            }
        }
        ContainerClickType::QuickMove => {
            let inv_range = top_count..(hotbar_start + 9).min(flat.len());
            let max_iters = if allow_multi_quick_move { 64 } else { 1 };
            for _ in 0..max_iters {
                if !can_take(ctx, flat) {
                    break;
                }
                let Some(preview) = peek_result(ctx, flat) else {
                    break;
                };
                if !can_fit_entire_stack(flat, inv_range.clone(), &preview) {
                    break;
                }
                let Some(mut taken) = consume_and_take(ctx, flat, outcome) else {
                    break;
                };
                move_into_range(flat, &mut taken, inv_range.clone(), true, None);
            }
        }
        ContainerClickType::Swap => {
            if carried.is_empty() && (0..=8).contains(&button) && can_take(ctx, flat) {
                let hb = hotbar_start + button as usize;
                if hb < flat.len() && flat[hb].is_empty() {
                    if let Some(taken) = consume_and_take(ctx, flat, outcome) {
                        flat[hb] = taken;
                    }
                }
            }
        }
        ContainerClickType::Throw => {
            if carried.is_empty() && can_take(ctx, flat) {
                if let Some(taken) = consume_and_take(ctx, flat, outcome) {
                    outcome.dropped_items.push(taken);
                }
            }
        }
        ContainerClickType::Clone => {
            if is_creative && carried.is_empty() {
                if let Some(mut preview) = peek_result(ctx, flat) {
                    preview.count = preview.max_stack_size();
                    *carried = preview;
                }
            }
        }
        ContainerClickType::PickupAll | ContainerClickType::QuickCraft => {}
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::packet_ids::play::clientbound as cb;
    use mc_data::{BlockStateId, default_state_id};
    use mc_proto::PacketReader;
    use mc_world::{item_damage, item_repair_cost, make_potion_stack, set_item_damage};
    use std::collections::HashMap;

    struct TestBlockWorld {
        blocks: HashMap<BlockPos, BlockStateId>,
    }

    impl TestBlockWorld {
        fn new() -> Self {
            Self {
                blocks: HashMap::new(),
            }
        }
    }

    impl BlockWorld for TestBlockWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.blocks.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }

        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            self.blocks.insert(pos, state);
            true
        }
    }

    fn packet_id(pkt: &[u8]) -> i32 {
        let mut r = PacketReader::new(pkt);
        r.read_varint().expect("varint packet id")
    }

    fn read_container_set_data(pkt: &[u8]) -> Option<(i32, i16, i16)> {
        let mut r = PacketReader::new(pkt);
        let id = r.read_varint().ok()?;
        if id != cb::CONTAINER_SET_DATA {
            return None;
        }
        let cid = r.read_varint().ok()?;
        let data_id = r.read_i16().ok()?;
        let value = r.read_i16().ok()?;
        Some((cid, data_id, value))
    }

    fn read_open_screen(pkt: &[u8]) -> Option<(i32, i32)> {
        let mut r = PacketReader::new(pkt);
        let id = r.read_varint().ok()?;
        if id != cb::OPEN_SCREEN {
            return None;
        }
        let cid = r.read_varint().ok()?;
        let menu_type = r.read_varint().ok()?;
        Some((cid, menu_type))
    }

    #[test]
    fn bookshelf_scanner_respects_offsets_obstructions_and_15_cap() {
        let mut world = TestBlockWorld::new();
        let center = BlockPos::new(10, 64, 10);
        let bookshelf = default_state_id("minecraft:bookshelf").unwrap();
        let torch = default_state_id("minecraft:torch").unwrap();
        let short_grass = default_state_id("minecraft:short_grass").unwrap();

        assert_eq!(count_bookshelves_around(&mut world, center), 0);

        // Place 16 bookshelves around the 5x2 perimeter at dy=0
        let mut placed = 0;
        for dx in -2_i32..=2 {
            for dz in -2_i32..=2 {
                if dx.abs() == 2 || dz.abs() == 2 {
                    world.set_block_state(center.offset(dx, 0, dz), bookshelf);
                    placed += 1;
                }
            }
        }
        assert_eq!(placed, 16);
        // Capped at 15 even with 16 valid bookshelves!
        assert_eq!(count_bookshelves_around(&mut world, center), 15);

        // Remove 2 bookshelves so only 14 remain
        world.set_block_state(center.offset(-2, 0, -2), BlockStateId::AIR);
        world.set_block_state(center.offset(-2, 0, -1), BlockStateId::AIR);
        assert_eq!(count_bookshelves_around(&mut world, center), 14);

        // Blocking the intermediate cell (1, 0, 0) with a torch blocks (2, 0, -1), (2, 0, 0), and (2, 0, 1)!
        world.set_block_state(center.offset(1, 0, 0), torch);
        assert_eq!(count_bookshelves_around(&mut world, center), 11);

        // Replaceable blocks (like short_grass) still transmit enchantment power in 26.3
        world.set_block_state(center.offset(1, 0, 0), short_grass);
        assert_eq!(count_bookshelves_around(&mut world, center), 14);
    }

    #[test]
    fn enchanting_menu_offers_and_button_click_deducts_lapis_and_1_to_3_levels() {
        let mut inv = PlayerInventory::new();
        // Hotbar slot 0 = inv.slots[36] (menu slot 29 in Enchanting), slot 1 = inv.slots[37] (menu slot 30)
        inv.slots[36] = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        inv.slots[37] = ItemStack::from_name("minecraft:lapis_lazuli", 5).unwrap();

        let mut menu = WorkstationMenu::new_enchanting(1, BlockPos::new(0, 64, 0), 12345, 15);
        let open = menu.open_packets(&inv);
        assert_eq!(read_open_screen(&open[0]), Some((1, menu_types::ENCHANTMENT)));
        assert_eq!(packet_id(&open[1]), cb::CONTAINER_SET_CONTENT);
        // 10 ContainerSetData packets (costs 0..2, seed 3, clue ids 4..6, clue levels 7..9)
        let data_entries: Vec<_> = open.iter().filter_map(|p| read_container_set_data(p)).collect();
        assert_eq!(data_entries.len(), 10);

        // Quick-move sword into slot 0 and lapis into slot 1
        let _ = menu.handle_click(&mut inv, 29, 0, ContainerClickType::QuickMove, 30, false);
        let out_lapis =
            menu.handle_click(&mut inv, 30, 0, ContainerClickType::QuickMove, 30, false);

        // Verify offers populated with level 30 in slot 2 and synced via ContainerSetData
        let cost_slot2 = out_lapis
            .packets
            .iter()
            .filter_map(|p| read_container_set_data(p))
            .find(|&(_, id, _)| id == 2)
            .map(|(_, _, v)| v)
            .unwrap();
        assert_eq!(cost_slot2, 30);

        // Insufficient XP (20 < 30) must reject button 2
        let rejected = menu.handle_button_click(&mut inv, 2, 20, false);
        assert!(rejected.packets.is_empty());
        assert_eq!(rejected.xp_level_delta, 0);

        // Level 30 player clicks bottom offer (button 2): requires 30, deducts ONLY 3 levels + 3 lapis!
        let ench_out = menu.handle_button_click(&mut inv, 2, 30, false);
        assert_eq!(ench_out.xp_level_delta, -3);
        assert!(ench_out.new_enchant_seed.is_some());
        assert_ne!(ench_out.new_enchant_seed.unwrap(), 12345);
        assert!(!ench_out.packets.is_empty());

        if let WorkstationMenu::Enchanting { slots, offers, .. } = &menu {
            assert_eq!(slots[1].count, 2); // 5 - 3 = 2 lapis remaining
            let enchants = read_item_enchantments(&slots[0]);
            assert!(!enchants.is_empty(), "diamond sword should now have wire enchantments");
            // Already-enchanted item should have cleared offers
            assert!(offers.iter().all(|o| o.xp_cost == 0));
        } else {
            panic!("expected enchanting menu");
        }

        // Book enchanting transmutes minecraft:book -> minecraft:enchanted_book with STORED_ENCHANTMENTS
        let mut book_menu = WorkstationMenu::new_enchanting(2, BlockPos::new(0, 64, 0), 777, 15);
        inv.slots[38] = ItemStack::from_name("minecraft:book", 3).unwrap();
        inv.slots[39] = ItemStack::from_name("minecraft:lapis_lazuli", 3).unwrap();
        let _ = book_menu.handle_click(&mut inv, 31, 0, ContainerClickType::QuickMove, 30, false);
        let _ = book_menu.handle_click(&mut inv, 32, 0, ContainerClickType::QuickMove, 30, false);
        assert_eq!(inv.slots[38].count, 2, "slot 0 must take only 1 book from stack");
        let book_out = book_menu.handle_button_click(&mut inv, 0, 30, false);
        assert_eq!(book_out.xp_level_delta, -1);
        if let WorkstationMenu::Enchanting { slots, .. } = &book_menu {
            assert_eq!(
                item_by_id(slots[0].item_id).unwrap().name,
                "minecraft:enchanted_book"
            );
            assert!(!read_item_enchantments(&slots[0]).is_empty());
        }
    }

    #[test]
    fn anvil_rename_book_combine_and_take_result() {
        let mut inv = PlayerInventory::new();
        let mut sword = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        set_item_damage(&mut sword, 100);
        write_item_enchantments(&mut sword, &[("minecraft:sharpness".into(), 3)]);

        let mut book = ItemStack::from_name("minecraft:enchanted_book", 1).unwrap();
        write_item_enchantments(&mut book, &[("minecraft:sharpness".into(), 3)]);

        // Anvil hotbar starts at slot 30 (inv.slots[36])
        inv.slots[36] = sword;
        inv.slots[37] = book;

        let mut menu = WorkstationMenu::new_anvil(2, BlockPos::new(4, 64, 4));
        let open = menu.open_packets(&inv);
        assert_eq!(read_open_screen(&open[0]), Some((2, menu_types::ANVIL)));

        // Move sword into slot 0 and book into slot 1
        let _ = menu.handle_click(&mut inv, 30, 0, ContainerClickType::QuickMove, 10, false);
        let _ = menu.handle_click(&mut inv, 31, 0, ContainerClickType::QuickMove, 10, false);

        // Send RenameItem -> "Excalibur"
        let rename_pkts = menu.handle_rename_item(&inv, "Excalibur".into());
        let cost_data = rename_pkts
            .iter()
            .filter_map(|p| read_container_set_data(p))
            .find(|&(_, id, _)| id == 0)
            .map(|(_, _, v)| v)
            .unwrap();
        assert_eq!(cost_data, 5); // Sharpness IV (1*4) + rename (1)

        // Insufficient XP (2 < 5) must NOT take result
        let fail_take = menu.handle_click(&mut inv, 2, 0, ContainerClickType::Pickup, 2, false);
        assert!(inv.carried.is_empty());
        assert_eq!(fail_take.xp_level_delta, 0);

        // Sufficient XP (10 >= 5) takes result onto cursor, deducts 5 levels, clears inputs
        let ok_take = menu.handle_click(&mut inv, 2, 0, ContainerClickType::Pickup, 10, false);
        assert_eq!(ok_take.xp_level_delta, -5);
        assert_eq!(item_custom_name(&inv.carried).as_deref(), Some("Excalibur"));
        assert_eq!(item_damage(&inv.carried), 100);
        assert_eq!(item_repair_cost(&inv.carried), 1);
        assert!(
            read_item_enchantments(&inv.carried)
                .contains(&("minecraft:sharpness".to_string(), 4))
        );

        if let WorkstationMenu::Anvil { slots, xp_cost, .. } = &menu {
            assert!(slots[0].is_empty());
            assert!(slots[1].is_empty());
            assert!(slots[2].is_empty());
            assert_eq!(*xp_cost, 0);
        }
    }

    #[test]
    fn brewing_stand_open_sync_and_slot_click_rules() {
        let mut inv = PlayerInventory::new();
        let mut stand = BrewingStandState::new();
        let water = make_potion_stack("minecraft:potion", "minecraft:water").unwrap();
        // Brewing stand hotbar starts at slot 32 (inv.slots[36])
        inv.slots[36] = water;
        inv.slots[37] = ItemStack::from_name("minecraft:nether_wart", 4).unwrap();
        inv.slots[38] = ItemStack::from_name("minecraft:blaze_powder", 2).unwrap();
        inv.slots[39] = ItemStack::from_name("minecraft:cobblestone", 16).unwrap();

        let open = brewing_open_packets(&stand, 3, &inv);
        assert_eq!(read_open_screen(&open[0]), Some((3, menu_types::BREWING_STAND)));
        let data_entries: Vec<_> = open.iter().filter_map(|p| read_container_set_data(p)).collect();
        assert_eq!(
            data_entries,
            vec![(3, 0, 0), (3, 1, 0), (3, 2, 400), (3, 3, 20)]
        );

        // Quick-move water -> bottle slot 0, nether wart -> slot 3, blaze powder -> slot 4
        let _ = brewing_handle_click(&mut stand, 3, &mut inv, 32, 0, ContainerClickType::QuickMove, false);
        let _ = brewing_handle_click(&mut stand, 3, &mut inv, 33, 0, ContainerClickType::QuickMove, false);
        let _ = brewing_handle_click(&mut stand, 3, &mut inv, 34, 0, ContainerClickType::QuickMove, false);

        assert_eq!(stand.bottle_potion(0), Some("minecraft:water"));
        assert_eq!(stand.slots[3].count, 4);
        assert_eq!(stand.slots[4].count, 2);

        // Cobblestone cannot be placed into fuel slot 4 or bottle slot 1
        let _ = brewing_handle_click(&mut stand, 3, &mut inv, 35, 0, ContainerClickType::Pickup, false);
        assert_eq!(inv.carried.count, 16);
        let _ = brewing_handle_click(&mut stand, 3, &mut inv, 1, 0, ContainerClickType::Pickup, false);
        assert!(stand.slots[1].is_empty());
        assert_eq!(inv.carried.count, 16);

        // Tick 1 consumes 1 blaze powder and starts brewing -> sync packets report brew_time=400, fuel=19
        assert!(stand.tick_brewing());
        let sync = brewing_sync_packets(&stand, 3, &inv);
        let synced_data: Vec<_> = sync.iter().filter_map(|p| read_container_set_data(p)).collect();
        assert_eq!(synced_data[0], (3, 0, 400));
        assert_eq!(synced_data[1], (3, 1, 19));
    }

    #[test]
    fn smithing_netherite_upgrade_preserves_enchantments_and_consumes_inputs() {
        let mut inv = PlayerInventory::new();
        let mut sword = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        write_item_enchantments(&mut sword, &[("minecraft:looting".into(), 3)]);
        // Smithing hotbar starts at slot 31 (inv.slots[36])
        inv.slots[36] =
            ItemStack::from_name("minecraft:netherite_upgrade_smithing_template", 1).unwrap();
        inv.slots[37] = sword;
        inv.slots[38] = ItemStack::from_name("minecraft:netherite_ingot", 2).unwrap();

        let mut menu = WorkstationMenu::new_smithing(4);
        let open = menu.open_packets(&inv);
        assert_eq!(read_open_screen(&open[0]), Some((4, menu_types::SMITHING)));

        // Quick-move routes template -> 0, diamond sword -> 1, netherite ingot -> 2
        let _ = menu.handle_click(&mut inv, 31, 0, ContainerClickType::QuickMove, 0, false);
        let _ = menu.handle_click(&mut inv, 32, 0, ContainerClickType::QuickMove, 0, false);
        let _ = menu.handle_click(&mut inv, 33, 0, ContainerClickType::QuickMove, 0, false);

        if let WorkstationMenu::Smithing { slots, .. } = &menu {
            assert_eq!(
                item_by_id(slots[3].item_id).unwrap().name,
                "minecraft:netherite_sword"
            );
        }

        // Take result slot 3 onto cursor
        let _ = menu.handle_click(&mut inv, 3, 0, ContainerClickType::Pickup, 0, false);
        assert_eq!(
            item_by_id(inv.carried.item_id).unwrap().name,
            "minecraft:netherite_sword"
        );
        assert!(
            read_item_enchantments(&inv.carried)
                .contains(&("minecraft:looting".to_string(), 3))
        );

        if let WorkstationMenu::Smithing { slots, .. } = &menu {
            assert!(slots[0].is_empty());
            assert!(slots[1].is_empty());
            assert_eq!(slots[2].count, 1); // 2 - 1 = 1 netherite ingot left
            assert!(slots[3].is_empty());
        }
    }

    #[test]
    fn stonecutter_button_select_and_take_result() {
        let mut inv = PlayerInventory::new();
        // Stonecutter hotbar starts at slot 29 (inv.slots[36])
        inv.slots[36] = ItemStack::from_name("minecraft:stone", 4).unwrap();

        let mut menu = WorkstationMenu::new_stonecutter(5);
        let open = menu.open_packets(&inv);
        assert_eq!(read_open_screen(&open[0]), Some((5, menu_types::STONECUTTER)));

        // Move stone into input slot 0
        let _ = menu.handle_click(&mut inv, 29, 0, ContainerClickType::QuickMove, 0, false);

        // Select recipe button 0
        let expected = stonecutting_for_input(item_by_name("minecraft:stone").unwrap().id)[0];
        let btn_out = menu.handle_button_click(&mut inv, 0, 0, false);
        let sel_data = btn_out
            .packets
            .iter()
            .filter_map(|p| read_container_set_data(p))
            .find(|&(_, id, _)| id == 0)
            .map(|(_, _, v)| v);
        assert_eq!(sel_data, Some(0));

        // Left-click result slot 1 -> crafts 1 input, leaves 3 stone and keeps result populated
        let _ = menu.handle_click(&mut inv, 1, 0, ContainerClickType::Pickup, 0, false);
        assert_eq!(inv.carried.item_id, expected.result_item);
        assert_eq!(inv.carried.count, expected.result_count);

        // Drop cursor item back into inventory, then shift-click result slot 1 to craft remaining 3 stone
        inv.carried = ItemStack::EMPTY;
        let _ = menu.handle_click(&mut inv, 1, 0, ContainerClickType::QuickMove, 0, false);
        if let WorkstationMenu::Stonecutter {
            slots,
            selected_recipe,
            ..
        } = &menu
        {
            assert!(slots[0].is_empty());
            assert!(slots[1].is_empty());
            assert_eq!(*selected_recipe, -1);
        }
        let total_crafted: u32 = inv
            .slots
            .iter()
            .filter(|s| s.item_id == expected.result_item)
            .map(|s| s.count as u32)
            .sum();
        assert_eq!(total_crafted, expected.result_count as u32 * 3);
    }

    #[test]
    fn merchant_offers_select_trade_complete_trade_and_close_drops() {
        let mut inv = PlayerInventory::new();
        inv.slots[36] = ItemStack::from_name("minecraft:wheat", 45).unwrap();

        let mut menu =
            WorkstationMenu::new_merchant_default(6, 99, VillagerProfession::Farmer, 1, 0);
        let open = menu.open_packets(&inv);
        assert_eq!(read_open_screen(&open[0]), Some((6, menu_types::MERCHANT)));
        assert_eq!(packet_id(&open[1]), cb::CONTAINER_SET_CONTENT);
        assert_eq!(packet_id(&open[2]), cb::MERCHANT_OFFERS);

        // Select trade 0 (20 wheat -> 1 emerald): auto-moves 45 wheat into payment slot 0!
        let sel_pkts = menu.handle_select_trade(&mut inv, 0);
        assert_eq!(packet_id(&sel_pkts[0]), cb::CONTAINER_SET_CONTENT);
        assert!(inv.slots[36].is_empty());

        if let WorkstationMenu::Merchant { slots, .. } = &menu {
            assert_eq!(slots[0].count, 45);
            assert_eq!(
                item_by_id(slots[2].item_id).unwrap().name,
                "minecraft:emerald"
            );
            assert_eq!(slots[2].count, 1);
        }

        // Take result slot 2 -> completes trade, consumes 20 wheat, increments uses, resends MerchantOffers!
        let trade_out = menu.handle_click(&mut inv, 2, 0, ContainerClickType::Pickup, 0, false);
        assert!(trade_out.completed_trade);
        assert_eq!(
            item_by_id(inv.carried.item_id).unwrap().name,
            "minecraft:emerald"
        );
        assert!(
            trade_out
                .packets
                .iter()
                .any(|p| packet_id(p) == cb::MERCHANT_OFFERS),
            "completed trade must resend updated MerchantOffers packet"
        );

        if let WorkstationMenu::Merchant {
            offers, slots, xp, ..
        } = &menu
        {
            assert_eq!(offers[0].uses, 1);
            assert_eq!(*xp, offers[0].xp);
            assert_eq!(slots[0].count, 25); // 45 - 20 = 25 wheat left
            assert_eq!(
                item_by_id(slots[2].item_id).unwrap().name,
                "minecraft:emerald"
            );
        }

        // Closing menu returns the remaining 25 wheat (and NOT the preview result emerald!)
        let drops = menu.close_drops();
        assert_eq!(drops.len(), 1);
        assert_eq!(item_by_id(drops[0].item_id).unwrap().name, "minecraft:wheat");
        assert_eq!(drops[0].count, 25);

        // Verify close_drops on transient menus returns only input slots
        let mut anvil = WorkstationMenu::new_anvil(7, BlockPos::new(1, 64, 1));
        if let WorkstationMenu::Anvil { slots, .. } = &mut anvil {
            slots[0] = ItemStack::from_name("minecraft:iron_sword", 1).unwrap();
            slots[2] = ItemStack::from_name("minecraft:iron_sword", 1).unwrap();
        }
        let anvil_drops = anvil.close_drops();
        assert_eq!(anvil_drops.len(), 1);
    }
}
