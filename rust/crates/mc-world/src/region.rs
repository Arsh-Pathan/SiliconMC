use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use mc_math::ChunkPos;
use mc_nbt::{NbtCompound, NbtError};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

const SECTOR_BYTES: usize = 4096;
const HEADER_BYTES: usize = 8192;
const EXTERNAL_STREAM_FLAG: u8 = 128;
const LZ4_MAGIC: &[u8; 8] = b"LZ4Block";

#[derive(Debug, Error)]
pub enum RegionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("NBT error: {0}")]
    Nbt(#[from] NbtError),
    #[error("Unsupported region compression version: {0}")]
    UnsupportedCompression(u8),
    #[error("Corrupted region/LZ4 chunk data: {0}")]
    Corrupted(String),
}

/// Anvil `.mca` RegionFile reader/writer supporting Minecraft 26.3's 4 compression schemes
/// (`1=Gzip`, `2=Zlib/Deflate`, `3=None`, `4=LZ4Block`) and `.mcc` external chunks.
pub struct RegionFile {
    path: PathBuf,
    file: File,
    offsets: [u32; 1024],
    timestamps: [u32; 1024],
}

impl RegionFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RegionError> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        let len = file.metadata()?.len();
        let mut offsets = [0u32; 1024];
        let mut timestamps = [0u32; 1024];

        if len >= HEADER_BYTES as u64 {
            let mut header = [0u8; HEADER_BYTES];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header)?;
            for i in 0..1024 {
                offsets[i] = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
                let t_off = SECTOR_BYTES + i * 4;
                timestamps[i] = u32::from_be_bytes(header[t_off..t_off + 4].try_into().unwrap());
            }
        } else if len == 0 {
            let zero = [0u8; HEADER_BYTES];
            file.write_all(&zero)?;
        }

        Ok(Self {
            path,
            file,
            offsets,
            timestamps,
        })
    }

    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self, RegionError> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let mut offsets = [0u32; 1024];
        let mut timestamps = [0u32; 1024];
        let mut header = [0u8; HEADER_BYTES];
        file.read_exact(&mut header)?;
        for i in 0..1024 {
            offsets[i] = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
            let t_off = SECTOR_BYTES + i * 4;
            timestamps[i] = u32::from_be_bytes(header[t_off..t_off + 4].try_into().unwrap());
        }
        Ok(Self {
            path,
            file,
            offsets,
            timestamps,
        })
    }

    #[inline]
    fn header_index(pos: ChunkPos) -> usize {
        pos.region_local_x() + pos.region_local_z() * 32
    }

    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        self.offsets[Self::header_index(pos)] != 0
    }

    /// Read and decompress the raw NBT compound for chunk `pos`, or `Ok(None)` if not present.
    pub fn read_chunk_nbt(&mut self, pos: ChunkPos) -> Result<Option<NbtCompound>, RegionError> {
        let offset = self.offsets[Self::header_index(pos)];
        if offset == 0 {
            return Ok(None);
        }
        let sector_num = (offset >> 8) as u64;
        let num_sectors = (offset & 0xFF) as usize;
        if sector_num < 2 || num_sectors == 0 {
            return Ok(None);
        }

        self.file.seek(SeekFrom::Start(sector_num * SECTOR_BYTES as u64))?;
        let mut chunk_header = [0u8; 5];
        self.file.read_exact(&mut chunk_header)?;
        let length = u32::from_be_bytes(chunk_header[0..4].try_into().unwrap()) as usize;
        let version = chunk_header[4];
        if length == 0 {
            return Ok(None);
        }

        let compressed = if (version & EXTERNAL_STREAM_FLAG) != 0 {
            let ext_path = self
                .path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!("c.{}.{}.mcc", pos.x, pos.z));
            std::fs::read(ext_path)?
        } else {
            let stream_len = length - 1;
            let mut buf = vec![0u8; stream_len];
            self.file.read_exact(&mut buf)?;
            buf
        };

        let raw_nbt = decompress_stream(version & !EXTERNAL_STREAM_FLAG, &compressed)?;
        let (_, compound) = mc_nbt::read_file_bytes(&raw_nbt)?;
        Ok(Some(compound))
    }

    /// Write a chunk NBT compound using Zlib (compression type 2, Minecraft's default).
    pub fn write_chunk_nbt(&mut self, pos: ChunkPos, compound: &NbtCompound) -> Result<(), RegionError> {
        let raw_nbt = mc_nbt::write_file_bytes("", compound)?;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw_nbt)?;
        let compressed = encoder.finish()?;

        let total_len = 5 + compressed.len();
        let needed_sectors = total_len.div_ceil(SECTOR_BYTES);
        if needed_sectors >= 256 {
            return Err(RegionError::Corrupted("Chunk exceeds 1 MiB inline limit".into()));
        }

        let file_len = self.file.metadata()?.len() as usize;
        let start_sector = (file_len.div_ceil(SECTOR_BYTES)).max(2);
        let write_offset = (start_sector * SECTOR_BYTES) as u64;

        let mut sector_buf = vec![0u8; needed_sectors * SECTOR_BYTES];
        sector_buf[0..4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        sector_buf[4] = 2; // VERSION_DEFLATE
        sector_buf[5..5 + compressed.len()].copy_from_slice(&compressed);

        self.file.seek(SeekFrom::Start(write_offset))?;
        self.file.write_all(&sector_buf)?;

        let idx = Self::header_index(pos);
        let packed_offset = ((start_sector as u32) << 8) | (needed_sectors as u32);
        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        self.offsets[idx] = packed_offset;
        self.timestamps[idx] = now_sec;

        self.file.seek(SeekFrom::Start((idx * 4) as u64))?;
        self.file.write_all(&packed_offset.to_be_bytes())?;
        self.file.seek(SeekFrom::Start((SECTOR_BYTES + idx * 4) as u64))?;
        self.file.write_all(&now_sec.to_be_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

fn decompress_stream(version: u8, data: &[u8]) -> Result<Vec<u8>, RegionError> {
    match version {
        1 => {
            let mut dec = GzDecoder::new(Cursor::new(data));
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        2 => {
            let mut dec = ZlibDecoder::new(Cursor::new(data));
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        3 => Ok(data.to_vec()),
        4 => decompress_lz4_java_blocks(data),
        other => Err(RegionError::UnsupportedCompression(other)),
    }
}

/// Decompress `net.jpountz.lz4.LZ4BlockInputStream` format used by Minecraft 26.3 (`RegionFileVersion.VERSION_LZ4`).
/// Header per block: 8B magic `"LZ4Block"`, 1B token (`method | compressionLevel`),
/// 4B LE compressedLen, 4B LE decompressedLen, 4B LE xxhash32 checksum.
fn decompress_lz4_java_blocks(data: &[u8]) -> Result<Vec<u8>, RegionError> {
    let mut cur = Cursor::new(data);
    let mut out = Vec::new();
    let mut header = [0u8; 21];

    loop {
        match cur.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(RegionError::Io(e)),
        }
        if &header[0..8] != LZ4_MAGIC {
            return Err(RegionError::Corrupted("Invalid LZ4Block magic".into()));
        }
        let token = header[8];
        let method = token & 0xF0;
        let comp_len = u32::from_le_bytes(header[9..13].try_into().unwrap()) as usize;
        let decomp_len = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
        if decomp_len == 0 && comp_len == 0 {
            break; // End-of-stream block
        }
        let mut block = vec![0u8; comp_len];
        cur.read_exact(&mut block)?;
        match method {
            0x10 => {
                // RAW (uncompressed)
                out.extend_from_slice(&block);
            }
            0x20 => {
                // LZ4 compressed
                let decompressed = lz4_flex::block::decompress(&block, decomp_len)
                    .map_err(|e| RegionError::Corrupted(format!("LZ4 block decode failed: {e}")))?;
                out.extend_from_slice(&decompressed);
            }
            other => {
                return Err(RegionError::Corrupted(format!(
                    "Unknown LZ4Block method: {other:#x}"
                )));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chunk;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use mc_data::{BlockStateId, LoadedDatapacks};
    use mc_nbt::NbtTag;
    use mc_oracle::{OracleClient, with_oracle};
    use serde_json::json;

    #[test]
    fn diff_region_file_roundtrip_against_java_oracle() {
        let tmp_dir = std::env::temp_dir().join("siliconmc_mca_test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let mca_path = tmp_dir.join("r.0.0.mca");
        let _ = std::fs::remove_file(&mca_path);

        // Build a synthetic 26.3-style chunk compound, write with Rust RegionFile, read with Java RegionFile!
        let mut chunk_tag = NbtCompound::new();
        chunk_tag.insert("DataVersion", NbtTag::Int(5023));
        chunk_tag.insert("xPos", NbtTag::Int(3));
        chunk_tag.insert("zPos", NbtTag::Int(5));
        chunk_tag.insert("yPos", NbtTag::Int(-4));
        chunk_tag.insert("Status", NbtTag::String("minecraft:full".into()));

        let mut sec0 = NbtCompound::new();
        sec0.insert("Y", NbtTag::Byte(-4));
        let mut bs = NbtCompound::new();
        let mut pal0 = NbtCompound::new();
        pal0.insert("Name", NbtTag::String("minecraft:stone".into()));
        bs.insert("palette", NbtTag::List(vec![NbtTag::Compound(pal0)]));
        sec0.insert("block_states", NbtTag::Compound(bs));
        let mut bio = NbtCompound::new();
        bio.insert("palette", NbtTag::List(vec![NbtTag::String("minecraft:plains".into())]));
        sec0.insert("biomes", NbtTag::Compound(bio));
        chunk_tag.insert("sections", NbtTag::List(vec![NbtTag::Compound(sec0)]));

        {
            let mut rf = RegionFile::open(&mca_path).unwrap();
            rf.write_chunk_nbt(ChunkPos::new(3, 5), &chunk_tag).unwrap();
        }

        // Java reads our Rust-written .mca file!
        let resp = with_oracle(|o| {
            o.call(json!({
                "op": "mca_read_chunk",
                "path": mca_path.to_string_lossy(),
                "cx": 3,
                "cz": 5
            }))
        })
        .unwrap();
        assert_eq!(resp["present"].as_bool(), Some(true));
        assert_eq!(resp["data_version"].as_i64(), Some(5023));
        assert_eq!(resp["status"].as_str(), Some("minecraft:full"));

        // Re-decode Java's returned NBT bytes in Rust and verify Chunk::from_nbt
        let java_nbt_bytes = B64.decode(resp["bytes_b64"].as_str().unwrap()).unwrap();
        let (_, java_compound) = mc_nbt::read_file_bytes(&java_nbt_bytes).unwrap();

        let data_dir = OracleClient::repo_root()
            .join("src")
            .join("main")
            .join("resources")
            .join("data");
        let datapacks = LoadedDatapacks::load_from_data_dir(&data_dir).unwrap();
        let chunk = Chunk::from_nbt(&java_compound, &datapacks, -4, 24).unwrap();
        assert_eq!(chunk.pos, ChunkPos::new(3, 5));
        assert_eq!(chunk.get_block(0, -64, 0), BlockStateId::STONE);
        assert_eq!(chunk.get_block(15, -49, 15), BlockStateId::STONE);
        assert_eq!(chunk.get_block(0, -48, 0), BlockStateId::AIR);

        // Now test full Chunk mutation -> LightEngine -> Chunk::to_nbt -> .mca -> Java RegionFile -> Chunk::from_nbt!
        let mut edited = Chunk::new_flat_overworld(ChunkPos::new(2, 4), datapacks.biome_id("minecraft:plains"));
        let torch = mc_data::default_state_id("minecraft:torch").unwrap();
        let button_state = BlockStateId(12514); // minecraft:acacia_button[face=floor,facing=north,powered=true]
        edited.set_block(4, -60, 7, torch);
        edited.set_block(5, -60, 7, button_state);
        crate::LightEngine::update_at(&mut edited, mc_math::BlockPos::new(32 + 4, -60, 64 + 7));
        assert!(edited.dirty);
        assert_eq!(edited.get_block_light(4, -60, 7), 14);

        let edited_nbt = edited.to_nbt(&datapacks);
        {
            let mut rf = RegionFile::open(&mca_path).unwrap();
            rf.write_chunk_nbt(ChunkPos::new(2, 4), &edited_nbt).unwrap();
        }
        let resp2 = with_oracle(|o| {
            o.call(json!({
                "op": "mca_read_chunk",
                "path": mca_path.to_string_lossy(),
                "cx": 2,
                "cz": 4
            }))
        })
        .unwrap();
        assert_eq!(resp2["present"].as_bool(), Some(true));
        assert_eq!(resp2["data_version"].as_i64(), Some(5023));
        let java_bytes2 = B64.decode(resp2["bytes_b64"].as_str().unwrap()).unwrap();
        let (_, java_cmp2) = mc_nbt::read_file_bytes(&java_bytes2).unwrap();
        let reloaded = Chunk::from_nbt(&java_cmp2, &datapacks, -4, 24).unwrap();
        assert_eq!(reloaded.get_block(4, -60, 7), torch);
        assert_eq!(reloaded.get_block(5, -60, 7), button_state);
        assert_eq!(reloaded.get_block_light(4, -60, 7), 14);
        assert_eq!(reloaded.get_block_light(5, -60, 7), 13);
        assert_eq!(reloaded.heightmaps, edited.heightmaps);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
