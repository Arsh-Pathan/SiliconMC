use crate::mth::{get_seed, java_string_hash_code};
use md5::{Digest, Md5};

pub const GOLDEN_RATIO_64: i64 = -7046029254386353131_i64;
pub const SILVER_RATIO_64: i64 = 7640891576956012809_i64;

const FLOAT_UNIT: f32 = 5.9604645e-8_f32; // 2^-24
const DOUBLE_UNIT: f64 = 1.1102230246251565e-16_f64; // 2^-53 (exact power of 2 in both f32 and f64)

pub trait RandomSource {
    fn set_seed(&mut self, seed: i64);
    fn next_int(&mut self) -> i32;
    fn next_int_bounded(&mut self, bound: i32) -> i32;
    fn next_long(&mut self) -> i64;
    fn next_boolean(&mut self) -> bool;
    fn next_float(&mut self) -> f32;
    fn next_double(&mut self) -> f64;
    fn next_gaussian(&mut self) -> f64;
}

#[derive(Debug, Clone, Default)]
struct MarsagliaPolarGaussian {
    next_next_gaussian: f64,
    have_next_next_gaussian: bool,
}

impl MarsagliaPolarGaussian {
    fn reset(&mut self) {
        self.have_next_next_gaussian = false;
    }

    fn next_gaussian(&mut self, next_double: impl FnMut() -> f64) -> f64 {
        let mut next_double = next_double;
        if self.have_next_next_gaussian {
            self.have_next_next_gaussian = false;
            return self.next_next_gaussian;
        }
        let mut x;
        let mut y;
        let mut radius_squared;
        loop {
            x = 2.0 * next_double() - 1.0;
            y = 2.0 * next_double() - 1.0;
            radius_squared = x * x + y * y;
            if radius_squared < 1.0 && radius_squared != 0.0 {
                break;
            }
        }
        let multiplier = (-2.0 * radius_squared.ln() / radius_squared).sqrt();
        self.next_next_gaussian = y * multiplier;
        self.have_next_next_gaussian = true;
        x * multiplier
    }
}

/// Exact match for `net.minecraft.world.level.levelgen.LegacyRandomSource`.
#[derive(Debug, Clone)]
pub struct LegacyRandomSource {
    seed: i64,
    gaussian: MarsagliaPolarGaussian,
}

impl LegacyRandomSource {
    const MODULUS_MASK: i64 = 281474976710655_i64; // (1 << 48) - 1
    const MULTIPLIER: i64 = 25214903917_i64;
    const INCREMENT: i64 = 11_i64;

    pub fn new(seed: i64) -> Self {
        let mut rng = Self {
            seed: 0,
            gaussian: MarsagliaPolarGaussian::default(),
        };
        rng.set_seed(seed);
        rng
    }

    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        let new_seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT)
            & Self::MODULUS_MASK;
        self.seed = new_seed;
        (new_seed >> (48 - bits)) as i32
    }

    pub fn fork(&mut self) -> Self {
        Self::new(self.next_long())
    }

    pub fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        LegacyPositionalRandomFactory {
            seed: self.next_long(),
        }
    }

    #[inline]
    fn next_double_raw(&mut self) -> f64 {
        let upper = self.next(26) as i64;
        let lower = self.next(27) as i64;
        let combined = (upper << 27).wrapping_add(lower);
        (combined as f64) * DOUBLE_UNIT
    }
}

impl RandomSource for LegacyRandomSource {
    fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ Self::MULTIPLIER) & Self::MODULUS_MASK;
        self.gaussian.reset();
    }

    fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        if (bound & bound.wrapping_sub(1)) == 0 {
            return (((bound as i64).wrapping_mul(self.next(31) as i64)) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    fn next_long(&mut self) -> i64 {
        let upper = self.next(32) as i64;
        let lower = self.next(32) as i64;
        (upper << 32).wrapping_add(lower)
    }

    fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    fn next_float(&mut self) -> f32 {
        (self.next(24) as f32) * FLOAT_UNIT
    }

    fn next_double(&mut self) -> f64 {
        self.next_double_raw()
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let val = g.next_gaussian(|| self.next_double_raw());
        self.gaussian = g;
        val
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LegacyPositionalRandomFactory {
    seed: i64,
}

impl LegacyPositionalRandomFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> LegacyRandomSource {
        let positional_seed = get_seed(x, y, z);
        LegacyRandomSource::new(positional_seed ^ self.seed)
    }

    pub fn from_hash_of(&self, name: &str) -> LegacyRandomSource {
        let h = java_string_hash_code(name) as i64;
        LegacyRandomSource::new(h ^ self.seed)
    }

    pub fn from_seed(&self, seed: i64) -> LegacyRandomSource {
        LegacyRandomSource::new(seed)
    }
}

#[inline]
pub fn mix_stafford13(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(-4658895280553007687_i64 as u64);
    z = (z ^ (z >> 27)).wrapping_mul(-7723592293110705685_i64 as u64);
    z ^ (z >> 31)
}

#[inline]
pub fn upgrade_seed_to_128bit(legacy_seed: i64) -> (i64, i64) {
    let low = (legacy_seed ^ SILVER_RATIO_64) as u64;
    let high = low.wrapping_add(GOLDEN_RATIO_64 as u64);
    (mix_stafford13(low) as i64, mix_stafford13(high) as i64)
}

pub fn seed_from_hash_of(input: &str) -> (i64, i64) {
    let digest = Md5::digest(input.as_bytes());
    let lo = i64::from_be_bytes(digest[0..8].try_into().unwrap());
    let hi = i64::from_be_bytes(digest[8..16].try_into().unwrap());
    (lo, hi)
}

/// Exact match for `net.minecraft.world.level.levelgen.Xoroshiro128PlusPlus`.
#[derive(Debug, Clone, Copy)]
pub struct Xoroshiro128PlusPlus {
    seed_lo: u64,
    seed_hi: u64,
}

impl Xoroshiro128PlusPlus {
    pub fn new(seed_lo: i64, seed_hi: i64) -> Self {
        let mut lo = seed_lo as u64;
        let mut hi = seed_hi as u64;
        if (lo | hi) == 0 {
            lo = GOLDEN_RATIO_64 as u64;
            hi = SILVER_RATIO_64 as u64;
        }
        Self {
            seed_lo: lo,
            seed_hi: hi,
        }
    }

    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let s0 = self.seed_lo;
        let mut s1 = self.seed_hi;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.seed_lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.seed_hi = s1.rotate_left(28);
        result as i64
    }
}

/// Exact match for `net.minecraft.world.level.levelgen.XoroshiroRandomSource`.
#[derive(Debug, Clone)]
pub struct XoroshiroRandomSource {
    rng: Xoroshiro128PlusPlus,
    gaussian: MarsagliaPolarGaussian,
}

impl XoroshiroRandomSource {
    pub fn new(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        Self::from_128bit(lo, hi)
    }

    pub fn from_128bit(seed_lo: i64, seed_hi: i64) -> Self {
        Self {
            rng: Xoroshiro128PlusPlus::new(seed_lo, seed_hi),
            gaussian: MarsagliaPolarGaussian::default(),
        }
    }

    pub fn fork(&mut self) -> Self {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        Self::from_128bit(lo, hi)
    }

    pub fn fork_positional(&mut self) -> XoroshiroPositionalRandomFactory {
        let lo = self.rng.next_long();
        let hi = self.rng.next_long();
        XoroshiroPositionalRandomFactory {
            seed_lo: lo,
            seed_hi: hi,
        }
    }

    #[inline]
    fn next_bits(&mut self, bits: u32) -> u64 {
        (self.rng.next_long() as u64) >> (64 - bits)
    }

    #[inline]
    fn next_double_raw(&mut self) -> f64 {
        (self.next_bits(53) as f64) * DOUBLE_UNIT
    }
}

impl RandomSource for XoroshiroRandomSource {
    fn set_seed(&mut self, seed: i64) {
        let (lo, hi) = upgrade_seed_to_128bit(seed);
        self.rng = Xoroshiro128PlusPlus::new(lo, hi);
        self.gaussian.reset();
    }

    fn next_int(&mut self) -> i32 {
        self.rng.next_long() as i32
    }

    fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "Bound must be positive");
        let bound_u = bound as u32 as u64;
        let mut random_bits = (self.next_int() as u32) as u64;
        let mut multiplied = random_bits.wrapping_mul(bound_u);
        let mut fractional = multiplied & 0xFFFF_FFFF;
        if fractional < bound_u {
            let threshold = ((!(bound as u32)).wrapping_add(1) % (bound as u32)) as u64;
            while fractional < threshold {
                random_bits = (self.next_int() as u32) as u64;
                multiplied = random_bits.wrapping_mul(bound_u);
                fractional = multiplied & 0xFFFF_FFFF;
            }
        }
        (multiplied >> 32) as i32
    }

    fn next_long(&mut self) -> i64 {
        self.rng.next_long()
    }

    fn next_boolean(&mut self) -> bool {
        (self.rng.next_long() & 1) != 0
    }

    fn next_float(&mut self) -> f32 {
        (self.next_bits(24) as f32) * FLOAT_UNIT
    }

    fn next_double(&mut self) -> f64 {
        self.next_double_raw()
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut g = std::mem::take(&mut self.gaussian);
        let val = g.next_gaussian(|| self.next_double_raw());
        self.gaussian = g;
        val
    }
}

#[derive(Debug, Clone, Copy)]
pub struct XoroshiroPositionalRandomFactory {
    seed_lo: i64,
    seed_hi: i64,
}

impl XoroshiroPositionalRandomFactory {
    pub fn at(&self, x: i32, y: i32, z: i32) -> XoroshiroRandomSource {
        let pos_seed = get_seed(x, y, z);
        XoroshiroRandomSource::from_128bit(pos_seed ^ self.seed_lo, self.seed_hi)
    }

    pub fn from_hash_of(&self, name: &str) -> XoroshiroRandomSource {
        let (lo, hi) = seed_from_hash_of(name);
        XoroshiroRandomSource::from_128bit(lo ^ self.seed_lo, hi ^ self.seed_hi)
    }

    pub fn from_seed(&self, seed: i64) -> XoroshiroRandomSource {
        XoroshiroRandomSource::from_128bit(seed ^ self.seed_lo, seed ^ self.seed_hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::with_oracle;
    use serde_json::{Value, json};

    fn assert_rng_matches(mut rng: impl RandomSource, resp: &Value, count: usize) {
        let bounds = [1, 2, 3, 7, 16, 17, 100, 1024, 1_000_003];
        let ints = resp["ints"].as_array().unwrap();
        let bounded = resp["bounded_ints"].as_array().unwrap();
        let longs = resp["longs"].as_array().unwrap();
        let bools = resp["booleans"].as_array().unwrap();
        let floats = resp["float_bits"].as_array().unwrap();
        let doubles = resp["double_bits"].as_array().unwrap();
        let gaussians = resp["gaussian_bits"].as_array().unwrap();

        for i in 0..count {
            assert_eq!(rng.next_int(), ints[i].as_i64().unwrap() as i32, "nextInt #{i}");
            assert_eq!(
                rng.next_int_bounded(bounds[i % bounds.len()]),
                bounded[i].as_i64().unwrap() as i32,
                "nextInt(bound) #{i}"
            );
            assert_eq!(rng.next_long(), longs[i].as_i64().unwrap(), "nextLong #{i}");
            assert_eq!(rng.next_boolean(), bools[i].as_bool().unwrap(), "nextBoolean #{i}");
            assert_eq!(
                rng.next_float().to_bits() as i32,
                floats[i].as_i64().unwrap() as i32,
                "nextFloat #{i}"
            );
            assert_eq!(
                rng.next_double().to_bits() as i64,
                doubles[i].as_i64().unwrap(),
                "nextDouble #{i}"
            );
            // Gaussian uses ln()/sqrt(); allow 0..=1 ULP diff across OS libm vs Java StrictMath/Math
            let rg = rng.next_gaussian().to_bits();
            let jg = gaussians[i].as_i64().unwrap() as u64;
            let ulp_diff = rg.abs_diff(jg);
            assert!(ulp_diff <= 1, "nextGaussian #{i} ULP diff {ulp_diff}: rust={rg:#x} java={jg:#x}");
        }
    }

    #[test]
    fn diff_legacy_random_against_java() {
        for seed in [0_i64, 1, -1, 42, i64::MIN, i64::MAX, -9876543210123456_i64] {
            let count = 32;
            let resp = with_oracle(|o| {
                o.call(json!({ "op": "rng_legacy", "seed": seed, "count": count }))
            })
            .unwrap();
            assert_rng_matches(LegacyRandomSource::new(seed), &resp, count);
        }

        // Positional at + fromHashOf
        let resp_at = with_oracle(|o| {
            o.call(json!({
                "op": "rng_legacy",
                "seed": 123456789_i64,
                "positional_at": [1024, -64, -77777],
                "count": 16
            }))
        })
        .unwrap();
        let rng_at = LegacyRandomSource::new(123456789)
            .fork_positional()
            .at(1024, -64, -77777);
        assert_rng_matches(rng_at, &resp_at, 16);

        let resp_hash = with_oracle(|o| {
            o.call(json!({
                "op": "rng_legacy",
                "seed": 123456789_i64,
                "positional_hash": "minecraft:aquifer",
                "count": 16
            }))
        })
        .unwrap();
        let rng_hash = LegacyRandomSource::new(123456789)
            .fork_positional()
            .from_hash_of("minecraft:aquifer");
        assert_rng_matches(rng_hash, &resp_hash, 16);
    }

    #[test]
    fn diff_xoroshiro_random_against_java() {
        for seed in [0_i64, 1, -1, 42, i64::MIN, i64::MAX, 0x1234_5678_9abc_def0_i64] {
            let count = 32;
            let resp = with_oracle(|o| {
                o.call(json!({ "op": "rng_xoroshiro", "seed": seed, "count": count }))
            })
            .unwrap();
            assert_rng_matches(XoroshiroRandomSource::new(seed), &resp, count);
        }

        // Edge case: (0, 0) fallback to golden/silver ratio
        let resp_zero = with_oracle(|o| {
            o.call(json!({ "op": "rng_xoroshiro", "seed_lo": 0, "seed_hi": 0, "count": 16 }))
        })
        .unwrap();
        assert_rng_matches(XoroshiroRandomSource::from_128bit(0, 0), &resp_zero, 16);

        // Positional at + MD5 fromHashOf
        let resp_at = with_oracle(|o| {
            o.call(json!({
                "op": "rng_xoroshiro",
                "seed": -424242_i64,
                "positional_at": [-50000, 128, 33333],
                "count": 16
            }))
        })
        .unwrap();
        let rng_at = XoroshiroRandomSource::new(-424242)
            .fork_positional()
            .at(-50000, 128, 33333);
        assert_rng_matches(rng_at, &resp_at, 16);

        let resp_hash = with_oracle(|o| {
            o.call(json!({
                "op": "rng_xoroshiro",
                "seed": -424242_i64,
                "positional_hash": "minecraft:ore_veininess",
                "count": 16
            }))
        })
        .unwrap();
        let rng_hash = XoroshiroRandomSource::new(-424242)
            .fork_positional()
            .from_hash_of("minecraft:ore_veininess");
        assert_rng_matches(rng_hash, &resp_hash, 16);
    }
}
