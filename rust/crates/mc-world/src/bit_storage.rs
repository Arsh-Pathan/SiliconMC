/// Exact match for Minecraft's `net.minecraft.util.SimpleBitStorage`:
/// values are packed tightly into `u64` words WITHOUT spanning across two words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleBitStorage {
    bits: u8,
    size: usize,
    values_per_long: usize,
    mask: u64,
    data: Vec<u64>,
}

impl SimpleBitStorage {
    pub fn new(bits: u8, size: usize) -> Self {
        assert!((1..=32).contains(&bits), "bits must be in 1..=32");
        let values_per_long = (64 / bits as usize).max(1);
        let long_count = size.div_ceil(values_per_long);
        let mask = (1u64 << bits) - 1;
        Self {
            bits,
            size,
            values_per_long,
            mask,
            data: vec![0u64; long_count],
        }
    }

    pub fn from_raw(bits: u8, size: usize, data: Vec<u64>) -> Result<Self, String> {
        if !(1..=32).contains(&bits) {
            return Err(format!("invalid bits: {bits}"));
        }
        let values_per_long = 64 / bits as usize;
        let expected = size.div_ceil(values_per_long);
        if data.len() != expected {
            return Err(format!(
                "Invalid long array length: expected {expected} (bits={bits}, size={size}), got {}",
                data.len()
            ));
        }
        let mask = (1u64 << bits) - 1;
        Ok(Self {
            bits,
            size,
            values_per_long,
            mask,
            data,
        })
    }

    pub fn from_values(bits: u8, values: &[u32]) -> Self {
        let mut s = Self::new(bits, values.len());
        for (i, &v) in values.iter().enumerate() {
            s.set(i, v);
        }
        s
    }

    #[inline]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn raw(&self) -> &[u64] {
        &self.data
    }

    #[inline]
    pub fn get(&self, index: usize) -> u32 {
        debug_assert!(index < self.size);
        let cell_idx = index / self.values_per_long;
        let bit_idx = (index - cell_idx * self.values_per_long) * (self.bits as usize);
        ((self.data[cell_idx] >> bit_idx) & self.mask) as u32
    }

    #[inline]
    pub fn set(&mut self, index: usize, value: u32) {
        debug_assert!(index < self.size);
        let cell_idx = index / self.values_per_long;
        let bit_idx = (index - cell_idx * self.values_per_long) * (self.bits as usize);
        let cur = self.data[cell_idx];
        self.data[cell_idx] = (cur & !(self.mask << bit_idx)) | (((value as u64) & self.mask) << bit_idx);
    }

    pub fn unpack(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.size);
        for i in 0..self.size {
            out.push(self.get(i));
        }
        out
    }
}
