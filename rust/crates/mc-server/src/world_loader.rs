use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mc_data::block_behaviors::{
    KIND_BARREL, KIND_BLAST_FURNACE, KIND_CHEST, KIND_FURNACE, KIND_HOPPER, KIND_SIGN, KIND_SMOKER,
    block_behavior,
};
use mc_data::recipes::{COOK_BLASTING, COOK_SMELTING, COOK_SMOKING};
use mc_data::{BlockStateId, LoadedDatapacks, WORLD_VERSION};
use mc_math::{BlockPos, ChunkPos};
use mc_nbt::{NbtCompound, NbtTag};
use mc_world::{
    BlockEntity, BlockEntityKind, BlockWorld, Chunk, DirtyLightSections, FurnaceTickResult,
    LightEngine, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT, RegionFile,
    best_neighbor_signal,
};
use mc_worldgen::{
    END_MIN_SECTION_Y, END_SECTION_COUNT, EndGenerator, NETHER_MIN_SECTION_Y, NETHER_SECTION_COUNT,
    NetherGenerator, OverworldGenerator,
};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DimensionId {
    #[default]
    Overworld,
    Nether,
    End,
}

impl DimensionId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Overworld => "minecraft:overworld",
            Self::Nether => "minecraft:the_nether",
            Self::End => "minecraft:the_end",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "minecraft:overworld" | "overworld" => Some(Self::Overworld),
            "minecraft:the_nether" | "the_nether" | "nether" => Some(Self::Nether),
            "minecraft:the_end" | "the_end" | "end" => Some(Self::End),
            _ => None,
        }
    }

    pub fn sea_level(self) -> i32 {
        match self {
            Self::Overworld => 63,
            Self::Nether => 32,
            Self::End => 0,
        }
    }

    pub fn region_subdir(self) -> &'static str {
        match self {
            Self::Overworld => "region",
            Self::Nether => "DIM-1/region",
            Self::End => "DIM1/region",
        }
    }
}

/// Mutable multi-dimension chunk store (Overworld, Nether, The End) backed by Minecraft 26.3
/// Anvil `.mca` region files and live 26.3 terrain generators.
pub struct WorldStore {
    world_dir: PathBuf,
    region_dir: PathBuf,
    datapacks: Arc<LoadedDatapacks>,
    default_biome_id: u32,
    pub seed: i64,
    pub spawn_pos: BlockPos,
    pub hashed_seed: i64,
    pub game_time: i64,
    pub day_time: i64,
    pub raining: bool,
    pub rain_time: i32,
    pub thundering: bool,
    pub thunder_time: i32,
    pub difficulty: i32,
    pub gamerules: crate::gamerules::GameRuleStore,
    generator: Option<OverworldGenerator>,
    nether_generator: NetherGenerator,
    end_generator: EndGenerator,
    chunks: HashMap<ChunkPos, Chunk>,
    nether_chunks: HashMap<ChunkPos, Chunk>,
    end_chunks: HashMap<ChunkPos, Chunk>,
}

impl WorldStore {
    pub fn open(
        world_dir: &Path,
        datapacks: Arc<LoadedDatapacks>,
        default_seed: i64,
        generator_type: &str,
    ) -> Self {
        let default_biome_id = datapacks.biome_id("minecraft:plains");
        let region_dir = world_dir.join("region");
        let mut spawn_pos = BlockPos::new(0, -60, 0);
        let mut seed = default_seed;
        let mut game_time = 0i64;
        let mut day_time = 6000i64;
        let mut raining = false;
        let mut rain_time = 24000i32;
        let mut thundering = false;
        let mut thunder_time = 24000i32;
        let mut difficulty = 2i32; // Normal
        let mut gamerules = crate::gamerules::GameRuleStore::new();
        let mut loaded_level_dat = false;

        let level_dat = world_dir.join("level.dat");
        if level_dat.is_file() {
            match Self::read_level_dat(&level_dat, &mut gamerules) {
                Ok((sp, s, gt, dt, r, rt, th, tht, diff)) => {
                    spawn_pos = sp;
                    seed = s;
                    game_time = gt;
                    day_time = dt;
                    raining = r;
                    rain_time = rt;
                    thundering = th;
                    thunder_time = tht;
                    difficulty = diff;
                    loaded_level_dat = true;
                    info!(
                        "Loaded level.dat from {}: seed={}, spawn=({}, {}, {})",
                        level_dat.display(),
                        seed,
                        spawn_pos.x,
                        spawn_pos.y,
                        spawn_pos.z
                    );
                }
                Err(e) => {
                    warn!(
                        "Could not parse {}: {e} — using configured seed",
                        level_dat.display()
                    );
                }
            }
        }

        let is_flat = generator_type.eq_ignore_ascii_case("flat")
            || generator_type.eq_ignore_ascii_case("minecraft:flat");
        let generator = if is_flat {
            None
        } else {
            let g = OverworldGenerator::new(seed);
            if !loaded_level_dat {
                let surf = g.sample_chunk_surface_level(0, 0, 0).round() as i32;
                spawn_pos = BlockPos::new(0, surf.max(64) + 2, 0);
            }
            Some(g)
        };

        let mut h = Sha1::new();
        h.update(seed.to_be_bytes());
        let dig = h.finalize();
        let hashed_seed = i64::from_be_bytes(dig[0..8].try_into().unwrap());

        Self {
            world_dir: world_dir.to_path_buf(),
            region_dir,
            datapacks,
            default_biome_id,
            seed,
            spawn_pos,
            hashed_seed,
            game_time,
            day_time,
            raining,
            rain_time,
            thundering,
            thunder_time,
            difficulty,
            gamerules,
            generator,
            nether_generator: NetherGenerator::new(seed),
            end_generator: EndGenerator::new(seed),
            chunks: HashMap::new(),
            nether_chunks: HashMap::new(),
            end_chunks: HashMap::new(),
        }
    }

    pub fn world_dir(&self) -> &Path {
        &self.world_dir
    }

    fn read_level_dat(
        path: &Path,
        gamerules: &mut crate::gamerules::GameRuleStore,
    ) -> Result<(BlockPos, i64, i64, i64, bool, i32, bool, i32, i32), String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut gz = GzDecoder::new(file);
        let mut raw = Vec::new();
        gz.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let (_, root) = mc_nbt::read_file_bytes(&raw).map_err(|e| e.to_string())?;
        let data = root
            .get_compound("Data")
            .ok_or("Missing Data compound in level.dat")?;
        let sx = data.get_int("SpawnX").unwrap_or(0);
        let sy = data.get_int("SpawnY").unwrap_or(80);
        let sz = data.get_int("SpawnZ").unwrap_or(0);
        let seed = data
            .get_compound("WorldGenSettings")
            .and_then(|w| w.get_long("seed"))
            .unwrap_or(0);
        let game_time = data.get_long("Time").unwrap_or(0);
        let day_time = data.get_long("DayTime").unwrap_or(6000);
        let raining = data.get_byte("raining").unwrap_or(0) != 0;
        let rain_time = data.get_int("rainTime").unwrap_or(24000);
        let thundering = data.get_byte("thundering").unwrap_or(0) != 0;
        let thunder_time = data.get_int("thunderTime").unwrap_or(24000);
        let difficulty = data.get_byte("Difficulty").map(|b| b as i32).unwrap_or(2);
        if let Some(gr) = data
            .get_compound("game_rules")
            .or_else(|| data.get_compound("GameRules"))
        {
            gamerules.load_from_nbt(gr);
        }
        Ok((
            BlockPos::new(sx, sy, sz),
            seed,
            game_time,
            day_time,
            raining,
            rain_time,
            thundering,
            thunder_time,
            difficulty,
        ))
    }

    /// Write exact Minecraft 26.3 gzipped NBT `level.dat`.
    pub fn write_level_dat(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.world_dir).map_err(|e| e.to_string())?;
        let path = self.world_dir.join("level.dat");
        let tmp = self.world_dir.join("level.dat.tmp");

        let mut wgs = NbtCompound::new();
        wgs.insert("seed", NbtTag::Long(self.seed));
        wgs.insert("generate_features", NbtTag::Byte(1));

        let mut version = NbtCompound::new();
        version.insert("Id", NbtTag::Int(WORLD_VERSION));
        version.insert("Name", NbtTag::String("26.3".into()));
        version.insert("Series", NbtTag::String("main".into()));
        version.insert("Snapshot", NbtTag::Byte(0));

        let mut data = NbtCompound::new();
        data.insert("DataVersion", NbtTag::Int(WORLD_VERSION));
        data.insert("version", NbtTag::Int(19133));
        data.insert("Version", NbtTag::Compound(version));
        data.insert(
            "LevelName",
            NbtTag::String(
                self.world_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("world")
                    .to_string(),
            ),
        );
        data.insert("Time", NbtTag::Long(self.game_time));
        data.insert("DayTime", NbtTag::Long(self.day_time));
        data.insert("SpawnX", NbtTag::Int(self.spawn_pos.x));
        data.insert("SpawnY", NbtTag::Int(self.spawn_pos.y));
        data.insert("SpawnZ", NbtTag::Int(self.spawn_pos.z));
        data.insert("raining", NbtTag::Byte(if self.raining { 1 } else { 0 }));
        data.insert("rainTime", NbtTag::Int(self.rain_time));
        data.insert(
            "thundering",
            NbtTag::Byte(if self.thundering { 1 } else { 0 }),
        );
        data.insert("thunderTime", NbtTag::Int(self.thunder_time));
        data.insert("Difficulty", NbtTag::Byte(self.difficulty as i8));
        data.insert("game_rules", NbtTag::Compound(self.gamerules.save_to_nbt()));
        data.insert("WorldGenSettings", NbtTag::Compound(wgs));

        let mut root = NbtCompound::new();
        root.insert("Data", NbtTag::Compound(data));

        let raw = mc_nbt::write_file_bytes("", &root).map_err(|e| e.to_string())?;
        {
            let file = File::create(&tmp).map_err(|e| e.to_string())?;
            let mut gz = GzEncoder::new(file, Compression::default());
            gz.write_all(&raw).map_err(|e| e.to_string())?;
            gz.finish().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn biome_name_at(&self, x: i32, y: i32, z: i32) -> &'static str {
        self.biome_name_in(DimensionId::Overworld, x, y, z)
    }

    pub fn biome_name_in(&self, dim: DimensionId, x: i32, y: i32, z: i32) -> &'static str {
        match dim {
            DimensionId::Overworld => {
                if let Some(ref g) = self.generator {
                    g.sample_biome_name(x, y, z)
                } else {
                    "minecraft:plains"
                }
            }
            DimensionId::Nether => self.nether_generator.sample_biome_name(x, z),
            DimensionId::End => self.end_generator.sample_biome_name(x, z),
        }
    }

    pub fn get_or_load_chunk(&mut self, pos: ChunkPos) -> &Chunk {
        self.get_or_load_chunk_in(DimensionId::Overworld, pos)
    }

    pub fn get_or_load_chunk_in(&mut self, dim: DimensionId, pos: ChunkPos) -> &Chunk {
        match dim {
            DimensionId::Overworld => {
                if !self.chunks.contains_key(&pos) {
                    let loaded = self
                        .try_load_from_mca_in(dim, pos)
                        .unwrap_or_else(|| {
                            if let Some(ref g) = self.generator {
                                g.generate_chunk(pos, &self.datapacks)
                            } else {
                                Chunk::new_flat_overworld(pos, self.default_biome_id)
                            }
                        });
                    self.chunks.insert(pos, loaded);
                }
                self.chunks.get(&pos).unwrap()
            }
            DimensionId::Nether => {
                if !self.nether_chunks.contains_key(&pos) {
                    let loaded = self
                        .try_load_from_mca_in(dim, pos)
                        .unwrap_or_else(|| self.nether_generator.generate_chunk(pos, &self.datapacks));
                    self.nether_chunks.insert(pos, loaded);
                }
                self.nether_chunks.get(&pos).unwrap()
            }
            DimensionId::End => {
                if !self.end_chunks.contains_key(&pos) {
                    let loaded = self
                        .try_load_from_mca_in(dim, pos)
                        .unwrap_or_else(|| self.end_generator.generate_chunk(pos, &self.datapacks));
                    self.end_chunks.insert(pos, loaded);
                }
                self.end_chunks.get(&pos).unwrap()
            }
        }
    }

    pub fn get_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn get_block(&mut self, pos: BlockPos) -> BlockStateId {
        let cp = pos.chunk_pos();
        let chunk = self.get_or_load_chunk(cp);
        chunk.get_block((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }

    pub fn relight_block(&mut self, pos: BlockPos) -> DirtyLightSections {
        let cp = pos.chunk_pos();
        let _ = self.get_or_load_chunk(cp);
        LightEngine::update_at(&mut self.chunks, pos)
    }

    pub fn set_block_and_relight(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Option<(BlockStateId, DirtyLightSections)> {
        let cp = pos.chunk_pos();
        let _ = self.get_or_load_chunk(cp);
        let chunk = self.chunks.get_mut(&cp)?;
        let old = chunk.set_block((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, state)?;
        if old.block_name() != state.block_name() {
            chunk.block_entities.remove(&pos);
            if let Some(be) = Self::default_block_entity(pos, state) {
                chunk.block_entities.insert(pos, be);
            }
        }
        let dirty_light = if old != state {
            LightEngine::update_at(&mut self.chunks, pos)
        } else {
            DirtyLightSections::default()
        };
        Some((old, dirty_light))
    }

    fn default_block_entity(pos: BlockPos, state: BlockStateId) -> Option<BlockEntity> {
        match block_behavior(state.0).kind {
            KIND_CHEST => Some(BlockEntity::new_chest(pos)),
            KIND_BARREL => Some(BlockEntity::new_barrel(pos)),
            KIND_FURNACE => Some(BlockEntity::new_furnace(pos, COOK_SMELTING)),
            KIND_BLAST_FURNACE => Some(BlockEntity::new_furnace(pos, COOK_BLASTING)),
            KIND_SMOKER => Some(BlockEntity::new_furnace(pos, COOK_SMOKING)),
            KIND_HOPPER => Some(BlockEntity::new_hopper(pos)),
            KIND_SIGN => Some(BlockEntity::new_sign(pos)),
            _ => None,
        }
    }

    pub fn ensure_block_entity(&mut self, pos: BlockPos) -> Option<&mut BlockEntity> {
        let state = self.get_block(pos);
        let cp = pos.chunk_pos();
        let chunk = self.chunks.get_mut(&cp)?;
        if !chunk.block_entities.contains_key(&pos) {
            if let Some(be) = Self::default_block_entity(pos, state) {
                chunk.block_entities.insert(pos, be);
                chunk.dirty = true;
            }
        }
        chunk.block_entities.get_mut(&pos)
    }

    pub fn get_block_entity(&self, pos: BlockPos) -> Option<&BlockEntity> {
        self.chunks.get(&pos.chunk_pos())?.block_entities.get(&pos)
    }

    pub fn get_block_entity_mut(&mut self, pos: BlockPos) -> Option<&mut BlockEntity> {
        let cp = pos.chunk_pos();
        let chunk = self.chunks.get_mut(&cp)?;
        chunk.dirty = true;
        chunk.block_entities.get_mut(&pos)
    }

    pub fn take_block_entity(&mut self, pos: BlockPos) -> Option<BlockEntity> {
        let cp = pos.chunk_pos();
        let chunk = self.chunks.get_mut(&cp)?;
        let removed = chunk.block_entities.remove(&pos);
        if removed.is_some() {
            chunk.dirty = true;
        }
        removed
    }

    pub fn insert_block_entity(&mut self, be: BlockEntity) {
        let cp = be.pos.chunk_pos();
        let _ = self.get_or_load_chunk(cp);
        if let Some(chunk) = self.chunks.get_mut(&cp) {
            chunk.block_entities.insert(be.pos, be);
            chunk.dirty = true;
        }
    }

    /// Tick all loaded furnace block entities for 1 server tick.
    pub fn tick_furnaces(&mut self) -> Vec<(BlockPos, FurnaceTickResult, [i16; 4])> {
        let mut updates = Vec::new();
        for chunk in self.chunks.values_mut() {
            let mut chunk_dirty = false;
            for (&pos, be) in chunk.block_entities.iter_mut() {
                if matches!(be.kind, BlockEntityKind::Furnace { .. }) {
                    let res = be.tick_furnace();
                    if res.changed {
                        chunk_dirty = true;
                        let data = match &be.kind {
                            BlockEntityKind::Furnace {
                                lit_time,
                                lit_duration,
                                cook_progress,
                                cook_total,
                                ..
                            } => [*lit_time, *lit_duration, *cook_progress, *cook_total],
                            _ => [0; 4],
                        };
                        updates.push((pos, res, data));
                    }
                }
            }
            if chunk_dirty {
                chunk.dirty = true;
            }
        }
        updates
    }

    /// Tick all loaded hoppers (8-tick transfer cooldown, redstone lock, pull from above, push to facing).
    /// Returns positions of all containers whose contents changed.
    pub fn tick_hoppers(&mut self) -> Vec<BlockPos> {
        let hopper_positions: Vec<BlockPos> = self
            .chunks
            .values()
            .flat_map(|c| {
                c.block_entities.iter().filter_map(|(&pos, be)| {
                    if matches!(be.kind, BlockEntityKind::Hopper { .. }) {
                        Some(pos)
                    } else {
                        None
                    }
                })
            })
            .collect();

        let mut modified = Vec::new();
        for pos in hopper_positions {
            let ready = {
                let Some(be) = self.chunks.get_mut(&pos.chunk_pos()).and_then(|c| c.block_entities.get_mut(&pos)) else {
                    continue;
                };
                let BlockEntityKind::Hopper { transfer_cooldown, .. } = &mut be.kind else {
                    continue;
                };
                if *transfer_cooldown > 0 {
                    *transfer_cooldown -= 1;
                    false
                } else {
                    true
                }
            };
            if !ready {
                continue;
            }
            // Redstone-locked hoppers pause transfer
            if best_neighbor_signal(self, pos) > 0 {
                continue;
            }
            let st = self.get_block(pos);
            let facing = block_behavior(st.0).facing;
            let (dx, dy, dz) = match facing {
                0 => (0, -1, 0),
                2 => (0, 0, -1),
                3 => (0, 0, 1),
                4 => (-1, 0, 0),
                5 => (1, 0, 0),
                _ => (0, -1, 0),
            };
            let target_pos = pos.offset(dx, dy, dz);
            let mut moved = false;

            // 1. Push 1 item from hopper -> target container
            let candidate = self
                .get_block_entity(pos)
                .and_then(|be| be.items_slice())
                .and_then(|slots| slots.iter().enumerate().find(|(_, s)| !s.is_empty()).map(|(i, s)| (i, s.clone())));
            if let Some((slot_idx, item)) = candidate {
                let inserted = self
                    .get_block_entity_mut(target_pos)
                    .map(|dst| dst.try_insert_one(&item, dy == -1))
                    .unwrap_or(false);
                if inserted {
                    if let Some(src) = self.get_block_entity_mut(pos) {
                        if let Some(slots) = src.items_slice_mut() {
                            if slots[slot_idx].count <= 1 {
                                slots[slot_idx] = mc_world::ItemStack::EMPTY;
                            } else {
                                slots[slot_idx].count -= 1;
                            }
                        }
                    }
                    modified.push(pos);
                    modified.push(target_pos);
                    moved = true;
                }
            }

            // 2. Pull 1 item from container directly above -> hopper
            let above_pos = pos.offset(0, 1, 0);
            let pulled = self
                .get_block_entity_mut(above_pos)
                .and_then(|src| src.try_extract_one());
            if let Some(item) = pulled {
                let ok = self
                    .get_block_entity_mut(pos)
                    .map(|dst| dst.try_insert_one(&item, true))
                    .unwrap_or(false);
                if ok {
                    modified.push(above_pos);
                    modified.push(pos);
                    moved = true;
                } else if let Some(src) = self.get_block_entity_mut(above_pos) {
                    // Put back if hopper was full
                    let _ = src.try_insert_one(&item, true);
                }
            }

            if moved {
                if let Some(be) = self.get_block_entity_mut(pos) {
                    if let BlockEntityKind::Hopper { transfer_cooldown, .. } = &mut be.kind {
                        *transfer_cooldown = 8;
                    }
                }
            }
        }
        modified
    }

    fn save_dim_dirty(
        rdir: &Path,
        map: &mut HashMap<ChunkPos, Chunk>,
        datapacks: &LoadedDatapacks,
    ) -> Result<usize, String> {
        let dirty_positions: Vec<ChunkPos> = map
            .iter()
            .filter_map(|(&pos, c)| if c.dirty { Some(pos) } else { None })
            .collect();
        if dirty_positions.is_empty() {
            return Ok(0);
        }
        std::fs::create_dir_all(rdir).map_err(|e| e.to_string())?;

        let mut by_region: HashMap<(i32, i32), Vec<ChunkPos>> = HashMap::new();
        for pos in dirty_positions {
            by_region
                .entry((pos.region_x(), pos.region_z()))
                .or_default()
                .push(pos);
        }

        let mut saved = 0usize;
        for ((rx, rz), positions) in by_region {
            let mca_path = rdir.join(format!("r.{rx}.{rz}.mca"));
            let mut rf = RegionFile::open(&mca_path).map_err(|e| e.to_string())?;
            for pos in positions {
                if let Some(chunk) = map.get_mut(&pos) {
                    let nbt = chunk.to_nbt(datapacks);
                    rf.write_chunk_nbt(pos, &nbt).map_err(|e| e.to_string())?;
                    chunk.dirty = false;
                    saved += 1;
                }
            }
        }
        Ok(saved)
    }

    pub fn save_dirty_chunks(&mut self) -> Result<usize, String> {
        let mut total = 0usize;
        total += Self::save_dim_dirty(&self.region_dir, &mut self.chunks, &self.datapacks)?;
        let nether_dir = self.world_dir.join(DimensionId::Nether.region_subdir());
        total += Self::save_dim_dirty(&nether_dir, &mut self.nether_chunks, &self.datapacks)?;
        let end_dir = self.world_dir.join(DimensionId::End.region_subdir());
        total += Self::save_dim_dirty(&end_dir, &mut self.end_chunks, &self.datapacks)?;
        self.write_level_dat()?;
        if total > 0 {
            info!("Saved {total} dirty chunk(s) and level.dat under {}", self.world_dir.display());
        }
        Ok(total)
    }

    fn try_load_from_mca_in(&self, dim: DimensionId, pos: ChunkPos) -> Option<Chunk> {
        let rdir = match dim {
            DimensionId::Overworld => self.region_dir.clone(),
            _ => self.world_dir.join(dim.region_subdir()),
        };
        let mca_path = rdir.join(format!("r.{}.{}.mca", pos.region_x(), pos.region_z()));
        if !mca_path.is_file() {
            return None;
        }
        let mut rf = RegionFile::open_readonly(&mca_path).ok()?;
        let nbt = rf.read_chunk_nbt(pos).ok()??;
        let (min_sec, sec_cnt) = match dim {
            DimensionId::Overworld => (OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT),
            DimensionId::Nether => (NETHER_MIN_SECTION_Y, NETHER_SECTION_COUNT),
            DimensionId::End => (END_MIN_SECTION_Y, END_SECTION_COUNT),
        };
        match Chunk::from_nbt(&nbt, &self.datapacks, min_sec, sec_cnt) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(
                    "Failed to parse chunk ({}, {}) from {}: {e}",
                    pos.x,
                    pos.z,
                    mca_path.display()
                );
                None
            }
        }
    }
}

impl BlockWorld for WorldStore {
    fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
        self.get_block(pos)
    }

    fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
        let cp = pos.chunk_pos();
        let _ = self.get_or_load_chunk(cp);
        let Some(chunk) = self.chunks.get_mut(&cp) else {
            return false;
        };
        chunk
            .set_block((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, state)
            .is_some()
    }

    fn container_comparator_signal(&self, pos: BlockPos) -> u8 {
        self.get_block_entity(pos)
            .map(|be| be.comparator_signal())
            .unwrap_or(0)
    }
}
