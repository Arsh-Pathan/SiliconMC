use crate::BlockWorld;
use mc_data::BlockStateId;
use mc_data::block_extra::{FIRE_STATE_ID, TNT_STATE_ID, blast_resistance, is_tnt};
use mc_math::rng::{LegacyRandomSource, RandomSource};
use mc_math::{BlockPos, Vec3};
use std::collections::HashSet;

const GRID_SIZE: i32 = 16;
const RAY_STEP: f32 = 0.3_f32;
const RAY_ATTENUATION: f32 = 0.22500001_f32;
const FLUID_BLAST_RESISTANCE: f32 = 100.0_f32;

/// Outcome of computing and applying a vanilla Minecraft 26.3 explosion to a `BlockWorld`.
#[derive(Debug, Clone)]
pub struct ExplosionResult {
    /// Non-air blocks removed by the blast as `(pos, previous_state)`.
    pub destroyed_blocks: Vec<(BlockPos, BlockStateId)>,
    /// Positions of TNT blocks struck by the blast (cleared in world so callers can spawn primed TNT).
    pub ignited_tnt: Vec<BlockPos>,
    /// Fire blocks placed after block destruction when `fire` is enabled.
    pub fire_blocks: Vec<(BlockPos, BlockStateId)>,
    pub center: Vec3,
    pub power: f32,
}

/// Computes affected block positions using vanilla Minecraft's 16x16x16 outer-shell raycast
/// and optionally destroys blocks, detects chain TNT ignition, and ignites fire.
pub fn compute_and_apply_explosion<W: BlockWorld>(
    world: &mut W,
    center: Vec3,
    power: f32,
    fire: bool,
    break_blocks: bool,
    seed: u64,
) -> ExplosionResult {
    let mut result = ExplosionResult {
        destroyed_blocks: Vec::new(),
        ignited_tnt: Vec::new(),
        fire_blocks: Vec::new(),
        center,
        power,
    };

    if power <= 0.0_f32 {
        return result;
    }

    let mut rng = LegacyRandomSource::new(seed as i64);
    let mut visited: HashSet<BlockPos> = HashSet::new();
    let mut to_blow: Vec<BlockPos> = Vec::new();

    for xx in 0..GRID_SIZE {
        for yy in 0..GRID_SIZE {
            for zz in 0..GRID_SIZE {
                if xx == 0
                    || xx == GRID_SIZE - 1
                    || yy == 0
                    || yy == GRID_SIZE - 1
                    || zz == 0
                    || zz == GRID_SIZE - 1
                {
                    let mut xd = (xx as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                    let mut yd = (yy as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                    let mut zd = (zz as f32 / 15.0_f32 * 2.0_f32 - 1.0_f32) as f64;
                    let len = (xd * xd + yd * yd + zd * zd).sqrt();
                    xd /= len;
                    yd /= len;
                    zd /= len;

                    let mut remaining_power = power * (0.7_f32 + rng.next_float() * 0.6_f32);
                    let mut xp = center.x;
                    let mut yp = center.y;
                    let mut zp = center.z;

                    while remaining_power > 0.0_f32 {
                        let pos = BlockPos::containing(xp, yp, zp);
                        if pos.y < -2048 || pos.y > 2048 {
                            break;
                        }

                        let state = world.get_block_state(pos);
                        let props = state.props();
                        if !state.is_air() || props.has_fluid() {
                            let mut resistance = blast_resistance(state.0);
                            if props.has_fluid() {
                                resistance = resistance.max(FLUID_BLAST_RESISTANCE);
                            }
                            remaining_power -= (resistance + 0.3_f32) * 0.3_f32;
                        }

                        if remaining_power > 0.0_f32 && visited.insert(pos) {
                            to_blow.push(pos);
                        }

                        xp += xd * (RAY_STEP as f64);
                        yp += yd * (RAY_STEP as f64);
                        zp += zd * (RAY_STEP as f64);
                        remaining_power -= RAY_ATTENUATION;
                    }
                }
            }
        }
    }

    if break_blocks {
        for &pos in &to_blow {
            let old_state = world.get_block_state(pos);
            if old_state.is_air() {
                continue;
            }
            if old_state.0 == TNT_STATE_ID || is_tnt(old_state.0) {
                world.set_block_state(pos, BlockStateId::AIR);
                result.ignited_tnt.push(pos);
                result.destroyed_blocks.push((pos, old_state));
            } else {
                world.set_block_state(pos, BlockStateId::AIR);
                result.destroyed_blocks.push((pos, old_state));
            }
        }
    }

    if fire {
        let fire_state = BlockStateId(FIRE_STATE_ID);
        for &pos in &to_blow {
            if rng.next_int_bounded(3) == 0 && world.get_block_state(pos).is_air() {
                let below_props = world.get_block_state(pos.offset(0, -1, 0)).props();
                if below_props.can_occlude() && below_props.is_solid() {
                    world.set_block_state(pos, fire_state);
                    result.fire_blocks.push((pos, fire_state));
                }
            }
        }
    }

    result
}

/// Computes vanilla explosion damage and knockback velocity for an entity at `entity_pos`
/// with full line-of-sight exposure (`1.0`). Returns `None` if outside blast diameter (`power * 2.0`).
#[inline]
pub fn compute_entity_explosion_knockback(
    center: Vec3,
    power: f32,
    entity_pos: Vec3,
) -> Option<(f32, Vec3)> {
    compute_entity_explosion_knockback_with_exposure(center, power, entity_pos, 1.0_f32)
}

/// Computes vanilla explosion damage and knockback velocity using an explicit exposure factor (`0.0..=1.0`).
pub fn compute_entity_explosion_knockback_with_exposure(
    center: Vec3,
    power: f32,
    entity_pos: Vec3,
    exposure: f32,
) -> Option<(f32, Vec3)> {
    if power < 1.0e-5_f32 {
        return None;
    }

    let diameter = (power * 2.0_f32) as f64;
    let dx = entity_pos.x - center.x;
    let dy = entity_pos.y - center.y;
    let dz = entity_pos.z - center.z;
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let dist_frac = dist / diameter;
    if dist_frac > 1.0_f64 {
        return None;
    }

    let impact = (1.0_f64 - dist_frac) * (exposure.clamp(0.0_f32, 1.0_f32) as f64);
    let damage = ((impact * impact + impact) / 2.0_f64 * 7.0_f64 * diameter + 1.0_f64) as f32;

    let knockback = if dist > 1.0e-5_f64 {
        Vec3::new(
            (dx / dist) * impact,
            (dy / dist) * impact,
            (dz / dist) * impact,
        )
    } else {
        Vec3::ZERO
    };

    Some((damage, knockback))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::block_extra::OBSIDIAN_STATE_ID;
    use std::collections::HashMap;

    struct MapWorld(HashMap<BlockPos, BlockStateId>);

    impl MapWorld {
        fn new() -> Self {
            Self(HashMap::new())
        }

        fn fill(&mut self, min: BlockPos, max: BlockPos, state: BlockStateId) {
            for x in min.x..=max.x {
                for y in min.y..=max.y {
                    for z in min.z..=max.z {
                        self.0.insert(BlockPos::new(x, y, z), state);
                    }
                }
            }
        }
    }

    impl BlockWorld for MapWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.0.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }

        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            if state.is_air() {
                self.0.remove(&pos);
            } else {
                self.0.insert(pos, state);
            }
            true
        }
    }

    #[test]
    fn tnt_crater_in_stone_vs_obsidian_protection() {
        let mut world = MapWorld::new();
        world.fill(
            BlockPos::new(-5, 5, -5),
            BlockPos::new(5, 15, 5),
            BlockStateId::STONE,
        );
        // Carve a 1x1x1 chamber at center for TNT detonation
        let center_pos = BlockPos::new(0, 10, 0);
        world.set_block_state(center_pos, BlockStateId::AIR);

        // Place an obsidian blast shield wall at x = 1 separating +X stone from the explosion
        let obsidian = BlockStateId(OBSIDIAN_STATE_ID);
        world.fill(
            BlockPos::new(1, 7, -3),
            BlockPos::new(1, 13, 3),
            obsidian,
        );

        let res = compute_and_apply_explosion(
            &mut world,
            Vec3::new(0.5, 10.5, 0.5),
            4.0,
            false,
            true,
            12345,
        );

        assert!(
            !res.destroyed_blocks.is_empty(),
            "TNT should carve a crater in stone"
        );
        // Stone immediately on the unprotected -X side must be destroyed
        assert!(world.get_block_state(BlockPos::new(-1, 10, 0)).is_air());
        // Every obsidian shield block must survive
        for y in 7..=13 {
            for z in -3..=3 {
                assert_eq!(
                    world.get_block_state(BlockPos::new(1, y, z)),
                    obsidian,
                    "Obsidian should resist TNT explosions"
                );
            }
        }
        // Stone directly behind the center of the obsidian shield must survive
        assert_eq!(
            world.get_block_state(BlockPos::new(2, 10, 0)),
            BlockStateId::STONE,
            "Stone shielded behind obsidian wall should be protected"
        );
    }

    #[test]
    fn chain_tnt_detection() {
        let mut world = MapWorld::new();
        let target_tnt = BlockPos::new(2, 10, 0);
        let far_tnt = BlockPos::new(20, 10, 0);
        world.set_block_state(target_tnt, BlockStateId(TNT_STATE_ID));
        world.set_block_state(far_tnt, BlockStateId(TNT_STATE_ID));

        let res = compute_and_apply_explosion(
            &mut world,
            Vec3::new(0.5, 10.5, 0.5),
            4.0,
            false,
            true,
            999,
        );

        assert!(
            res.ignited_tnt.contains(&target_tnt),
            "Nearby TNT should be detected for chain ignition"
        );
        assert!(
            !res.ignited_tnt.contains(&far_tnt),
            "Out-of-range TNT should not ignite"
        );
        assert!(world.get_block_state(target_tnt).is_air());
        assert_eq!(world.get_block_state(far_tnt), BlockStateId(TNT_STATE_ID));
    }

    #[test]
    fn explosion_with_fire_ignites_floor() {
        let mut world = MapWorld::new();
        world.fill(
            BlockPos::new(-6, 9, -6),
            BlockPos::new(6, 9, 6),
            BlockStateId(OBSIDIAN_STATE_ID),
        );

        let res = compute_and_apply_explosion(
            &mut world,
            Vec3::new(0.5, 10.5, 0.5),
            4.0,
            true,
            true,
            42,
        );

        assert!(
            !res.fire_blocks.is_empty(),
            "Fiery explosion over solid floor should place fire blocks"
        );
        for (pos, state) in &res.fire_blocks {
            assert_eq!(state.0, FIRE_STATE_ID);
            assert_eq!(world.get_block_state(*pos), BlockStateId(FIRE_STATE_ID));
            assert_eq!(
                world.get_block_state(pos.offset(0, -1, 0)),
                BlockStateId(OBSIDIAN_STATE_ID)
            );
        }
    }

    #[test]
    fn entity_blast_damage_and_knockback_accuracy() {
        let center = Vec3::new(0.0, 10.0, 0.0);

        // Point-blank TNT (power 4.0 -> diameter 8.0): impact=1.0, damage=57.0
        let (dmg_zero, kb_zero) =
            compute_entity_explosion_knockback(center, 4.0, center).expect("point-blank hit");
        assert!((dmg_zero - 57.0).abs() < 1e-4);
        assert_eq!(kb_zero, Vec3::ZERO);

        // Half blast radius (dist=4.0 along +X): impact=0.5, damage=22.0, kb=(0.5, 0, 0)
        let (dmg_mid, kb_mid) =
            compute_entity_explosion_knockback(center, 4.0, Vec3::new(4.0, 10.0, 0.0))
                .expect("mid-range hit");
        assert!((dmg_mid - 22.0).abs() < 1e-4);
        assert!((kb_mid.x - 0.5).abs() < 1e-6);
        assert!(kb_mid.y.abs() < 1e-6);
        assert!(kb_mid.z.abs() < 1e-6);

        // Beyond blast diameter (dist > 8.0) -> None
        assert!(
            compute_entity_explosion_knockback(center, 4.0, Vec3::new(8.5, 10.0, 0.0)).is_none()
        );
        assert!(compute_entity_explosion_knockback(center, 0.0, center).is_none());
    }
}
