use crate::SimpleBitStorage;
use mc_math::ceillog2;

/// Vanilla-compatible paletted storage for chunk section blocks (4096 entries) or biomes (64 entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PalettedStorage {
    Single {
        value: u32,
        entry_count: usize,
    },
    Indirect {
        bits: u8,
        palette: Vec<u32>,
        storage: SimpleBitStorage,
    },
    Direct {
        bits: u8,
        storage: SimpleBitStorage,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct PaletteStrategy {
    pub entry_count: usize,
    pub min_indirect_bits: u8,
    pub max_indirect_bits: u8,
    pub global_bits: u8,
}

impl PaletteStrategy {
    pub fn for_block_states(total_states: usize) -> Self {
        Self {
            entry_count: 4096,
            min_indirect_bits: 4,
            max_indirect_bits: 8,
            global_bits: ceillog2(total_states as i32) as u8,
        }
    }

    pub fn for_biomes(total_biomes: usize) -> Self {
        Self {
            entry_count: 64,
            min_indirect_bits: 1,
            max_indirect_bits: 3,
            global_bits: (ceillog2(total_biomes.max(2) as i32) as u8).max(1),
        }
    }
}

impl PalettedStorage {
    pub fn single(value: u32, entry_count: usize) -> Self {
        Self::Single { value, entry_count }
    }

    pub fn from_flat(values: &[u32], strategy: PaletteStrategy) -> Self {
        assert_eq!(values.len(), strategy.entry_count);
        let mut palette: Vec<u32> = Vec::new();
        let mut indices: Vec<u32> = Vec::with_capacity(values.len());
        let mut too_many = false;
        let max_palette_len = 1usize << strategy.max_indirect_bits;

        for &v in values {
            if let Some(pos) = palette.iter().position(|&p| p == v) {
                indices.push(pos as u32);
            } else {
                if palette.len() >= max_palette_len {
                    too_many = true;
                    break;
                }
                let id = palette.len() as u32;
                palette.push(v);
                indices.push(id);
            }
        }

        if too_many {
            Self::Direct {
                bits: strategy.global_bits,
                storage: SimpleBitStorage::from_values(strategy.global_bits, values),
            }
        } else if palette.len() <= 1 {
            Self::Single {
                value: palette.first().copied().unwrap_or(0),
                entry_count: strategy.entry_count,
            }
        } else {
            let raw_bits = ceillog2(palette.len() as i32) as u8;
            let bits = raw_bits.max(strategy.min_indirect_bits);
            Self::Indirect {
                bits,
                palette,
                storage: SimpleBitStorage::from_values(bits, &indices),
            }
        }
    }

    pub fn from_disk_palette(
        palette: Vec<u32>,
        data_longs: Option<&[i64]>,
        strategy: PaletteStrategy,
    ) -> Result<Self, String> {
        if palette.is_empty() {
            return Err("Empty palette in disk section".into());
        }
        if palette.len() == 1 {
            return Ok(Self::Single {
                value: palette[0],
                entry_count: strategy.entry_count,
            });
        }
        let raw_bits = ceillog2(palette.len() as i32) as u8;
        let disk_bits = raw_bits.max(strategy.min_indirect_bits);
        let longs = data_longs.ok_or("Missing `data` long array for multi-entry palette")?;
        let raw_u64: Vec<u64> = longs.iter().map(|&x| x as u64).collect();
        let disk_storage = SimpleBitStorage::from_raw(disk_bits, strategy.entry_count, raw_u64)?;

        if disk_bits <= strategy.max_indirect_bits {
            Ok(Self::Indirect {
                bits: disk_bits,
                palette,
                storage: disk_storage,
            })
        } else {
            // Re-encode into global/direct palette
            let mut global_values = Vec::with_capacity(strategy.entry_count);
            for i in 0..strategy.entry_count {
                let idx = disk_storage.get(i) as usize;
                global_values.push(palette.get(idx).copied().unwrap_or(0));
            }
            Ok(Self::Direct {
                bits: strategy.global_bits,
                storage: SimpleBitStorage::from_values(strategy.global_bits, &global_values),
            })
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> u32 {
        match self {
            Self::Single { value, .. } => *value,
            Self::Indirect { palette, storage, .. } => {
                palette[storage.get(index) as usize]
            }
            Self::Direct { storage, .. } => storage.get(index),
        }
    }

    /// Set the entry at `index` to `value`, automatically upgrading Single -> Indirect -> Direct
    /// following Minecraft's `PalettedContainer` resize rules. Returns the previous value.
    pub fn set(&mut self, index: usize, value: u32, strategy: PaletteStrategy) -> u32 {
        match self {
            Self::Single {
                value: cur,
                entry_count,
            } => {
                let old = *cur;
                if old == value {
                    return old;
                }
                let bits = strategy.min_indirect_bits;
                let mut storage = SimpleBitStorage::new(bits, *entry_count);
                storage.set(index, 1);
                *self = Self::Indirect {
                    bits,
                    palette: vec![old, value],
                    storage,
                };
                old
            }
            Self::Indirect {
                bits,
                palette,
                storage,
            } => {
                let old = palette[storage.get(index) as usize];
                if old == value {
                    return old;
                }
                if let Some(pos) = palette.iter().position(|&p| p == value) {
                    storage.set(index, pos as u32);
                    return old;
                }
                let new_idx = palette.len() as u32;
                palette.push(value);
                let max_entries = 1usize << (*bits as usize);
                if palette.len() <= max_entries {
                    storage.set(index, new_idx);
                    return old;
                }
                let needed_bits = (ceillog2(palette.len() as i32) as u8).max(strategy.min_indirect_bits);
                if needed_bits <= strategy.max_indirect_bits {
                    let count = storage.size();
                    let mut new_storage = SimpleBitStorage::new(needed_bits, count);
                    for i in 0..count {
                        new_storage.set(i, storage.get(i));
                    }
                    new_storage.set(index, new_idx);
                    *bits = needed_bits;
                    *storage = new_storage;
                    old
                } else {
                    let count = storage.size();
                    let mut global_values = Vec::with_capacity(count);
                    for i in 0..count {
                        global_values.push(palette[storage.get(i) as usize]);
                    }
                    global_values[index] = value;
                    *self = Self::Direct {
                        bits: strategy.global_bits,
                        storage: SimpleBitStorage::from_values(strategy.global_bits, &global_values),
                    };
                    old
                }
            }
            Self::Direct { storage, .. } => {
                let old = storage.get(index);
                if old != value {
                    storage.set(index, value);
                }
                old
            }
        }
    }

    /// Compact and serialize into Minecraft Anvil disk format (`palette`, optional `data` long[]).
    pub fn to_disk_palette(&self, strategy: PaletteStrategy) -> (Vec<u32>, Option<Vec<i64>>) {
        match self {
            Self::Single { value, .. } => (vec![*value], None),
            Self::Indirect { .. } | Self::Direct { .. } => {
                let count = self.entry_count();
                let mut palette: Vec<u32> = Vec::new();
                let mut indices: Vec<u32> = Vec::with_capacity(count);
                for i in 0..count {
                    let v = self.get(i);
                    if let Some(pos) = palette.iter().position(|&p| p == v) {
                        indices.push(pos as u32);
                    } else {
                        let id = palette.len() as u32;
                        palette.push(v);
                        indices.push(id);
                    }
                }
                if palette.len() <= 1 {
                    (vec![palette.first().copied().unwrap_or(0)], None)
                } else {
                    let raw_bits = ceillog2(palette.len() as i32) as u8;
                    let bits = raw_bits.max(strategy.min_indirect_bits);
                    let storage = SimpleBitStorage::from_values(bits, &indices);
                    let longs: Vec<i64> = storage.raw().iter().map(|&u| u as i64).collect();
                    (palette, Some(longs))
                }
            }
        }
    }

    #[inline]
    pub fn entry_count(&self) -> usize {
        match self {
            Self::Single { entry_count, .. } => *entry_count,
            Self::Indirect { storage, .. } | Self::Direct { storage, .. } => storage.size(),
        }
    }

    /// Write using Minecraft 26.3's `PalettedContainer.Data.write(FriendlyByteBuf)` wire format:
    /// `u8 bits` + palette + raw fixed-size `long[]` (NO VarInt length prefix on `long[]`!).
    pub fn write_network(&self, out: &mut Vec<u8>) {
        match self {
            Self::Single { value, .. } => {
                out.push(0);
                write_varint(out, *value as i32);
                // ZeroBitStorage raw() has length 0 -> 0 bytes written
            }
            Self::Indirect {
                bits,
                palette,
                storage,
            } => {
                out.push(*bits);
                write_varint(out, palette.len() as i32);
                for &v in palette {
                    write_varint(out, v as i32);
                }
                for &word in storage.raw() {
                    out.extend_from_slice(&word.to_be_bytes());
                }
            }
            Self::Direct { bits, storage } => {
                out.push(*bits);
                for &word in storage.raw() {
                    out.extend_from_slice(&word.to_be_bytes());
                }
            }
        }
    }
}

#[inline]
fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if (v & !0x7F) == 0 {
            out.push(v as u8);
            return;
        }
        out.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
}
