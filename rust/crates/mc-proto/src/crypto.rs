use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use sha1::{Digest, Sha1};

/// Exact AES-128-CFB8 stream cipher matching Minecraft's `Crypt.getCipher` (key == initial IV).
#[derive(Clone)]
pub struct AesCfb8 {
    cipher: Aes128,
    shift_reg: [u8; 16],
}

impl AesCfb8 {
    pub fn new(key: [u8; 16]) -> Self {
        let cipher = Aes128::new(GenericArray::from_slice(&key));
        Self {
            cipher,
            shift_reg: key,
        }
    }

    pub fn encrypt_in_place(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let mut block = GenericArray::clone_from_slice(&self.shift_reg);
            self.cipher.encrypt_block(&mut block);
            let c = *byte ^ block[0];
            *byte = c;
            self.shift_reg.copy_within(1..16, 0);
            self.shift_reg[15] = c;
        }
    }

    pub fn decrypt_in_place(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let mut block = GenericArray::clone_from_slice(&self.shift_reg);
            self.cipher.encrypt_block(&mut block);
            let c = *byte;
            *byte = c ^ block[0];
            self.shift_reg.copy_within(1..16, 0);
            self.shift_reg[15] = c;
        }
    }
}

/// Exact match for Mojang's `new BigInteger(Crypt.digestData(serverId, publicKey, secretKey)).toString(16)`.
pub fn minecraft_server_hash(server_id: &str, shared_secret: &[u8], public_key_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(shared_secret);
    hasher.update(public_key_der);
    let mut digest: [u8; 20] = hasher.finalize().into();

    let negative = (digest[0] & 0x80) != 0;
    if negative {
        // Two's complement negate
        let mut carry = true;
        for b in digest.iter_mut().rev() {
            let inv = !*b;
            let (res, c) = inv.overflowing_add(u8::from(carry));
            *b = res;
            carry = c;
        }
    }

    let hex_full: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = hex_full.trim_start_matches('0');
    let body = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{body}")
    } else {
        body.to_string()
    }
}

/// Offline-mode player UUID matching `UUIDUtil.createOfflinePlayerUUID(name)`.
pub fn offline_player_uuid(username: &str) -> uuid::Uuid {
    let input = format!("OfflinePlayer:{username}");
    // Java `UUID.nameUUIDFromBytes` uses MD5 (UUID v3) without a namespace prefix
    use md5_v3::md5_uuid;
    md5_uuid(input.as_bytes())
}

mod md5_v3 {
    use uuid::Uuid;

    pub fn md5_uuid(bytes: &[u8]) -> Uuid {
        // Exact RFC 4122 / Java UUID.nameUUIDFromBytes using standard MD5
        let mut d = md5_digest(bytes);
        d[6] = (d[6] & 0x0F) | 0x30; // version 3
        d[8] = (d[8] & 0x3F) | 0x80; // variant IETF
        Uuid::from_bytes(d)
    }

    // Minimal self-contained MD5 (avoids extra dep in mc-proto)
    fn md5_digest(input: &[u8]) -> [u8; 16] {
        const S: [u32; 64] = [
            7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
            5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
            6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
        ];
        const K: [u32; 64] = [
            0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
            0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
            0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
            0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
            0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
            0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
            0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
            0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
            0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
            0xeb86d391,
        ];
        let mut msg = input.to_vec();
        let bit_len = (input.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while (msg.len() % 64) != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_le_bytes());

        let (mut a0, mut b0, mut c0, mut d0) =
            (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
        for chunk in msg.chunks_exact(64) {
            let mut m = [0u32; 16];
            for i in 0..16 {
                m[i] = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
            }
            let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
            for i in 0..64 {
                let (f, g) = match i {
                    0..=15 => ((b & c) | ((!b) & d), i),
                    16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                    32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                    _ => (c ^ (b | (!d)), (7 * i) % 16),
                };
                let temp = d;
                d = c;
                c = b;
                b = b.wrapping_add(
                    a.wrapping_add(f)
                        .wrapping_add(K[i])
                        .wrapping_add(m[g])
                        .rotate_left(S[i]),
                );
                a = temp;
            }
            a0 = a0.wrapping_add(a);
            b0 = b0.wrapping_add(b);
            c0 = c0.wrapping_add(c);
            d0 = d0.wrapping_add(d);
        }
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&a0.to_le_bytes());
        out[4..8].copy_from_slice(&b0.to_le_bytes());
        out[8..12].copy_from_slice(&c0.to_le_bytes());
        out[12..16].copy_from_slice(&d0.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_cfb8_roundtrip_and_mc_hash_vectors() {
        let key = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01];
        let mut enc = AesCfb8::new(key);
        let mut dec = AesCfb8::new(key);
        let original = b"SiliconMC Protocol 777 encrypted stream test payload!".to_vec();
        let mut buf = original.clone();
        enc.encrypt_in_place(&mut buf);
        assert_ne!(buf, original);
        dec.decrypt_in_place(&mut buf);
        assert_eq!(buf, original);

        // Known official wiki.vg / Mojang test vectors
        assert_eq!(
            minecraft_server_hash("", b"Notch", b""),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );
        assert_eq!(
            minecraft_server_hash("", b"jeb_", b""),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
        assert_eq!(
            minecraft_server_hash("", b"simon", b""),
            "88e16a1019277b15d58faf0541e11910eb756f6"
        );
    }
}
