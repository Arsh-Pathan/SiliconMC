pub mod mutf8;

use indexmap::IndexMap;
use std::io::{Cursor, Read, Write};
use thiserror::Error;

pub const MAX_DEPTH: usize = 512;

#[derive(Debug, Error)]
pub enum NbtError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unknown NBT tag id: {0}")]
    UnknownTagId(u8),
    #[error("Root NBT tag must be Compound (10), got {0}")]
    RootNotCompound(u8),
    #[error("NBT nesting depth exceeded {MAX_DEPTH}")]
    MaxDepthExceeded,
    #[error("Negative list/array length: {0}")]
    NegativeLength(i32),
    #[error("Invalid Java Modified UTF-8 string")]
    InvalidMutf8,
    #[error("Modified UTF-8 string too long ({0} bytes > 65535)")]
    StringTooLong(usize),
    #[error("Heterogeneous NBT list elements are not valid in binary NBT")]
    HeterogeneousList,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtTag>),
    Compound(NbtCompound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NbtCompound {
    entries: IndexMap<String, NbtTag>,
}

impl NbtCompound {
    pub fn new() -> Self {
        Self {
            entries: IndexMap::new(),
        }
    }

    pub fn insert(&mut self, key: impl Into<String>, value: NbtTag) -> Option<NbtTag> {
        self.entries.insert(key.into(), value)
    }

    pub fn get(&self, key: &str) -> Option<&NbtTag> {
        self.entries.get(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &NbtTag)> {
        self.entries.iter()
    }

    pub fn get_byte(&self, key: &str) -> Option<i8> {
        match self.get(key) {
            Some(NbtTag::Byte(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_short(&self, key: &str) -> Option<i16> {
        match self.get(key) {
            Some(NbtTag::Short(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i32> {
        match self.get(key) {
            Some(NbtTag::Int(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_long(&self, key: &str) -> Option<i64> {
        match self.get(key) {
            Some(NbtTag::Long(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_float(&self, key: &str) -> Option<f32> {
        match self.get(key) {
            Some(NbtTag::Float(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_double(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(NbtTag::Double(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(NbtTag::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_compound(&self, key: &str) -> Option<&NbtCompound> {
        match self.get(key) {
            Some(NbtTag::Compound(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_list(&self, key: &str) -> Option<&[NbtTag]> {
        match self.get(key) {
            Some(NbtTag::List(l)) => Some(l.as_slice()),
            _ => None,
        }
    }

    pub fn get_byte_array(&self, key: &str) -> Option<&[i8]> {
        match self.get(key) {
            Some(NbtTag::ByteArray(a)) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn get_int_array(&self, key: &str) -> Option<&[i32]> {
        match self.get(key) {
            Some(NbtTag::IntArray(a)) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn get_long_array(&self, key: &str) -> Option<&[i64]> {
        match self.get(key) {
            Some(NbtTag::LongArray(a)) => Some(a.as_slice()),
            _ => None,
        }
    }
}

impl NbtTag {
    pub const fn tag_id(&self) -> u8 {
        match self {
            NbtTag::End => 0,
            NbtTag::Byte(_) => 1,
            NbtTag::Short(_) => 2,
            NbtTag::Int(_) => 3,
            NbtTag::Long(_) => 4,
            NbtTag::Float(_) => 5,
            NbtTag::Double(_) => 6,
            NbtTag::ByteArray(_) => 7,
            NbtTag::String(_) => 8,
            NbtTag::List(_) => 9,
            NbtTag::Compound(_) => 10,
            NbtTag::IntArray(_) => 11,
            NbtTag::LongArray(_) => 12,
        }
    }

    pub fn as_compound(&self) -> Option<&NbtCompound> {
        match self {
            NbtTag::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// Convert a `serde_json::Value` into an `NbtTag` (for vanilla JSON datapack -> network NBT sync).
    pub fn from_json(v: &serde_json::Value) -> NbtTag {
        match v {
            serde_json::Value::Null => NbtTag::Compound(NbtCompound::new()),
            serde_json::Value::Bool(b) => NbtTag::Byte(if *b { 1 } else { 0 }),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                        NbtTag::Int(i as i32)
                    } else {
                        NbtTag::Long(i)
                    }
                } else {
                    NbtTag::Double(n.as_f64().unwrap_or(0.0))
                }
            }
            serde_json::Value::String(s) => NbtTag::String(s.clone()),
            serde_json::Value::Array(arr) => {
                let items: Vec<NbtTag> = arr.iter().map(Self::from_json).collect();
                // Minecraft's ListTag wraps heterogeneous lists in {"": elem} compounds (1.21.5+ / 26.x)
                if items.len() > 1 {
                    let first_id = items[0].tag_id();
                    if items.iter().any(|t| t.tag_id() != first_id) {
                        let wrapped = items
                            .into_iter()
                            .map(|item| {
                                let mut c = NbtCompound::new();
                                c.insert("", item);
                                NbtTag::Compound(c)
                            })
                            .collect();
                        return NbtTag::List(wrapped);
                    }
                }
                NbtTag::List(items)
            }
            serde_json::Value::Object(map) => {
                let mut c = NbtCompound::new();
                for (k, val) in map {
                    c.insert(k.clone(), Self::from_json(val));
                }
                NbtTag::Compound(c)
            }
        }
    }
}

fn read_u8<R: Read>(r: &mut R) -> Result<u8, NbtError> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_i16<R: Read>(r: &mut R) -> Result<i16, NbtError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(i16::from_be_bytes(b))
}

fn read_i32<R: Read>(r: &mut R) -> Result<i32, NbtError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

fn read_i64<R: Read>(r: &mut R) -> Result<i64, NbtError> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_be_bytes(b))
}

fn read_payload<R: Read>(r: &mut R, tag_id: u8, depth: usize) -> Result<NbtTag, NbtError> {
    if depth > MAX_DEPTH {
        return Err(NbtError::MaxDepthExceeded);
    }
    match tag_id {
        0 => Ok(NbtTag::End),
        1 => Ok(NbtTag::Byte(read_u8(r)? as i8)),
        2 => Ok(NbtTag::Short(read_i16(r)?)),
        3 => Ok(NbtTag::Int(read_i32(r)?)),
        4 => Ok(NbtTag::Long(read_i64(r)?)),
        5 => Ok(NbtTag::Float(f32::from_bits(read_i32(r)? as u32))),
        6 => Ok(NbtTag::Double(f64::from_bits(read_i64(r)? as u64))),
        7 => {
            let len = read_i32(r)?;
            if len < 0 {
                return Err(NbtError::NegativeLength(len));
            }
            let mut raw = vec![0u8; len as usize];
            r.read_exact(&mut raw)?;
            Ok(NbtTag::ByteArray(raw.into_iter().map(|b| b as i8).collect()))
        }
        8 => Ok(NbtTag::String(mutf8::read_mutf8(r)?)),
        9 => {
            let elem_id = read_u8(r)?;
            let len = read_i32(r)?;
            if len < 0 {
                return Err(NbtError::NegativeLength(len));
            }
            if elem_id == 0 && len > 0 {
                return Err(NbtError::UnknownTagId(0));
            }
            let mut list = Vec::with_capacity(len as usize);
            for _ in 0..len {
                list.push(read_payload(r, elem_id, depth + 1)?);
            }
            Ok(NbtTag::List(list))
        }
        10 => {
            let mut compound = NbtCompound::new();
            loop {
                let child_id = read_u8(r)?;
                if child_id == 0 {
                    break;
                }
                let name = mutf8::read_mutf8(r)?;
                let value = read_payload(r, child_id, depth + 1)?;
                compound.insert(name, value);
            }
            Ok(NbtTag::Compound(compound))
        }
        11 => {
            let len = read_i32(r)?;
            if len < 0 {
                return Err(NbtError::NegativeLength(len));
            }
            let mut out = Vec::with_capacity(len as usize);
            for _ in 0..len {
                out.push(read_i32(r)?);
            }
            Ok(NbtTag::IntArray(out))
        }
        12 => {
            let len = read_i32(r)?;
            if len < 0 {
                return Err(NbtError::NegativeLength(len));
            }
            let mut out = Vec::with_capacity(len as usize);
            for _ in 0..len {
                out.push(read_i64(r)?);
            }
            Ok(NbtTag::LongArray(out))
        }
        other => Err(NbtError::UnknownTagId(other)),
    }
}

fn write_payload<W: Write>(w: &mut W, tag: &NbtTag, depth: usize) -> Result<(), NbtError> {
    if depth > MAX_DEPTH {
        return Err(NbtError::MaxDepthExceeded);
    }
    match tag {
        NbtTag::End => Ok(()),
        NbtTag::Byte(v) => Ok(w.write_all(&[*v as u8])?),
        NbtTag::Short(v) => Ok(w.write_all(&v.to_be_bytes())?),
        NbtTag::Int(v) => Ok(w.write_all(&v.to_be_bytes())?),
        NbtTag::Long(v) => Ok(w.write_all(&v.to_be_bytes())?),
        NbtTag::Float(v) => Ok(w.write_all(&v.to_bits().to_be_bytes())?),
        NbtTag::Double(v) => Ok(w.write_all(&v.to_bits().to_be_bytes())?),
        NbtTag::ByteArray(arr) => {
            w.write_all(&(arr.len() as i32).to_be_bytes())?;
            let bytes: Vec<u8> = arr.iter().map(|&b| b as u8).collect();
            w.write_all(&bytes)?;
            Ok(())
        }
        NbtTag::String(s) => mutf8::write_mutf8(w, s),
        NbtTag::List(items) => {
            let elem_id = items.first().map(NbtTag::tag_id).unwrap_or(0);
            if items.iter().any(|t| t.tag_id() != elem_id) {
                return Err(NbtError::HeterogeneousList);
            }
            w.write_all(&[elem_id])?;
            w.write_all(&(items.len() as i32).to_be_bytes())?;
            for item in items {
                write_payload(w, item, depth + 1)?;
            }
            Ok(())
        }
        NbtTag::Compound(c) => {
            for (k, v) in c.iter() {
                w.write_all(&[v.tag_id()])?;
                mutf8::write_mutf8(w, k)?;
                write_payload(w, v, depth + 1)?;
            }
            w.write_all(&[0])?;
            Ok(())
        }
        NbtTag::IntArray(arr) => {
            w.write_all(&(arr.len() as i32).to_be_bytes())?;
            for v in arr {
                w.write_all(&v.to_be_bytes())?;
            }
            Ok(())
        }
        NbtTag::LongArray(arr) => {
            w.write_all(&(arr.len() as i32).to_be_bytes())?;
            for v in arr {
                w.write_all(&v.to_be_bytes())?;
            }
            Ok(())
        }
    }
}

/// Read standard file/disk NBT (`NbtIo.read`): root tag id (10) + root name + compound payload.
pub fn read_file<R: Read>(r: &mut R) -> Result<(String, NbtCompound), NbtError> {
    let root_id = read_u8(r)?;
    if root_id != 10 {
        return Err(NbtError::RootNotCompound(root_id));
    }
    let root_name = mutf8::read_mutf8(r)?;
    match read_payload(r, 10, 0)? {
        NbtTag::Compound(c) => Ok((root_name, c)),
        _ => unreachable!(),
    }
}

/// Write standard file/disk NBT (`NbtIo.write`): root tag id (10) + root name + compound payload.
pub fn write_file<W: Write>(w: &mut W, root_name: &str, compound: &NbtCompound) -> Result<(), NbtError> {
    w.write_all(&[10])?;
    mutf8::write_mutf8(w, root_name)?;
    write_payload(w, &NbtTag::Compound(compound.clone()), 0)
}

pub fn read_file_bytes(bytes: &[u8]) -> Result<(String, NbtCompound), NbtError> {
    let mut cur = Cursor::new(bytes);
    read_file(&mut cur)
}

pub fn write_file_bytes(root_name: &str, compound: &NbtCompound) -> Result<Vec<u8>, NbtError> {
    let mut out = Vec::new();
    write_file(&mut out, root_name, compound)?;
    Ok(out)
}

/// Read network rootless NBT (`NbtIo.readAnyTag` — used in Protocol 777).
pub fn read_network<R: Read>(r: &mut R) -> Result<NbtTag, NbtError> {
    let tag_id = read_u8(r)?;
    if tag_id == 0 {
        return Ok(NbtTag::End);
    }
    read_payload(r, tag_id, 0)
}

/// Write network rootless NBT (`NbtIo.writeAnyTag` — used in Protocol 777).
pub fn write_network<W: Write>(w: &mut W, tag: &NbtTag) -> Result<(), NbtError> {
    w.write_all(&[tag.tag_id()])?;
    if tag.tag_id() != 0 {
        write_payload(w, tag, 0)?;
    }
    Ok(())
}

pub fn write_network_bytes(tag: &NbtTag) -> Result<Vec<u8>, NbtError> {
    let mut out = Vec::new();
    write_network(&mut out, tag)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use mc_oracle::with_oracle;
    use serde_json::json;

    fn sample_compound() -> NbtCompound {
        let mut root = NbtCompound::new();
        root.insert("byte_val", NbtTag::Byte(-42));
        root.insert("short_val", NbtTag::Short(32000));
        root.insert("int_val", NbtTag::Int(-1_234_567_890));
        root.insert("long_val", NbtTag::Long(i64::MIN + 7));
        root.insert("float_val", NbtTag::Float(3.5_f32));
        root.insert("double_val", NbtTag::Double(-12345.6789));
        root.insert("byte_arr", NbtTag::ByteArray(vec![0, 1, -1, 127, -128]));
        root.insert("int_arr", NbtTag::IntArray(vec![10, -20, 300_000]));
        root.insert("long_arr", NbtTag::LongArray(vec![0, i64::MAX, i64::MIN]));
        // Includes ASCII, NUL byte (MUTF-8 0xC0 0x80), CJK, and emoji (surrogate pair in MUTF-8!)
        root.insert("str_utf8", NbtTag::String("hello\0世界🚀".to_string()));
        root.insert(
            "empty_list",
            NbtTag::List(vec![]),
        );
        root.insert(
            "str_list",
            NbtTag::List(vec![
                NbtTag::String("alpha".into()),
                NbtTag::String("beta".into()),
            ]),
        );
        let mut nested = NbtCompound::new();
        nested.insert("flag", NbtTag::Byte(1));
        nested.insert("name", NbtTag::String("siliconmc".into()));
        root.insert("nested", NbtTag::Compound(nested));
        root
    }

    #[test]
    fn diff_nbt_file_and_network_roundtrip_against_java() {
        let compound = sample_compound();

        // 1) Disk/file format
        let file_bytes = write_file_bytes("", &compound).unwrap();
        let (java_file_bytes, _snbt) = with_oracle(|o| o.nbt_roundtrip("file", &file_bytes)).unwrap();
        // Java CompoundTag uses HashMap so key ordering can differ; re-decode Java's bytes and compare AST!
        let (name, decoded_from_java) = read_file_bytes(&java_file_bytes).unwrap();
        assert_eq!(name, "");
        assert_eq!(decoded_from_java.len(), compound.len());
        for (k, v) in compound.iter() {
            assert_eq!(decoded_from_java.get(k), Some(v), "File NBT mismatch on key {k}");
        }

        // 2) Network rootless format (Protocol 777)
        let net_bytes = write_network_bytes(&NbtTag::Compound(compound.clone())).unwrap();
        let (java_net_bytes, _) = with_oracle(|o| o.nbt_roundtrip("network", &net_bytes)).unwrap();
        let mut cur = Cursor::new(&java_net_bytes);
        let decoded_net = read_network(&mut cur).unwrap();
        assert_eq!(cur.position() as usize, java_net_bytes.len());
        let NbtTag::Compound(dc) = decoded_net else {
            panic!("Expected compound");
        };
        for (k, v) in compound.iter() {
            assert_eq!(dc.get(k), Some(v), "Network NBT mismatch on key {k}");
        }

        // 3) Java-generated NBT from SNBT decoded by Rust
        let resp = with_oracle(|o| {
            o.call(json!({
                "op": "nbt_from_snbt",
                "snbt": "{text:\"hi\",bold:1b,extra:[{text:\"world\"}],ids:[L;1L,-2L,999999999999L]}",
                "mode": "network"
            }))
        })
        .unwrap();
        let bytes = B64.decode(resp["bytes_b64"].as_str().unwrap()).unwrap();
        let mut cur = Cursor::new(&bytes);
        let tag = read_network(&mut cur).unwrap();
        let c = tag.as_compound().unwrap();
        assert_eq!(c.get_string("text"), Some("hi"));
        assert_eq!(c.get_byte("bold"), Some(1));
        assert_eq!(c.get_long_array("ids"), Some(&[1, -2, 999_999_999_999][..]));
    }
}
