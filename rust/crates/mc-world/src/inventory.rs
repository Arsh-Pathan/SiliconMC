use mc_data::items::{ItemInfo, item_by_id, item_by_name};
use mc_data::BlockStateId;

/// Lightweight, protocol-exact item stack representation matching Minecraft 26.3's `ItemStack`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemStack {
    pub item_id: u16,
    pub count: u8,
    /// Raw added `DataComponentPatch` entries: `(component_type_id, encoded_bytes)`.
    pub added_components: Vec<(i32, Vec<u8>)>,
    /// Removed `DataComponentPatch` component type IDs.
    pub removed_components: Vec<i32>,
}

impl ItemStack {
    pub const EMPTY: Self = Self {
        item_id: 0,
        count: 0,
        added_components: Vec::new(),
        removed_components: Vec::new(),
    };

    pub fn new(item_id: u16, count: u8) -> Self {
        if count == 0 || item_id == 0 {
            Self::EMPTY
        } else {
            Self {
                item_id,
                count,
                added_components: Vec::new(),
                removed_components: Vec::new(),
            }
        }
    }

    pub fn has_components(&self) -> bool {
        !self.added_components.is_empty() || !self.removed_components.is_empty()
    }

    pub fn from_name(name: &str, count: u8) -> Option<Self> {
        let info = item_by_name(name)?;
        Some(Self::new(info.id, count.min(info.max_stack_size)))
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0 || self.item_id == 0
    }

    #[inline]
    pub fn info(&self) -> Option<&'static ItemInfo> {
        if self.is_empty() {
            None
        } else {
            item_by_id(self.item_id)
        }
    }

    #[inline]
    pub fn max_stack_size(&self) -> u8 {
        self.info().map(|i| i.max_stack_size).unwrap_or(64)
    }

    #[inline]
    pub fn placed_block_state(&self) -> Option<BlockStateId> {
        self.info()
            .and_then(|i| i.placed_block_state())
            .map(BlockStateId)
    }
}

/// Vanilla 46-slot player inventory container (`InventoryMenu`):
/// - `0`: crafting output
/// - `1..=4`: crafting grid
/// - `5..=8`: armor (head, chest, legs, feet)
/// - `9..=35`: main inventory (3 rows x 9)
/// - `36..=44`: hotbar (9 slots)
/// - `45`: offhand
#[derive(Debug, Clone)]
pub struct PlayerInventory {
    pub slots: Vec<ItemStack>,
    pub selected_hotbar: u8,
    pub carried: ItemStack,
    pub state_id: i32,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerInventory {
    pub const SLOT_COUNT: usize = 46;
    pub const HOTBAR_START: usize = 36;
    pub const MAIN_START: usize = 9;
    pub const OFFHAND_SLOT: usize = 45;

    pub fn new() -> Self {
        Self {
            slots: vec![ItemStack::EMPTY; Self::SLOT_COUNT],
            selected_hotbar: 0,
            carried: ItemStack::EMPTY,
            state_id: 1,
        }
    }

    #[inline]
    pub fn next_state_id(&mut self) -> i32 {
        self.state_id = (self.state_id + 1) & 0x7FFF;
        self.state_id
    }

    #[inline]
    pub fn held_slot_index(&self) -> usize {
        Self::HOTBAR_START + (self.selected_hotbar as usize).min(8)
    }

    #[inline]
    pub fn held_item(&self) -> &ItemStack {
        &self.slots[self.held_slot_index()]
    }

    #[inline]
    pub fn offhand_item(&self) -> &ItemStack {
        &self.slots[Self::OFFHAND_SLOT]
    }

    pub fn set_selected_hotbar(&mut self, slot: i16) {
        if (0..=8).contains(&slot) {
            self.selected_hotbar = slot as u8;
        }
    }

    pub fn set_slot(&mut self, slot: usize, stack: ItemStack) -> bool {
        if slot >= Self::SLOT_COUNT {
            return false;
        }
        self.slots[slot] = if stack.is_empty() {
            ItemStack::EMPTY
        } else {
            stack
        };
        true
    }

    /// Add `count` items of `item_id` to the player's hotbar + main inventory (vanilla order).
    /// Returns the list of `(container_slot, updated_stack)` pairs to sync to the client.
    pub fn give_item(&mut self, item_id: u16, mut remaining: u32) -> Vec<(i16, ItemStack)> {
        let Some(info) = item_by_id(item_id) else {
            return Vec::new();
        };
        if item_id == 0 || remaining == 0 {
            return Vec::new();
        }
        let max_stack = info.max_stack_size.max(1) as u32;
        let mut changed = Vec::new();

        // Order: hotbar (36..45), then main (9..36)
        let order: Vec<usize> = (Self::HOTBAR_START..45).chain(Self::MAIN_START..36).collect();

        // 1. Merge into existing matching stacks
        for &idx in &order {
            if remaining == 0 {
                break;
            }
            let s = &mut self.slots[idx];
            if !s.is_empty() && s.item_id == item_id && !s.has_components() {
                let space = max_stack.saturating_sub(s.count as u32);
                if space > 0 {
                    let add = remaining.min(space);
                    s.count += add as u8;
                    remaining -= add;
                    changed.push((idx as i16, s.clone()));
                }
            }
        }

        // 2. Fill empty slots
        for &idx in &order {
            if remaining == 0 {
                break;
            }
            if self.slots[idx].is_empty() {
                let add = remaining.min(max_stack);
                let st = ItemStack::new(item_id, add as u8);
                self.slots[idx] = st.clone();
                remaining -= add;
                changed.push((idx as i16, st));
            }
        }

        changed
    }

    /// Consume 1 item from the currently held slot (or offhand if `offhand == true`).
    pub fn consume_one(&mut self, offhand: bool) -> Option<(i16, ItemStack)> {
        let idx = if offhand {
            Self::OFFHAND_SLOT
        } else {
            self.held_slot_index()
        };
        let slot = &mut self.slots[idx];
        if slot.is_empty() {
            return None;
        }
        if slot.count <= 1 {
            *slot = ItemStack::EMPTY;
        } else {
            slot.count -= 1;
        }
        Some((idx as i16, slot.clone()))
    }
}
