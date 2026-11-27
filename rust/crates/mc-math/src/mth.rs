use std::sync::LazyLock;

pub const PI: f32 = std::f32::consts::PI;
pub const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
pub const TWO_PI: f32 = std::f32::consts::TAU;
pub const DEG_TO_RAD: f32 = (std::f64::consts::PI / 180.0) as f32;
pub const RAD_TO_DEG: f32 = 180.0_f32 / PI;
pub const EPSILON: f32 = 1.0e-5_f32;

const SIN_QUANTIZATION: usize = 65536;
const SIN_MASK: i64 = 65535;
const COS_OFFSET: f64 = 16384.0;
const SIN_SCALE: f64 = 10430.378350470453;

static SIN_TABLE: LazyLock<Box<[f32; SIN_QUANTIZATION]>> = LazyLock::new(|| {
    let mut table = Box::new([0.0_f32; SIN_QUANTIZATION]);
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = ((i as f64) / SIN_SCALE).sin() as f32;
    }
    table
});

const MULTIPLY_DE_BRUIJN_BIT_POSITION: [i32; 32] = [
    0, 1, 28, 2, 29, 14, 24, 3, 30, 22, 20, 15, 25, 17, 4, 8, 31, 27, 13, 23, 21, 19, 16, 7, 26,
    12, 18, 6, 11, 5, 10, 9,
];

/// Exact match for `net.minecraft.util.Mth.sin(double)`.
#[inline]
pub fn sin(v: f64) -> f32 {
    let idx = ((v * SIN_SCALE) as i64 & SIN_MASK) as usize;
    SIN_TABLE[idx]
}

/// Exact match for `net.minecraft.util.Mth.cos(double)`.
#[inline]
pub fn cos(v: f64) -> f32 {
    let idx = ((v * SIN_SCALE + COS_OFFSET) as i64 & SIN_MASK) as usize;
    SIN_TABLE[idx]
}

#[inline]
pub fn floor_f32(v: f32) -> i32 {
    v.floor() as i32
}

#[inline]
pub fn floor_f64(v: f64) -> i32 {
    v.floor() as i32
}

#[inline]
pub fn lfloor(v: f64) -> i64 {
    v.floor() as i64
}

#[inline]
pub fn ceil_f32(v: f32) -> i32 {
    v.ceil() as i32
}

#[inline]
pub fn ceil_f64(v: f64) -> i32 {
    v.ceil() as i32
}

/// Exact match for Java's `Math.floorDiv(int, int)`.
#[inline]
pub fn floor_div(a: i32, b: i32) -> i32 {
    let mut r = a.wrapping_div(b);
    if (a ^ b) < 0 && r.wrapping_mul(b) != a {
        r = r.wrapping_sub(1);
    }
    r
}

/// Exact match for Java's `Math.floorMod(int, int)`.
#[inline]
pub fn floor_mod(a: i32, b: i32) -> i32 {
    a.wrapping_sub(floor_div(a, b).wrapping_mul(b))
}

#[inline]
pub fn smallest_encompassing_power_of_two(input: i32) -> i32 {
    let mut r = input.wrapping_sub(1);
    r |= r >> 1;
    r |= r >> 2;
    r |= r >> 4;
    r |= r >> 8;
    r |= r >> 16;
    r.wrapping_add(1)
}

#[inline]
pub fn is_power_of_two(input: i32) -> bool {
    input != 0 && (input & input.wrapping_sub(1)) == 0
}

#[inline]
pub fn ceillog2(mut input: i32) -> i32 {
    input = if is_power_of_two(input) {
        input
    } else {
        smallest_encompassing_power_of_two(input)
    };
    let idx = (((input as i64).wrapping_mul(125613361_i64) >> 27) & 31) as usize;
    MULTIPLY_DE_BRUIJN_BIT_POSITION[idx]
}

#[inline]
pub fn log2(input: i32) -> i32 {
    ceillog2(input) - if is_power_of_two(input) { 0 } else { 1 }
}

/// Exact match for `net.minecraft.util.Mth.getSeed(int, int, int)`.
/// Note: `x * 3129871` intentionally wraps in 32-bit signed `int` before widening to `i64`.
#[inline]
pub fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut seed = (x.wrapping_mul(3129871) as i64)
        ^ ((z as i64).wrapping_mul(116129781_i64))
        ^ (y as i64);
    seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(42317861_i64)
        .wrapping_add(seed.wrapping_mul(11_i64));
    seed >> 16
}

/// Exact match for Java's `String.hashCode()` (UTF-16 code units).
pub fn java_string_hash_code(s: &str) -> i32 {
    let mut h: i32 = 0;
    for unit in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(unit as i32);
    }
    h
}

#[inline]
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut v = angle % 360.0_f32;
    if v >= 180.0_f32 {
        v -= 360.0_f32;
    }
    if v < -180.0_f32 {
        v += 360.0_f32;
    }
    v
}

/// Exact match for `net.minecraft.util.Mth.smoothstep(float)`.
#[inline]
pub fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0_f32 - 15.0_f32) + 10.0_f32)
}

/// Exact match for `net.minecraft.util.Mth.smoothstepDerivative(float)`.
#[inline]
pub fn smoothstep_derivative(x: f32) -> f32 {
    30.0_f32 * x * x * (x - 1.0_f32) * (x - 1.0_f32)
}

/// Exact match for `net.minecraft.util.Mth.lerp(float, float, float)`.
#[inline]
pub fn lerp_f32(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

/// Exact match for `net.minecraft.util.Mth.lerp2(float, float, ...)`.
#[inline]
pub fn lerp2_f32(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    lerp_f32(a2, lerp_f32(a1, x00, x10), lerp_f32(a1, x01, x11))
}

/// Exact match for `net.minecraft.util.Mth.lerp3(float, float, float, ...)`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn lerp3_f32(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    lerp_f32(
        a3,
        lerp2_f32(a1, a2, x000, x100, x010, x110),
        lerp2_f32(a1, a2, x001, x101, x011, x111),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlockPos;
    use mc_oracle::with_oracle;
    use serde_json::json;

    #[test]
    fn diff_mth_sin_cos_blockpos_and_seed_against_java() {
        let trig_inputs: Vec<f64> = vec![
            0.0,
            1.0,
            -1.0,
            0.5,
            std::f64::consts::PI,
            -std::f64::consts::PI,
            std::f64::consts::TAU,
            123.456789,
            -98765.4321,
            1e-7,
            42.0,
            -0.000123,
        ];
        let trig_bits: Vec<i64> = trig_inputs.iter().map(|v| v.to_bits() as i64).collect();

        let positions: Vec<(i32, i32, i32)> = vec![
            (0, 0, 0),
            (1, 2, 3),
            (-1, -64, -1),
            (12345, -64, -98765),
            (30_000_000, 319, -30_000_000),
            (-30_000_000, -2048, 30_000_000),
            (33_554_431, 2047, -33_554_432),
            (999_999, 100, 888_888),
        ];

        let resp = with_oracle(|o| {
            o.call(json!({
                "op": "mth_sample",
                "trig_inputs": trig_bits,
                "block_positions": positions.iter().map(|(x, y, z)| json!([x, y, z])).collect::<Vec<_>>(),
            }))
        })
        .unwrap();

        let java_sin = resp["sin_bits"].as_array().unwrap();
        let java_cos = resp["cos_bits"].as_array().unwrap();
        for (i, &v) in trig_inputs.iter().enumerate() {
            assert_eq!(
                sin(v).to_bits() as i32,
                java_sin[i].as_i64().unwrap() as i32,
                "Mth.sin mismatch for {v}"
            );
            assert_eq!(
                cos(v).to_bits() as i32,
                java_cos[i].as_i64().unwrap() as i32,
                "Mth.cos mismatch for {v}"
            );
        }

        let java_packed = resp["packed"].as_array().unwrap();
        let java_unpacked = resp["unpacked"].as_array().unwrap();
        let java_seeds = resp["seeds"].as_array().unwrap();
        for (i, &(x, y, z)) in positions.iter().enumerate() {
            let bp = BlockPos::new(x, y, z);
            let p = bp.as_long();
            assert_eq!(p, java_packed[i].as_i64().unwrap(), "BlockPos.asLong mismatch for {bp:?}");
            let u = BlockPos::from_long(p);
            let ju = java_unpacked[i].as_array().unwrap();
            assert_eq!(
                (u.x, u.y, u.z),
                (
                    ju[0].as_i64().unwrap() as i32,
                    ju[1].as_i64().unwrap() as i32,
                    ju[2].as_i64().unwrap() as i32
                )
            );
            assert_eq!(get_seed(x, y, z), java_seeds[i].as_i64().unwrap(), "Mth.getSeed mismatch for {bp:?}");
        }
    }
}
