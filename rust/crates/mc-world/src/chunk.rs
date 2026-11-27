use crate::{BlockEntity, LightEngine, PaletteStrategy, PalettedStorage, SimpleBitStorage};
use mc_data::{
    BlockStateId, LoadedDatapacks, TOTAL_BLOCK_STATES, WORLD_VERSION, lookup_block_state_from_nbt,
};
use mc_math::{BlockPos, ChunkPos, ceillog2};
use mc_nbt::{NbtCompound, NbtTag};
use std::collections::BTreeMap;

/// Overworld defaults for Minecraft 26.3: Y = -64..320 (24 sections: section Y = -4..=19).
pub const OVERWORLD_MIN_SECTION_Y: i32 = -4;
pub const OVERWORLD_SECTION_COUNT: usize = 24;

#[derive(Debug, Clone)]
pub struct ChunkSection {
    pub y: i8,
    pub non_empty_block_count: i16,
    pub fluid_count: i16,
    pub block_states: PalettedStorage,
    pub biomes: PalettedStorage,
    pub sky_light: Option<Vec<u8>>,
    pub block_light: Option<Vec<u8>>,
}

impl ChunkSection {
    pub fn empty(y: i8, default_biome_id: u32) -> Self {
        Self {
            y,
            non_empty_block_count: 0,
            fluid_count: 0,
            block_states: PalettedStorage::single(BlockStateId::AIR.0, 4096),
            biomes: PalettedStorage::single(default_biome_id, 64),
            sky_light: None,
            block_light: None,
        }
    }

    #[inline]
    pub fn block_index(x: usize, y: usize, z: usize) -> usize {
        ((y & 15) << 8) | ((z & 15) << 4) | (x & 15)
    }

    #[inline]
    pub fn get_block(&self, x: usize, y: usize, z: usize) -> BlockStateId {
        BlockStateId(self.block_states.get(Self::block_index(x, y, z)))
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, state: BlockStateId) -> BlockStateId {
        let strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let old = BlockStateId(self.block_states.set(Self::block_index(x, y, z), state.0, strat));
        if old != state {
            if !old.is_air() {
                self.non_empty_block_count -= 1;
                if old.is_fluid_block() {
                    self.fluid_count -= 1;
                }
            }
            if !state.is_air() {
                self.non_empty_block_count += 1;
                if state.is_fluid_block() {
                    self.fluid_count += 1;
                }
            }
        }
        old
    }

    pub fn recalc_counts(&mut self) {
        let mut non_empty = 0i16;
        let mut fluids = 0i16;
        for i in 0..4096 {
            let state = BlockStateId(self.block_states.get(i));
            if !state.is_air() {
                non_empty += 1;
                if state.is_fluid_block() {
                    fluids += 1;
                }
            }
        }
        self.non_empty_block_count = non_empty;
        self.fluid_count = fluids;
    }

    /// Write using Minecraft 26.3 `LevelChunkSection.write(FriendlyByteBuf)`:
    /// `i16 nonEmptyBlockCount` + `i16 fluidCount` + `blockStates` + `biomes`.
    pub fn write_network(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.non_empty_block_count.to_be_bytes());
        out.extend_from_slice(&self.fluid_count.to_be_bytes());
        self.block_states.write_network(out);
        self.biomes.write_network(out);
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    pub min_section_y: i32,
    pub status: String,
    pub sections: Vec<ChunkSection>,
    /// Client-relevant heightmaps keyed by `Heightmap.Types` protocol ID
    /// (`WORLD_SURFACE = 1`, `MOTION_BLOCKING = 4`, `MOTION_BLOCKING_NO_LEAVES = 5`).
    pub heightmaps: BTreeMap<i32, Vec<i64>>,
    /// Light sections covering `min_section_y - 1 ..= min_section_y + sections.len()` (26 sections for Overworld).
    pub extra_sky_light_below: Option<Vec<u8>>,
    pub extra_sky_light_above: Option<Vec<u8>>,
    pub extra_block_light_below: Option<Vec<u8>>,
    pub extra_block_light_above: Option<Vec<u8>>,
    pub block_entities: BTreeMap<BlockPos, BlockEntity>,
    pub dirty: bool,
}

#[inline]
pub fn get_nibble(layer: &[u8], x: usize, y: usize, z: usize) -> u8 {
    let idx = ChunkSection::block_index(x, y, z);
    let byte = layer[idx >> 1];
    if (idx & 1) == 0 {
        byte & 0x0F
    } else {
        (byte >> 4) & 0x0F
    }
}

#[inline]
pub fn set_nibble(layer: &mut [u8], x: usize, y: usize, z: usize, val: u8) {
    let idx = ChunkSection::block_index(x, y, z);
    let b = &mut layer[idx >> 1];
    if (idx & 1) == 0 {
        *b = (*b & 0xF0) | (val & 0x0F);
    } else {
        *b = (*b & 0x0F) | ((val & 0x0F) << 4);
    }
}

impl Chunk {
    #[inline]
    pub fn min_y(&self) -> i32 {
        self.min_section_y << 4
    }

    #[inline]
    pub fn max_y(&self) -> i32 {
        (self.min_section_y + self.sections.len() as i32) << 4
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.sections.len() * 16
    }

    /// Parse a real Minecraft 26.3 Anvil chunk NBT compound (`SerializableChunkData.parse`).
    pub fn from_nbt(
        root: &NbtCompound,
        datapacks: &LoadedDatapacks,
        min_section_y: i32,
        section_count: usize,
    ) -> Result<Self, String> {
        let x = root.get_int("xPos").unwrap_or(0);
        let z = root.get_int("zPos").unwrap_or(0);
        let status = root.get_string("Status").unwrap_or("minecraft:empty").to_string();

        let default_biome = datapacks.biome_id("minecraft:plains");
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let biome_strat = PaletteStrategy::for_biomes(datapacks.biome_count());

        let mut sections: Vec<ChunkSection> = (0..section_count)
            .map(|i| ChunkSection::empty((min_section_y + i as i32) as i8, default_biome))
            .collect();

        let mut extra_sky_light_below = None;
        let mut extra_sky_light_above = None;
        let mut extra_block_light_below = None;
        let mut extra_block_light_above = None;

        if let Some(sec_list) = root.get_list("sections") {
            for tag in sec_list {
                let Some(sec_tag) = tag.as_compound() else {
                    continue;
                };
                let sy = sec_tag.get_byte("Y").unwrap_or(0) as i32;
                let sky_light = sec_tag
                    .get_byte_array("SkyLight")
                    .map(|a| a.iter().map(|&b| b as u8).collect::<Vec<u8>>());
                let block_light = sec_tag
                    .get_byte_array("BlockLight")
                    .map(|a| a.iter().map(|&b| b as u8).collect::<Vec<u8>>());

                if sy == min_section_y - 1 {
                    extra_sky_light_below = sky_light;
                    extra_block_light_below = block_light;
                    continue;
                }
                if sy == min_section_y + section_count as i32 {
                    extra_sky_light_above = sky_light;
                    extra_block_light_above = block_light;
                    continue;
                }
                if sy < min_section_y || sy >= min_section_y + section_count as i32 {
                    continue;
                }

                let idx = (sy - min_section_y) as usize;
                let mut section = ChunkSection::empty(sy as i8, default_biome);

                if let Some(bs_tag) = sec_tag.get_compound("block_states") {
                    if let Some(pal_list) = bs_tag.get_list("palette") {
                        let mut pal_ids = Vec::with_capacity(pal_list.len());
                        for p in pal_list {
                            let id = p
                                .as_compound()
                                .and_then(lookup_block_state_from_nbt)
                                .unwrap_or(BlockStateId::AIR);
                            pal_ids.push(id.0);
                        }
                        if !pal_ids.is_empty() {
                            section.block_states = PalettedStorage::from_disk_palette(
                                pal_ids,
                                bs_tag.get_long_array("data"),
                                block_strat,
                            )?;
                        }
                    }
                }

                if let Some(bio_tag) = sec_tag.get_compound("biomes") {
                    if let Some(pal_list) = bio_tag.get_list("palette") {
                        let mut pal_ids = Vec::with_capacity(pal_list.len());
                        for p in pal_list {
                            let bname = match p {
                                NbtTag::String(s) => s.as_str(),
                                _ => "minecraft:plains",
                            };
                            pal_ids.push(datapacks.biome_id(bname));
                        }
                        if !pal_ids.is_empty() {
                            section.biomes = PalettedStorage::from_disk_palette(
                                pal_ids,
                                bio_tag.get_long_array("data"),
                                biome_strat,
                            )?;
                        }
                    }
                }

                section.sky_light = sky_light;
                section.block_light = block_light;
                section.recalc_counts();
                sections[idx] = section;
            }
        }

        let mut heightmaps = BTreeMap::new();
        if let Some(hm_tag) = root.get_compound("Heightmaps") {
            for (key, proto_id) in [
                ("WORLD_SURFACE", 1),
                ("MOTION_BLOCKING", 4),
                ("MOTION_BLOCKING_NO_LEAVES", 5),
            ] {
                if let Some(arr) = hm_tag.get_long_array(key) {
                    heightmaps.insert(proto_id, arr.to_vec());
                }
            }
        }

        let mut block_entities = BTreeMap::new();
        if let Some(be_list) = root.get_list("block_entities") {
            for tag in be_list {
                if let Some(c) = tag.as_compound() {
                    if let Some(be) = BlockEntity::load_from_nbt(c) {
                        block_entities.insert(be.pos, be);
                    }
                }
            }
        }

        let mut chunk = Self {
            pos: ChunkPos::new(x, z),
            min_section_y,
            status,
            sections,
            heightmaps,
            extra_sky_light_below,
            extra_sky_light_above,
            extra_block_light_below,
            extra_block_light_above,
            block_entities,
            dirty: false,
        };
        if chunk.heightmaps.is_empty() {
            chunk.recalc_all_heightmaps();
        }
        Ok(chunk)
    }

    /// Serialize this chunk back to a valid Minecraft 26.3 Anvil NBT compound (`DataVersion: 5023`).
    pub fn to_nbt(&self, datapacks: &LoadedDatapacks) -> NbtCompound {
        let mut root = NbtCompound::new();
        root.insert("DataVersion", NbtTag::Int(WORLD_VERSION));
        root.insert("xPos", NbtTag::Int(self.pos.x));
        root.insert("zPos", NbtTag::Int(self.pos.z));
        root.insert("yPos", NbtTag::Int(self.min_section_y));
        root.insert("Status", NbtTag::String(self.status.clone()));
        root.insert("LastUpdate", NbtTag::Long(0));
        root.insert("InhabitedTime", NbtTag::Long(0));
        root.insert("isLightOn", NbtTag::Byte(1));

        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let biome_strat = PaletteStrategy::for_biomes(datapacks.biome_count());

        let mut sections_list = Vec::with_capacity(self.sections.len() + 2);

        if self.extra_sky_light_below.is_some() || self.extra_block_light_below.is_some() {
            let mut s = NbtCompound::new();
            s.insert("Y", NbtTag::Byte((self.min_section_y - 1) as i8));
            if let Some(ref l) = self.extra_sky_light_below {
                s.insert("SkyLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            if let Some(ref l) = self.extra_block_light_below {
                s.insert("BlockLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            sections_list.push(NbtTag::Compound(s));
        }

        for sec in &self.sections {
            let mut s = NbtCompound::new();
            s.insert("Y", NbtTag::Byte(sec.y));

            let (bs_pal, bs_data) = sec.block_states.to_disk_palette(block_strat);
            let mut bs = NbtCompound::new();
            let pal_tags: Vec<NbtTag> = bs_pal
                .into_iter()
                .map(|id| NbtTag::Compound(BlockStateId(id).to_nbt()))
                .collect();
            bs.insert("palette", NbtTag::List(pal_tags));
            if let Some(longs) = bs_data {
                bs.insert("data", NbtTag::LongArray(longs));
            }
            s.insert("block_states", NbtTag::Compound(bs));

            let (bio_pal, bio_data) = sec.biomes.to_disk_palette(biome_strat);
            let mut bio = NbtCompound::new();
            let bio_tags: Vec<NbtTag> = bio_pal
                .into_iter()
                .map(|id| NbtTag::String(datapacks.biome_name(id).to_string()))
                .collect();
            bio.insert("palette", NbtTag::List(bio_tags));
            if let Some(longs) = bio_data {
                bio.insert("data", NbtTag::LongArray(longs));
            }
            s.insert("biomes", NbtTag::Compound(bio));

            if let Some(ref l) = sec.sky_light {
                s.insert("SkyLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            if let Some(ref l) = sec.block_light {
                s.insert("BlockLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            sections_list.push(NbtTag::Compound(s));
        }

        if self.extra_sky_light_above.is_some() || self.extra_block_light_above.is_some() {
            let mut s = NbtCompound::new();
            s.insert(
                "Y",
                NbtTag::Byte((self.min_section_y + self.sections.len() as i32) as i8),
            );
            if let Some(ref l) = self.extra_sky_light_above {
                s.insert("SkyLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            if let Some(ref l) = self.extra_block_light_above {
                s.insert("BlockLight", NbtTag::ByteArray(l.iter().map(|&b| b as i8).collect()));
            }
            sections_list.push(NbtTag::Compound(s));
        }

        root.insert("sections", NbtTag::List(sections_list));

        let mut hm_compound = NbtCompound::new();
        for (&proto_id, arr) in &self.heightmaps {
            let key = match proto_id {
                1 => "WORLD_SURFACE",
                4 => "MOTION_BLOCKING",
                5 => "MOTION_BLOCKING_NO_LEAVES",
                _ => continue,
            };
            hm_compound.insert(key, NbtTag::LongArray(arr.clone()));
        }
        root.insert("Heightmaps", NbtTag::Compound(hm_compound));
        let be_tags: Vec<NbtTag> = self
            .block_entities
            .values()
            .map(|be| NbtTag::Compound(be.save_to_nbt()))
            .collect();
        root.insert("block_entities", NbtTag::List(be_tags));
        root
    }

    /// Create a lit superflat Overworld chunk (bedrock at -64, dirt -63..-62, grass at -61).
    pub fn new_flat_overworld(pos: ChunkPos, default_biome_id: u32) -> Self {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let mut sections: Vec<ChunkSection> = (0..OVERWORLD_SECTION_COUNT)
            .map(|i| ChunkSection::empty((OVERWORLD_MIN_SECTION_Y + i as i32) as i8, default_biome_id))
            .collect();

        let mut flat_blocks = vec![BlockStateId::AIR.0; 4096];
        for z in 0..16 {
            for x in 0..16 {
                flat_blocks[ChunkSection::block_index(x, 0, z)] = BlockStateId::BEDROCK.0;
                flat_blocks[ChunkSection::block_index(x, 1, z)] = BlockStateId::DIRT.0;
                flat_blocks[ChunkSection::block_index(x, 2, z)] = BlockStateId::DIRT.0;
                flat_blocks[ChunkSection::block_index(x, 3, z)] = BlockStateId::GRASS_BLOCK.0;
            }
        }
        sections[0].block_states = PalettedStorage::from_flat(&flat_blocks, block_strat);
        sections[0].recalc_counts();

        let mut chunk = Self {
            pos,
            min_section_y: OVERWORLD_MIN_SECTION_Y,
            status: "minecraft:full".into(),
            sections,
            heightmaps: BTreeMap::new(),
            extra_sky_light_below: None,
            extra_sky_light_above: Some(vec![0xFF; 2048]),
            extra_block_light_below: None,
            extra_block_light_above: None,
            block_entities: BTreeMap::new(),
            dirty: false,
        };
        chunk.recalc_all_heightmaps();
        LightEngine::compute_initial_chunk_light(&mut chunk);
        chunk
    }

    pub fn get_block(&self, local_x: usize, world_y: i32, local_z: usize) -> BlockStateId {
        let sec_y = world_y >> 4;
        let idx = sec_y - self.min_section_y;
        if idx < 0 || (idx as usize) >= self.sections.len() {
            return BlockStateId::AIR;
        }
        self.sections[idx as usize].get_block(local_x, (world_y & 15) as usize, local_z)
    }

    /// Set a block in this chunk, incrementally updating section counts, heightmaps, and `dirty`.
    /// Returns `Some(previous_state)` if `world_y` is inside the chunk height range, else `None`.
    pub fn set_block(
        &mut self,
        local_x: usize,
        world_y: i32,
        local_z: usize,
        state: BlockStateId,
    ) -> Option<BlockStateId> {
        let sec_y = world_y >> 4;
        let idx = sec_y - self.min_section_y;
        if idx < 0 || (idx as usize) >= self.sections.len() {
            return None;
        }
        let old = self.sections[idx as usize].set_block(
            local_x & 15,
            (world_y & 15) as usize,
            local_z & 15,
            state,
        );
        if old != state {
            self.dirty = true;
            self.recalc_column_heightmaps(local_x & 15, local_z & 15);
        }
        Some(old)
    }

    /// Recompute `WORLD_SURFACE(1)`, `MOTION_BLOCKING(4)`, and `MOTION_BLOCKING_NO_LEAVES(5)`
    /// matching Minecraft 26.3's `Heightmap.java` exactly.
    pub fn recalc_all_heightmaps(&mut self) {
        let height_bits = ceillog2((self.height() + 1) as i32) as u8;
        let mut ws = SimpleBitStorage::new(height_bits, 256);
        let mut mb = SimpleBitStorage::new(height_bits, 256);
        let mut mbnl = SimpleBitStorage::new(height_bits, 256);

        let min_y = self.min_y();
        let max_y = self.max_y();

        for z in 0..16 {
            for x in 0..16 {
                let col_idx = x + z * 16;
                let mut h_ws = 0u32;
                let mut h_mb = 0u32;
                let mut h_mbnl = 0u32;

                for y in (min_y..max_y).rev() {
                    let st = self.get_block(x, y, z);
                    if st == BlockStateId::AIR {
                        continue;
                    }
                    let props = st.props();
                    let rel = (y + 1 - min_y) as u32;
                    if h_ws == 0 && !props.is_air() {
                        h_ws = rel;
                    }
                    if h_mb == 0 && props.is_motion_blocking() {
                        h_mb = rel;
                    }
                    if h_mbnl == 0 && props.is_motion_blocking_no_leaves() {
                        h_mbnl = rel;
                    }
                    if h_ws != 0 && h_mb != 0 && h_mbnl != 0 {
                        break;
                    }
                }
                ws.set(col_idx, h_ws);
                mb.set(col_idx, h_mb);
                mbnl.set(col_idx, h_mbnl);
            }
        }

        self.heightmaps
            .insert(1, ws.raw().iter().map(|&u| u as i64).collect());
        self.heightmaps
            .insert(4, mb.raw().iter().map(|&u| u as i64).collect());
        self.heightmaps
            .insert(5, mbnl.raw().iter().map(|&u| u as i64).collect());
    }

    fn recalc_column_heightmaps(&mut self, x: usize, z: usize) {
        let height_bits = ceillog2((self.height() + 1) as i32) as u8;
        let expected_words = 256usize.div_ceil(64 / height_bits as usize);
        if self.heightmaps.get(&1).map(|v| v.len()) != Some(expected_words)
            || self.heightmaps.get(&4).map(|v| v.len()) != Some(expected_words)
            || self.heightmaps.get(&5).map(|v| v.len()) != Some(expected_words)
        {
            self.recalc_all_heightmaps();
            return;
        }

        let min_y = self.min_y();
        let max_y = self.max_y();
        let mut h_ws = 0u32;
        let mut h_mb = 0u32;
        let mut h_mbnl = 0u32;

        for y in (min_y..max_y).rev() {
            let st = self.get_block(x, y, z);
            if st == BlockStateId::AIR {
                continue;
            }
            let props = st.props();
            let rel = (y + 1 - min_y) as u32;
            if h_ws == 0 && !props.is_air() {
                h_ws = rel;
            }
            if h_mb == 0 && props.is_motion_blocking() {
                h_mb = rel;
            }
            if h_mbnl == 0 && props.is_motion_blocking_no_leaves() {
                h_mbnl = rel;
            }
            if h_ws != 0 && h_mb != 0 && h_mbnl != 0 {
                break;
            }
        }

        let col_idx = x + z * 16;
        for (hm_id, val) in [(1, h_ws), (4, h_mb), (5, h_mbnl)] {
            if let Some(arr) = self.heightmaps.get_mut(&hm_id) {
                let raw: Vec<u64> = arr.iter().map(|&v| v as u64).collect();
                if let Ok(mut bs) = SimpleBitStorage::from_raw(height_bits, 256, raw) {
                    bs.set(col_idx, val);
                    *arr = bs.raw().iter().map(|&u| u as i64).collect();
                }
            }
        }
    }

    pub fn get_sky_light(&self, local_x: usize, world_y: i32, local_z: usize) -> u8 {
        let sec_y = world_y >> 4;
        let rel = sec_y - self.min_section_y;
        let layer = if rel == -1 {
            self.extra_sky_light_below.as_deref()
        } else if rel == self.sections.len() as i32 {
            self.extra_sky_light_above.as_deref()
        } else if rel >= 0 && (rel as usize) < self.sections.len() {
            self.sections[rel as usize].sky_light.as_deref()
        } else if rel > self.sections.len() as i32 {
            return 15;
        } else {
            return 0;
        };
        match layer {
            Some(buf) if buf.len() == 2048 => get_nibble(buf, local_x, (world_y & 15) as usize, local_z),
            _ => 0,
        }
    }

    pub fn set_sky_light(&mut self, local_x: usize, world_y: i32, local_z: usize, val: u8) -> Option<usize> {
        let sec_y = world_y >> 4;
        let rel = sec_y - self.min_section_y;
        let (light_sec_idx, slot) = if rel == -1 {
            (0usize, &mut self.extra_sky_light_below)
        } else if rel == self.sections.len() as i32 {
            (self.sections.len() + 1, &mut self.extra_sky_light_above)
        } else if rel >= 0 && (rel as usize) < self.sections.len() {
            (rel as usize + 1, &mut self.sections[rel as usize].sky_light)
        } else {
            return None;
        };
        let buf = slot.get_or_insert_with(|| vec![0u8; 2048]);
        if buf.len() != 2048 {
            buf.resize(2048, 0);
        }
        set_nibble(buf, local_x, (world_y & 15) as usize, local_z, val);
        Some(light_sec_idx)
    }

    pub fn get_block_light(&self, local_x: usize, world_y: i32, local_z: usize) -> u8 {
        let sec_y = world_y >> 4;
        let rel = sec_y - self.min_section_y;
        let layer = if rel == -1 {
            self.extra_block_light_below.as_deref()
        } else if rel == self.sections.len() as i32 {
            self.extra_block_light_above.as_deref()
        } else if rel >= 0 && (rel as usize) < self.sections.len() {
            self.sections[rel as usize].block_light.as_deref()
        } else {
            return 0;
        };
        match layer {
            Some(buf) if buf.len() == 2048 => get_nibble(buf, local_x, (world_y & 15) as usize, local_z),
            _ => 0,
        }
    }

    pub fn set_block_light(&mut self, local_x: usize, world_y: i32, local_z: usize, val: u8) -> Option<usize> {
        let sec_y = world_y >> 4;
        let rel = sec_y - self.min_section_y;
        let (light_sec_idx, slot) = if rel == -1 {
            (0usize, &mut self.extra_block_light_below)
        } else if rel == self.sections.len() as i32 {
            (self.sections.len() + 1, &mut self.extra_block_light_above)
        } else if rel >= 0 && (rel as usize) < self.sections.len() {
            (rel as usize + 1, &mut self.sections[rel as usize].block_light)
        } else {
            return None;
        };
        let buf = slot.get_or_insert_with(|| vec![0u8; 2048]);
        if buf.len() != 2048 {
            buf.resize(2048, 0);
        }
        set_nibble(buf, local_x, (world_y & 15) as usize, local_z, val);
        Some(light_sec_idx)
    }

    pub fn encode_network_sections(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.sections.len() * 256);
        for sec in &self.sections {
            sec.write_network(&mut buf);
        }
        buf
    }

    /// Collect light section bitsets + 2048-byte layers matching `ClientboundLightUpdatePacketData`.
    pub fn collect_light_data(&self) -> (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>, Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let all_bits = (1u64 << (self.sections.len() + 2)) - 1;
        self.collect_light_data_for_mask(all_bits, all_bits)
    }

    /// Collect light update data restricted to dirty section bitmasks (bit `i` = light section `min_section_y - 1 + i`).
    pub fn collect_light_data_for_mask(
        &self,
        sky_filter: u64,
        block_filter: u64,
    ) -> (Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>, Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let total_light_sections = self.sections.len() + 2;
        let mut sky_mask: u64 = 0;
        let mut block_mask: u64 = 0;
        let mut empty_sky_mask: u64 = 0;
        let mut empty_block_mask: u64 = 0;
        let mut sky_updates = Vec::new();
        let mut block_updates = Vec::new();

        for i in 0..total_light_sections {
            let bit = 1u64 << i;
            let (sky, block) = if i == 0 {
                (self.extra_sky_light_below.as_deref(), self.extra_block_light_below.as_deref())
            } else if i == total_light_sections - 1 {
                (self.extra_sky_light_above.as_deref(), self.extra_block_light_above.as_deref())
            } else {
                let s = &self.sections[i - 1];
                (s.sky_light.as_deref(), s.block_light.as_deref())
            };

            if (sky_filter & bit) != 0 {
                if let Some(layer) = sky {
                    if layer.len() == 2048 {
                        if layer.iter().all(|&b| b == 0) {
                            empty_sky_mask |= bit;
                        } else {
                            sky_mask |= bit;
                            sky_updates.push(layer.to_vec());
                        }
                    }
                }
            }
            if (block_filter & bit) != 0 {
                if let Some(layer) = block {
                    if layer.len() == 2048 {
                        if layer.iter().all(|&b| b == 0) {
                            empty_block_mask |= bit;
                        } else {
                            block_mask |= bit;
                            block_updates.push(layer.to_vec());
                        }
                    }
                }
            }
        }

        let to_bitset = |v: u64| if v == 0 { Vec::new() } else { vec![v] };
        (
            to_bitset(sky_mask),
            to_bitset(block_mask),
            to_bitset(empty_sky_mask),
            to_bitset(empty_block_mask),
            sky_updates,
            block_updates,
        )
    }
}
