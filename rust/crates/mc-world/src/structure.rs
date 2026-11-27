use crate::{BlockEntity, BlockWorld};
use flate2::read::GzDecoder;
use mc_data::{BlockStateId, lookup_block_state_from_nbt};
use mc_math::BlockPos;
use mc_nbt::{NbtCompound, NbtTag};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureRotation {
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

impl StructureRotation {
    pub fn from_degrees(deg: i32) -> Self {
        match deg.rem_euclid(360) {
            90 => Self::Clockwise90,
            180 => Self::Clockwise180,
            270 => Self::CounterClockwise90,
            _ => Self::None,
        }
    }

    #[inline]
    pub fn transform(self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        match self {
            Self::None => (x, y, z),
            Self::Clockwise90 => (-z, y, x),
            Self::Clockwise180 => (-x, y, -z),
            Self::CounterClockwise90 => (z, y, -x),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructureBlock {
    pub pos: BlockPos,
    pub state: BlockStateId,
    pub nbt: Option<NbtCompound>,
}

/// Vanilla Minecraft 26.3 `.nbt` StructureTemplate (`data/minecraft/structure/**/*.nbt`).
#[derive(Debug, Clone)]
pub struct StructureTemplate {
    pub size: (i32, i32, i32),
    pub blocks: Vec<StructureBlock>,
}

impl StructureTemplate {
    /// Load a gzipped `.nbt` structure file directly from `data/minecraft/structure/<id>.nbt`.
    pub fn load_from_data_dir(data_dir: &Path, template_id: &str) -> Result<Self, String> {
        let clean = template_id
            .strip_prefix("minecraft:")
            .unwrap_or(template_id)
            .trim_start_matches('/');
        if clean.contains("..") {
            return Err("Invalid structure template path".into());
        }
        let path = data_dir
            .join("minecraft")
            .join("structure")
            .join(format!("{clean}.nbt"));
        Self::load_from_file(&path)
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw).map_err(|e| e.to_string())?;

        // Structure .nbt files are normally Gzip-compressed (0x1f 0x8b); support raw NBT too.
        let nbt_bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
            let mut gz = GzDecoder::new(&raw[..]);
            let mut dec = Vec::new();
            gz.read_to_end(&mut dec).map_err(|e| e.to_string())?;
            dec
        } else {
            raw
        };

        let (_, root) = mc_nbt::read_file_bytes(&nbt_bytes).map_err(|e| e.to_string())?;
        Self::from_nbt(&root)
    }

    pub fn from_nbt(root: &NbtCompound) -> Result<Self, String> {
        let size = root
            .get_list("size")
            .map(|l| {
                let g = |i: usize| match l.get(i) {
                    Some(NbtTag::Int(v)) => *v,
                    _ => 0,
                };
                (g(0), g(1), g(2))
            })
            .unwrap_or((0, 0, 0));

        // Support both `palette` and `palettes` (pick first variant when multiple palettes exist)
        let palette_list = root.get_list("palette").or_else(|| {
            root.get_list("palettes").and_then(|pl| match pl.first() {
                Some(NbtTag::List(first)) => Some(first.as_slice()),
                _ => None,
            })
        });

        let mut palette: Vec<BlockStateId> = Vec::new();
        if let Some(plist) = palette_list {
            for tag in plist {
                let st = tag
                    .as_compound()
                    .and_then(lookup_block_state_from_nbt)
                    .unwrap_or(BlockStateId::AIR);
                palette.push(st);
            }
        }

        let mut blocks = Vec::new();
        if let Some(blist) = root.get_list("blocks") {
            for tag in blist {
                let Some(bc) = tag.as_compound() else {
                    continue;
                };
                let state_idx = bc.get_int("state").unwrap_or(0) as usize;
                let state = palette.get(state_idx).copied().unwrap_or(BlockStateId::AIR);
                if state.block_name() == "minecraft:structure_void" {
                    continue;
                }
                let pos = bc
                    .get_list("pos")
                    .map(|l| {
                        let g = |i: usize| match l.get(i) {
                            Some(NbtTag::Int(v)) => *v,
                            _ => 0,
                        };
                        BlockPos::new(g(0), g(1), g(2))
                    })
                    .unwrap_or(BlockPos::ZERO);
                let nbt = bc.get_compound("nbt").cloned();
                blocks.push(StructureBlock { pos, state, nbt });
            }
        }

        Ok(Self { size, blocks })
    }

    /// Place this template into `world` anchored at `origin` with `rotation`.
    /// Returns `(placed_blocks, instantiated_block_entities)`.
    pub fn place<W: BlockWorld>(
        &self,
        world: &mut W,
        origin: BlockPos,
        rotation: StructureRotation,
    ) -> (Vec<(BlockPos, BlockStateId)>, Vec<BlockEntity>) {
        let mut placed = Vec::with_capacity(self.blocks.len());
        let mut block_entities = Vec::new();

        for b in &self.blocks {
            let (rx, ry, rz) = rotation.transform(b.pos.x, b.pos.y, b.pos.z);
            let wp = origin.offset(rx, ry, rz);
            if b.state.block_name() == "minecraft:jigsaw" {
                continue;
            }
            if world.set_block_state(wp, b.state) {
                placed.push((wp, b.state));
                if let Some(ref raw_nbt) = b.nbt {
                    let mut patched = raw_nbt.clone();
                    patched.insert("x", NbtTag::Int(wp.x));
                    patched.insert("y", NbtTag::Int(wp.y));
                    patched.insert("z", NbtTag::Int(wp.z));
                    if let Some(be) = BlockEntity::load_from_nbt(&patched) {
                        block_entities.push(be);
                    }
                }
            }
        }
        (placed, block_entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::OracleClient;
    use std::collections::HashMap;

    struct MapWorld(HashMap<BlockPos, BlockStateId>);
    impl BlockWorld for MapWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.0.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }
        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            self.0.insert(pos, state);
            true
        }
    }

    #[test]
    fn loads_and_places_real_vanilla_26_3_igloo_and_village_house_nbt() {
        let data_dir = OracleClient::repo_root()
            .join("src")
            .join("main")
            .join("resources")
            .join("data");

        let igloo = StructureTemplate::load_from_data_dir(&data_dir, "minecraft:igloo/top").unwrap();
        assert!(igloo.size.0 >= 7 && igloo.size.1 >= 5 && igloo.size.2 >= 7);
        assert!(igloo.blocks.len() > 100);
        let mut w = MapWorld(HashMap::new());
        let (placed, _bes) = igloo.place(&mut w, BlockPos::new(10, 64, 10), StructureRotation::None);
        assert!(placed.len() > 100);
        assert!(placed.iter().any(|(_, s)| s.block_name() == "minecraft:snow_block"));

        let house = StructureTemplate::load_from_data_dir(
            &data_dir,
            "village/plains/houses/plains_small_house_1",
        )
        .unwrap();
        assert!(house.blocks.len() > 50);
    }
}
