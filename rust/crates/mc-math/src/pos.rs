use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const ZERO: Self = Self { x: 0, y: 0, z: 0 };
    pub const PACKED_HORIZONTAL_LENGTH: u32 = 26;
    pub const PACKED_Y_LENGTH: u32 = 12;
    const PACKED_X_MASK: i64 = (1_i64 << Self::PACKED_HORIZONTAL_LENGTH) - 1;
    const PACKED_Y_MASK: i64 = (1_i64 << Self::PACKED_Y_LENGTH) - 1;
    const PACKED_Z_MASK: i64 = (1_i64 << Self::PACKED_HORIZONTAL_LENGTH) - 1;
    const Z_OFFSET: u32 = Self::PACKED_Y_LENGTH;
    const X_OFFSET: u32 = Self::PACKED_Y_LENGTH + Self::PACKED_HORIZONTAL_LENGTH;

    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn as_long(self) -> i64 {
        (((self.x as i64) & Self::PACKED_X_MASK) << Self::X_OFFSET)
            | ((self.y as i64) & Self::PACKED_Y_MASK)
            | (((self.z as i64) & Self::PACKED_Z_MASK) << Self::Z_OFFSET)
    }

    #[inline]
    pub const fn from_long(packed: i64) -> Self {
        let x = (packed >> 38) as i32;
        let y = ((packed << 52) >> 52) as i32;
        let z = ((packed << 26) >> 38) as i32;
        Self { x, y, z }
    }

    #[inline]
    pub fn containing(x: f64, y: f64, z: f64) -> Self {
        Self {
            x: x.floor() as i32,
            y: y.floor() as i32,
            z: z.floor() as i32,
        }
    }

    #[inline]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    #[inline]
    pub const fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.x >> 4, self.z >> 4)
    }

    #[inline]
    pub const fn section_pos(self) -> SectionPos {
        SectionPos::new(self.x >> 4, self.y >> 4, self.z >> 4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const ZERO: Self = Self { x: 0, z: 0 };

    #[inline]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    #[inline]
    pub const fn as_long(self) -> i64 {
        ((self.x as u32 as u64) | ((self.z as u32 as u64) << 32)) as i64
    }

    #[inline]
    pub const fn from_long(packed: i64) -> Self {
        Self {
            x: (packed as u64 & 0xFFFF_FFFF) as u32 as i32,
            z: ((packed as u64 >> 32) & 0xFFFF_FFFF) as u32 as i32,
        }
    }

    #[inline]
    pub const fn region_x(self) -> i32 {
        self.x >> 5
    }

    #[inline]
    pub const fn region_z(self) -> i32 {
        self.z >> 5
    }

    #[inline]
    pub const fn region_local_x(self) -> usize {
        (self.x & 31) as usize
    }

    #[inline]
    pub const fn region_local_z(self) -> usize {
        (self.z & 31) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn as_long(self) -> i64 {
        (((self.x as i64) & 0x3F_FFFF) << 42)
            | (((self.z as i64) & 0x3F_FFFF) << 20)
            | ((self.y as i64) & 0xF_FFFF)
    }

    #[inline]
    pub const fn from_long(packed: i64) -> Self {
        Self {
            x: (packed >> 42) as i32,
            y: ((packed << 44) >> 44) as i32,
            z: ((packed << 22) >> 42) as i32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    #[inline]
    pub fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    #[inline]
    pub fn distance_to_sqr(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Aabb {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl Aabb {
    #[inline]
    pub fn new(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Self {
        Self {
            min_x: x0.min(x1),
            min_y: y0.min(y1),
            min_z: z0.min(z1),
            max_x: x0.max(x1),
            max_y: y0.max(y1),
            max_z: z0.max(z1),
        }
    }

    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min_x < other.max_x
            && self.max_x > other.min_x
            && self.min_y < other.max_y
            && self.max_y > other.min_y
            && self.min_z < other.max_z
            && self.max_z > other.min_z
    }
}
