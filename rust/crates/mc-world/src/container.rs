use crate::ItemStack;
use mc_data::recipes::match_crafting;

/// Matches Minecraft 26.3's `net.minecraft.world.inventory.ContainerInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerClickType {
    Pickup = 0,
    QuickMove = 1,
    Swap = 2,
    Clone = 3,
    Throw = 4,
    QuickCraft = 5,
    PickupAll = 6,
}

impl ContainerClickType {
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::Pickup),
            1 => Some(Self::QuickMove),
            2 => Some(Self::Swap),
            3 => Some(Self::Clone),
            4 => Some(Self::Throw),
            5 => Some(Self::QuickCraft),
            6 => Some(Self::PickupAll),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClickResult {
    pub dropped: Option<ItemStack>,
}

/// Recompute crafting output slot `0` for either 2x2 inventory crafting (slots 1..=4)
/// or 3x3 crafting table (slots 1..=9).
pub fn refresh_crafting_result(slots: &mut [ItemStack], grid_w: usize, grid_h: usize) {
    let n = grid_w * grid_h;
    if slots.len() <= n {
        return;
    }
    let mut ids = [0u16; 9];
    for i in 0..n {
        ids[i] = if slots[1 + i].is_empty() {
            0
        } else {
            slots[1 + i].item_id
        };
    }
    slots[0] = match match_crafting(grid_w, grid_h, &ids[..n]) {
        Some((item_id, count)) => ItemStack::new(item_id, count),
        None => ItemStack::EMPTY,
    };
}

fn can_merge(a: &ItemStack, b: &ItemStack) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.item_id == b.item_id
        && !a.has_components()
        && !b.has_components()
}

fn move_into_range(slots: &mut [ItemStack], stack: &mut ItemStack, range: std::ops::Range<usize>) {
    if stack.is_empty() {
        return;
    }
    let max_st = stack.max_stack_size();
    // 1. Merge into existing matching stacks
    for idx in range.clone() {
        if stack.is_empty() {
            return;
        }
        if can_merge(&slots[idx], stack) {
            let space = max_st.saturating_sub(slots[idx].count);
            if space > 0 {
                let take = stack.count.min(space);
                slots[idx].count += take;
                stack.count -= take;
                if stack.count == 0 {
                    *stack = ItemStack::EMPTY;
                }
            }
        }
    }
    // 2. Place into empty slots
    for idx in range {
        if stack.is_empty() {
            return;
        }
        if slots[idx].is_empty() {
            slots[idx] = stack.clone();
            *stack = ItemStack::EMPTY;
        }
    }
}

/// Apply a vanilla container slot click to a flat menu slot view (`slots`) + cursor `carried`.
/// - `top_count`: number of top container slots (e.g., 27 for chest, 3 for furnace, 10 for crafting table, 9 for player inv top)
/// - `crafting_grid`: `Some((2, 2))` for player inventory or `Some((3, 3))` for crafting table (slot 0 = result, 1..=w*h = grid)
/// - `hotbar_start`: index of hotbar slot 0 inside `slots` (36 for player inv, 37 for crafting table, 54 for chest, 30 for furnace)
pub fn apply_container_click(
    slots: &mut [ItemStack],
    carried: &mut ItemStack,
    slot_num: i16,
    button: i8,
    click_type: ContainerClickType,
    top_count: usize,
    crafting_grid: Option<(usize, usize)>,
    hotbar_start: usize,
    is_creative: bool,
) -> ClickResult {
    let mut res = ClickResult::default();

    // Outside-window drop (-999)
    if slot_num == -999 {
        if click_type == ContainerClickType::Pickup && !carried.is_empty() {
            if button == 0 {
                res.dropped = Some(std::mem::take(carried));
            } else if button == 1 {
                let mut one = carried.clone();
                one.count = 1;
                res.dropped = Some(one);
                if carried.count <= 1 {
                    *carried = ItemStack::EMPTY;
                } else {
                    carried.count -= 1;
                }
            }
        }
        return res;
    }

    if slot_num < 0 || (slot_num as usize) >= slots.len() {
        return res;
    }
    let idx = slot_num as usize;

    // Special case: taking from crafting output slot 0!
    if let Some((gw, gh)) = crafting_grid {
        if idx == 0 {
            refresh_crafting_result(slots, gw, gh);
            if slots[0].is_empty() {
                return res;
            }
            match click_type {
                ContainerClickType::Pickup => {
                    let out = slots[0].clone();
                    if carried.is_empty() {
                        *carried = out;
                        consume_crafting_inputs(slots, gw * gh);
                        refresh_crafting_result(slots, gw, gh);
                    } else if can_merge(carried, &out)
                        && carried.count.saturating_add(out.count) <= carried.max_stack_size()
                    {
                        carried.count += out.count;
                        consume_crafting_inputs(slots, gw * gh);
                        refresh_crafting_result(slots, gw, gh);
                    }
                }
                ContainerClickType::QuickMove => {
                    // Craft repeatedly while recipe matches and player inventory has room
                    let inv_start = top_count;
                    let inv_end = (hotbar_start + 9).min(slots.len());
                    for _ in 0..64 {
                        refresh_crafting_result(slots, gw, gh);
                        if slots[0].is_empty() {
                            break;
                        }
                        let mut out = slots[0].clone();
                        let orig = out.count;
                        move_into_range(slots, &mut out, inv_start..inv_end);
                        if out.count == orig {
                            break;
                        }
                        consume_crafting_inputs(slots, gw * gh);
                    }
                    refresh_crafting_result(slots, gw, gh);
                }
                _ => {}
            }
            return res;
        }
    }

    match click_type {
        ContainerClickType::Pickup => {
            if button == 0 {
                // Left click
                if carried.is_empty() {
                    std::mem::swap(&mut slots[idx], carried);
                } else if slots[idx].is_empty() {
                    std::mem::swap(&mut slots[idx], carried);
                } else if can_merge(&slots[idx], carried) {
                    let max_st = slots[idx].max_stack_size();
                    let space = max_st.saturating_sub(slots[idx].count);
                    let take = carried.count.min(space);
                    slots[idx].count += take;
                    carried.count -= take;
                    if carried.count == 0 {
                        *carried = ItemStack::EMPTY;
                    }
                } else {
                    std::mem::swap(&mut slots[idx], carried);
                }
            } else if button == 1 {
                // Right click: split half or place 1
                if carried.is_empty() {
                    if !slots[idx].is_empty() {
                        let take = slots[idx].count.div_ceil(2);
                        let mut taken = slots[idx].clone();
                        taken.count = take;
                        slots[idx].count -= take;
                        if slots[idx].count == 0 {
                            slots[idx] = ItemStack::EMPTY;
                        }
                        *carried = taken;
                    }
                } else if slots[idx].is_empty() {
                    let mut one = carried.clone();
                    one.count = 1;
                    slots[idx] = one;
                    carried.count -= 1;
                    if carried.count == 0 {
                        *carried = ItemStack::EMPTY;
                    }
                } else if can_merge(&slots[idx], carried)
                    && slots[idx].count < slots[idx].max_stack_size()
                {
                    slots[idx].count += 1;
                    carried.count -= 1;
                    if carried.count == 0 {
                        *carried = ItemStack::EMPTY;
                    }
                } else {
                    std::mem::swap(&mut slots[idx], carried);
                }
            }
        }
        ContainerClickType::QuickMove => {
            if !slots[idx].is_empty() {
                let mut moving = std::mem::take(&mut slots[idx]);
                if idx < top_count {
                    // Move from top container -> player inventory
                    let inv_end = (hotbar_start + 9).min(slots.len());
                    move_into_range(slots, &mut moving, top_count..inv_end);
                } else if top_count > 0 && crafting_grid.is_none() {
                    // Move from player inventory -> chest/barrel/furnace
                    move_into_range(slots, &mut moving, 0..top_count);
                } else if idx >= hotbar_start && idx < hotbar_start + 9 {
                    // Move from hotbar -> main inventory
                    move_into_range(slots, &mut moving, top_count..hotbar_start);
                } else {
                    // Move from main inventory -> hotbar
                    let inv_end = (hotbar_start + 9).min(slots.len());
                    move_into_range(slots, &mut moving, hotbar_start..inv_end);
                }
                slots[idx] = moving;
            }
        }
        ContainerClickType::Swap => {
            if (0..=8).contains(&button) {
                let hb = hotbar_start + button as usize;
                if hb < slots.len() {
                    slots.swap(idx, hb);
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
                    res.dropped = Some(std::mem::take(&mut slots[idx]));
                } else {
                    let mut one = slots[idx].clone();
                    one.count = 1;
                    res.dropped = Some(one);
                    slots[idx].count -= 1;
                    if slots[idx].count == 0 {
                        slots[idx] = ItemStack::EMPTY;
                    }
                }
            }
        }
        ContainerClickType::PickupAll => {
            if !carried.is_empty() {
                let max_st = carried.max_stack_size();
                for s in slots.iter_mut() {
                    if carried.count >= max_st {
                        break;
                    }
                    if can_merge(carried, s) {
                        let take = (max_st - carried.count).min(s.count);
                        carried.count += take;
                        s.count -= take;
                        if s.count == 0 {
                            *s = ItemStack::EMPTY;
                        }
                    }
                }
            }
        }
        ContainerClickType::QuickCraft => {}
    }

    if let Some((gw, gh)) = crafting_grid {
        refresh_crafting_result(slots, gw, gh);
    }

    res
}

fn consume_crafting_inputs(slots: &mut [ItemStack], n: usize) {
    for i in 1..=n {
        if !slots[i].is_empty() {
            if slots[i].count <= 1 {
                slots[i] = ItemStack::EMPTY;
            } else {
                slots[i].count -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::items::item_by_name;

    #[test]
    fn player_2x2_crafting_and_chest_quick_move() {
        let oak_log = ItemStack::from_name("minecraft:oak_log", 2).unwrap();
        let oak_planks_id = item_by_name("minecraft:oak_planks").unwrap().id;

        let mut slots = vec![ItemStack::EMPTY; 46];
        let mut carried = ItemStack::EMPTY;
        slots[36] = oak_log;

        // Pick up oak logs from hotbar slot 36, right-click 1 into crafting slot 1
        apply_container_click(
            &mut slots,
            &mut carried,
            36,
            0,
            ContainerClickType::Pickup,
            9,
            Some((2, 2)),
            36,
            false,
        );
        assert_eq!(carried.count, 2);

        apply_container_click(
            &mut slots,
            &mut carried,
            1,
            1,
            ContainerClickType::Pickup,
            9,
            Some((2, 2)),
            36,
            false,
        );
        // Output slot 0 must now contain 4 oak_planks!
        assert_eq!(slots[0].item_id, oak_planks_id);
        assert_eq!(slots[0].count, 4);

        // Put remaining log back into slot 36, then click slot 0 to take 4 oak_planks
        apply_container_click(
            &mut slots,
            &mut carried,
            36,
            0,
            ContainerClickType::Pickup,
            9,
            Some((2, 2)),
            36,
            false,
        );
        assert!(carried.is_empty());

        apply_container_click(
            &mut slots,
            &mut carried,
            0,
            0,
            ContainerClickType::Pickup,
            9,
            Some((2, 2)),
            36,
            false,
        );
        assert_eq!(carried.item_id, oak_planks_id);
        assert_eq!(carried.count, 4);
        assert!(slots[1].is_empty());
        assert!(slots[0].is_empty());

        // QuickMove into a 27-slot chest
        let mut chest_menu = vec![ItemStack::EMPTY; 63];
        chest_menu[54] = ItemStack::new(oak_planks_id, 16); // hotbar slot 0
        let mut c2 = ItemStack::EMPTY;
        apply_container_click(
            &mut chest_menu,
            &mut c2,
            54,
            0,
            ContainerClickType::QuickMove,
            27,
            None,
            54,
            false,
        );
        assert!(chest_menu[54].is_empty());
        assert_eq!(chest_menu[0].item_id, oak_planks_id);
        assert_eq!(chest_menu[0].count, 16);
    }
}
