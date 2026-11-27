use mc_math::{BlockPos, Vec3};
use mc_nbt::{NbtError, NbtTag};
use std::io::Cursor;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("Unexpected EOF while reading packet")]
    Eof,
    #[error("VarInt exceeds 5 bytes")]
    VarIntTooLong,
    #[error("VarLong exceeds 10 bytes")]
    VarLongTooLong,
    #[error("Invalid UTF-8 string")]
    InvalidUtf8,
    #[error("String/array length {0} exceeds maximum {1}")]
    LengthExceeded(usize, usize),
    #[error("Negative length: {0}")]
    NegativeLength(i32),
    #[error("Unknown packet id {0} in phase {1}")]
    UnknownPacketId(i32, &'static str),
    #[error("Invalid enum/intent value: {0}")]
    InvalidValue(i32),
    #[error("NBT error: {0}")]
    Nbt(#[from] NbtError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Compression error: {0}")]
    Compression(String),
}

/// Zero-copy slice reader matching Minecraft's `FriendlyByteBuf`.
pub struct PacketReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PacketReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ProtoError> {
        if self.pos + len > self.data.len() {
            return Err(ProtoError::Eof);
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_i8(&mut self) -> Result<i8, ProtoError> {
        Ok(self.read_u8()? as i8)
    }

    pub fn read_bool(&mut self) -> Result<bool, ProtoError> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtoError> {
        Ok(u16::from_be_bytes(self.read_bytes(2)?.try_into().unwrap()))
    }

    pub fn read_i16(&mut self) -> Result<i16, ProtoError> {
        Ok(i16::from_be_bytes(self.read_bytes(2)?.try_into().unwrap()))
    }

    pub fn read_i32(&mut self) -> Result<i32, ProtoError> {
        Ok(i32::from_be_bytes(self.read_bytes(4)?.try_into().unwrap()))
    }

    pub fn read_i64(&mut self) -> Result<i64, ProtoError> {
        Ok(i64::from_be_bytes(self.read_bytes(8)?.try_into().unwrap()))
    }

    pub fn read_f32(&mut self) -> Result<f32, ProtoError> {
        Ok(f32::from_bits(u32::from_be_bytes(
            self.read_bytes(4)?.try_into().unwrap(),
        )))
    }

    pub fn read_f64(&mut self) -> Result<f64, ProtoError> {
        Ok(f64::from_bits(u64::from_be_bytes(
            self.read_bytes(8)?.try_into().unwrap(),
        )))
    }

    pub fn read_varint(&mut self) -> Result<i32, ProtoError> {
        let mut value: u32 = 0;
        let mut shift = 0;
        loop {
            let b = self.read_u8()?;
            value |= ((b & 0x7F) as u32) << shift;
            if (b & 0x80) == 0 {
                return Ok(value as i32);
            }
            shift += 7;
            if shift >= 35 {
                return Err(ProtoError::VarIntTooLong);
            }
        }
    }

    pub fn read_varlong(&mut self) -> Result<i64, ProtoError> {
        let mut value: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.read_u8()?;
            value |= ((b & 0x7F) as u64) << shift;
            if (b & 0x80) == 0 {
                return Ok(value as i64);
            }
            shift += 7;
            if shift >= 70 {
                return Err(ProtoError::VarLongTooLong);
            }
        }
    }

    pub fn read_string(&mut self, max_utf16_len: usize) -> Result<String, ProtoError> {
        let byte_len = self.read_varint()?;
        if byte_len < 0 {
            return Err(ProtoError::NegativeLength(byte_len));
        }
        let max_bytes = max_utf16_len.saturating_mul(3);
        if (byte_len as usize) > max_bytes {
            return Err(ProtoError::LengthExceeded(byte_len as usize, max_bytes));
        }
        let raw = self.read_bytes(byte_len as usize)?;
        let s = std::str::from_utf8(raw).map_err(|_| ProtoError::InvalidUtf8)?;
        Ok(s.to_string())
    }

    pub fn read_byte_array(&mut self, max_len: usize) -> Result<Vec<u8>, ProtoError> {
        let len = self.read_varint()?;
        if len < 0 {
            return Err(ProtoError::NegativeLength(len));
        }
        if (len as usize) > max_len {
            return Err(ProtoError::LengthExceeded(len as usize, max_len));
        }
        Ok(self.read_bytes(len as usize)?.to_vec())
    }

    pub fn read_uuid(&mut self) -> Result<Uuid, ProtoError> {
        let raw: [u8; 16] = self.read_bytes(16)?.try_into().unwrap();
        Ok(Uuid::from_bytes(raw))
    }

    pub fn read_block_pos(&mut self) -> Result<BlockPos, ProtoError> {
        Ok(BlockPos::from_long(self.read_i64()?))
    }

    pub fn read_vec3(&mut self) -> Result<Vec3, ProtoError> {
        Ok(Vec3::new(self.read_f64()?, self.read_f64()?, self.read_f64()?))
    }

    /// Exact match for Minecraft 26.3's `net.minecraft.network.LpVec3.read(ByteBuf)`.
    pub fn read_lp_vec3(&mut self) -> Result<Vec3, ProtoError> {
        let lowest = self.read_u8()? as u64;
        if lowest == 0 {
            return Ok(Vec3::ZERO);
        }
        let middle = self.read_u8()? as u64;
        let highest = u32::from_be_bytes(self.read_bytes(4)?.try_into().unwrap()) as u64;
        let buffer = (highest << 16) | (middle << 8) | lowest;
        let mut scale = lowest & 3;
        if (lowest & 4) == 4 {
            scale |= ((self.read_varint()? as u32) as u64) << 2;
        }
        let unpack = |v: u64| -> f64 { (((v & 32767).min(32766)) as f64) * 2.0 / 32766.0 - 1.0 };
        let s = scale as f64;
        Ok(Vec3::new(
            unpack(buffer >> 3) * s,
            unpack(buffer >> 18) * s,
            unpack(buffer >> 33) * s,
        ))
    }

    pub fn read_nbt_network(&mut self) -> Result<NbtTag, ProtoError> {
        let mut cur = Cursor::new(&self.data[self.pos..]);
        let tag = mc_nbt::read_network(&mut cur)?;
        self.pos += cur.position() as usize;
        Ok(tag)
    }
}

/// Owned byte writer matching Minecraft's `FriendlyByteBuf`.
#[derive(Debug, Default, Clone)]
pub struct PacketWriter {
    buf: Vec<u8>,
}

impl PacketWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_i8(&mut self, v: i8) {
        self.buf.push(v as u8);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.buf.push(if v { 1 } else { 0 });
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
    }

    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_varint(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            if (v & !0x7F) == 0 {
                self.buf.push(v as u8);
                return;
            }
            self.buf.push(((v & 0x7F) | 0x80) as u8);
            v >>= 7;
        }
    }

    pub fn write_optional_varint(&mut self, value: Option<i32>) {
        match value {
            None => self.write_varint(0),
            Some(v) => self.write_varint(v + 1),
        }
    }

    pub fn write_varlong(&mut self, value: i64) {
        let mut v = value as u64;
        loop {
            if (v & !0x7F) == 0 {
                self.buf.push(v as u8);
                return;
            }
            self.buf.push(((v & 0x7F) | 0x80) as u8);
            v >>= 7;
        }
    }

    pub fn write_string(&mut self, s: &str) {
        self.write_varint(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn write_byte_array(&mut self, bytes: &[u8]) {
        self.write_varint(bytes.len() as i32);
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_long_array(&mut self, longs: &[i64]) {
        self.write_varint(longs.len() as i32);
        for &v in longs {
            self.write_i64(v);
        }
    }

    /// Exact match for Minecraft 26.3 `ByteBufCodecs.BIT_SET` (`FriendlyByteBuf.writeByteArray(value.toByteArray())`).
    pub fn write_bitset(&mut self, words: &[u64]) {
        let mut bytes = Vec::with_capacity(words.len() * 8);
        for &w in words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        self.write_byte_array(&bytes);
    }

    pub fn write_uuid(&mut self, uuid: Uuid) {
        self.buf.extend_from_slice(uuid.as_bytes());
    }

    pub fn write_block_pos(&mut self, pos: BlockPos) {
        self.write_i64(pos.as_long());
    }

    pub fn write_vec3(&mut self, v: Vec3) {
        self.write_f64(v.x);
        self.write_f64(v.y);
        self.write_f64(v.z);
    }

    /// Exact match for Minecraft 26.3's `net.minecraft.network.LpVec3.write(ByteBuf, Vec3)`.
    pub fn write_lp_vec3(&mut self, v: Vec3) {
        const ABS_MAX: f64 = 1.7179869183e10;
        const ABS_MIN: f64 = 3.051944088384301e-5;
        let sanitize = |val: f64| -> f64 {
            if val.is_nan() {
                0.0
            } else {
                val.clamp(-ABS_MAX, ABS_MAX)
            }
        };
        let x = sanitize(v.x);
        let y = sanitize(v.y);
        let z = sanitize(v.z);
        let len = x.abs().max(y.abs().max(z.abs()));
        if len < ABS_MIN {
            self.write_u8(0);
            return;
        }
        let scale = len.ceil() as i64;
        let is_partial = (scale & 3) != scale;
        let markers = if is_partial { (scale & 3) | 4 } else { scale };
        let pack = |val: f64| -> i64 { ((val * 0.5 + 0.5) * 32766.0).round() as i64 };
        let xn = pack(x / scale as f64) << 3;
        let yn = pack(y / scale as f64) << 18;
        let zn = pack(z / scale as f64) << 33;
        let buffer = markers | xn | yn | zn;
        self.write_u8(buffer as u8);
        self.write_u8((buffer >> 8) as u8);
        self.write_i32((buffer >> 16) as i32);
        if is_partial {
            self.write_varint((scale >> 2) as i32);
        }
    }

    pub fn write_nbt_network(&mut self, tag: &NbtTag) -> Result<(), ProtoError> {
        mc_nbt::write_network(&mut self.buf, tag)?;
        Ok(())
    }
}

pub fn varint_size(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    while (v & !0x7F) != 0 {
        v >>= 7;
        n += 1;
    }
    n
}
