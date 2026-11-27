use std::io::{Read, Write};

use crate::NbtError;

/// Encode a Rust `&str` into Java Modified UTF-8 (`DataOutput.writeUTF`).
pub fn write_mutf8<W: Write>(w: &mut W, s: &str) -> Result<(), NbtError> {
    let mut encoded = Vec::with_capacity(s.len());
    for code_unit in s.encode_utf16() {
        match code_unit {
            0x0001..=0x007F => encoded.push(code_unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                encoded.push((0xC0 | ((code_unit >> 6) & 0x1F)) as u8);
                encoded.push((0x80 | (code_unit & 0x3F)) as u8);
            }
            0x0800..=0xFFFF => {
                encoded.push((0xE0 | ((code_unit >> 12) & 0x0F)) as u8);
                encoded.push((0x80 | ((code_unit >> 6) & 0x3F)) as u8);
                encoded.push((0x80 | (code_unit & 0x3F)) as u8);
            }
        }
    }
    if encoded.len() > u16::MAX as usize {
        return Err(NbtError::StringTooLong(encoded.len()));
    }
    w.write_all(&(encoded.len() as u16).to_be_bytes())?;
    w.write_all(&encoded)?;
    Ok(())
}

/// Decode Java Modified UTF-8 (`DataInput.readUTF`) into a Rust `String`.
pub fn read_mutf8<R: Read>(r: &mut R) -> Result<String, NbtError> {
    let mut len_buf = [0u8; 2];
    r.read_exact(&mut len_buf)?;
    let byte_len = u16::from_be_bytes(len_buf) as usize;
    let mut bytes = vec![0u8; byte_len];
    r.read_exact(&mut bytes)?;

    let mut utf16 = Vec::with_capacity(byte_len);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        if b0 & 0x80 == 0 {
            utf16.push(b0 as u16);
            i += 1;
        } else if (b0 & 0xE0) == 0xC0 {
            if i + 1 >= bytes.len() {
                return Err(NbtError::InvalidMutf8);
            }
            let b1 = bytes[i + 1];
            if (b1 & 0xC0) != 0x80 {
                return Err(NbtError::InvalidMutf8);
            }
            let unit = (((b0 & 0x1F) as u16) << 6) | ((b1 & 0x3F) as u16);
            utf16.push(unit);
            i += 2;
        } else if (b0 & 0xF0) == 0xE0 {
            if i + 2 >= bytes.len() {
                return Err(NbtError::InvalidMutf8);
            }
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            if (b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 {
                return Err(NbtError::InvalidMutf8);
            }
            let unit = (((b0 & 0x0F) as u16) << 12)
                | (((b1 & 0x3F) as u16) << 6)
                | ((b2 & 0x3F) as u16);
            utf16.push(unit);
            i += 3;
        } else {
            return Err(NbtError::InvalidMutf8);
        }
    }
    String::from_utf16(&utf16).map_err(|_| NbtError::InvalidMutf8)
}
