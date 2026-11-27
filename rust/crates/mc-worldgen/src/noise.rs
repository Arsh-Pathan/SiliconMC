use mc_data::noises::{NoiseNormalization, NormalNoiseSpec, vanilla_noise_spec};
use mc_math::{
    LegacyRandomSource, RandomSource, XoroshiroRandomSource, floor_f64, lerp3_f32, smoothstep,
};

const GRADIENT: [(i32, i32, i32); 16] = [
    (1, 1, 0),
    (-1, 1, 0),
    (1, -1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (-1, 0, 1),
    (1, 0, -1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, -1, 1),
    (0, 1, -1),
    (0, -1, -1),
    (1, 1, 0),
    (0, -1, 1),
    (-1, 1, 0),
    (0, -1, -1),
];

const ROUND_OFF: f64 = 3.3554432e7_f64; // 33_554_432
/// Exact match for Java `Math.nextDown(1.6777216E7)`.
const HALF_ROUND_OFF: f64 = 1.6777216e7_f64.next_down();

/// Exact match for Minecraft 26.3 `GradientNoise.wrap(double)`.
#[inline]
pub fn wrap_noise_coord(x: f64) -> f64 {
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        x
    } else {
        x - (x / ROUND_OFF + 0.5).floor() * ROUND_OFF
    }
}

#[inline]
fn grad_dot(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let (gx, gy, gz) = GRADIENT[(hash & 15) as usize];
    (gx as f32) * x + (gy as f32) * y + (gz as f32) * z
}

/// Bit-exact port of Minecraft 26.3's `net.minecraft.world.level.levelgen.synth.PerlinNoise`
/// (subclass of `GradientNoise`, returning `f32`).
#[derive(Debug, Clone)]
pub struct PerlinNoise {
    perms: [u8; 256],
    offset_x: f64,
    offset_y: f64,
    offset_z: f64,
}

impl PerlinNoise {
    pub const STANDARD_DEVIATION: f64 = 0.2702247831245211;

    pub fn new(random: &mut impl RandomSource) -> Self {
        Self::with_offset_scale(random, 256.0)
    }

    pub fn with_offset_scale(random: &mut impl RandomSource, noise_offset_scale: f64) -> Self {
        let offset_x = random.next_double() * noise_offset_scale;
        let offset_y = random.next_double() * noise_offset_scale;
        let offset_z = random.next_double() * noise_offset_scale;

        let mut perms = [0u8; 256];
        for (i, slot) in perms.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_int_bounded((256 - i) as i32) as usize;
            perms.swap(i, offset + i);
        }

        Self {
            perms,
            offset_x,
            offset_y,
            offset_z,
        }
    }

    #[inline]
    fn permute(&self, x: i32) -> i32 {
        self.perms[(x & 0xFF) as usize] as i32
    }

    #[inline]
    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        self.get_3d(wrap_noise_coord(x), 0.0, wrap_noise_coord(y))
    }

    #[inline]
    pub fn get_3d(&self, in_x: f64, in_y: f64, in_z: f64) -> f32 {
        let x = wrap_noise_coord(in_x) + self.offset_x;
        let y = wrap_noise_coord(in_y) + self.offset_y;
        let z = wrap_noise_coord(in_z) + self.offset_z;

        let floor_x = floor_f64(x);
        let floor_y = floor_f64(y);
        let floor_z = floor_f64(z);

        let rx = (x - floor_x as f64) as f32;
        let ry = (y - floor_y as f64) as f32;
        let rz = (z - floor_z as f64) as f32;

        self.sample_and_lerp(floor_x, floor_y, floor_z, rx, ry, rz, ry)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn sample_and_lerp(
        &self,
        x: i32,
        y: i32,
        z: i32,
        rx: f32,
        ry: f32,
        rz: f32,
        original_ry: f32,
    ) -> f32 {
        let x0 = self.permute(x);
        let x1 = self.permute(x.wrapping_add(1));
        let xy00 = self.permute(x0.wrapping_add(y));
        let xy01 = self.permute(x0.wrapping_add(y).wrapping_add(1));
        let xy10 = self.permute(x1.wrapping_add(y));
        let xy11 = self.permute(x1.wrapping_add(y).wrapping_add(1));

        let d000 = grad_dot(self.permute(xy00.wrapping_add(z)), rx, ry, rz);
        let d100 = grad_dot(self.permute(xy10.wrapping_add(z)), rx - 1.0_f32, ry, rz);
        let d010 = grad_dot(self.permute(xy01.wrapping_add(z)), rx, ry - 1.0_f32, rz);
        let d110 = grad_dot(self.permute(xy11.wrapping_add(z)), rx - 1.0_f32, ry - 1.0_f32, rz);
        let d001 = grad_dot(self.permute(xy00.wrapping_add(z).wrapping_add(1)), rx, ry, rz - 1.0_f32);
        let d101 = grad_dot(
            self.permute(xy10.wrapping_add(z).wrapping_add(1)),
            rx - 1.0_f32,
            ry,
            rz - 1.0_f32,
        );
        let d011 = grad_dot(
            self.permute(xy01.wrapping_add(z).wrapping_add(1)),
            rx,
            ry - 1.0_f32,
            rz - 1.0_f32,
        );
        let d111 = grad_dot(
            self.permute(xy11.wrapping_add(z).wrapping_add(1)),
            rx - 1.0_f32,
            ry - 1.0_f32,
            rz - 1.0_f32,
        );

        let x_alpha = smoothstep(rx);
        let y_alpha = smoothstep(original_ry);
        let z_alpha = smoothstep(rz);
        lerp3_f32(x_alpha, y_alpha, z_alpha, d000, d100, d010, d110, d001, d101, d011, d111)
    }
}

#[derive(Debug, Clone)]
pub struct NoiseLayer {
    pub noise: PerlinNoise,
    pub frequency: f64,
    pub amplitude: f32,
}

/// Bit-exact port of Minecraft 26.3's `net.minecraft.world.level.levelgen.synth.NoiseStack`.
#[derive(Debug, Clone)]
pub struct NoiseStack {
    pub layers: Vec<NoiseLayer>,
}

impl NoiseStack {
    #[inline]
    pub fn get_3d(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let freq = layer.frequency;
            value += layer.amplitude * layer.noise.get_3d(x * freq, y * freq, z * freq);
        }
        value
    }

    #[inline]
    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let freq = layer.frequency;
            value += layer.amplitude * layer.noise.get_2d(x * freq, y * freq);
        }
        value
    }
}

const INPUT_FACTOR: f64 = 1.0181268882175227;
const TARGET_DEVIATION: f64 = 0.3333333333333333;

#[derive(Debug, Clone)]
struct OctaveInfo {
    octave_index: i32,
    frequency: f64,
    amplitude: f64,
}

/// Bit-exact port of Minecraft 26.3's `net.minecraft.world.level.levelgen.synth.NormalNoise`.
#[derive(Debug, Clone)]
pub struct NormalNoise {
    octaves: Vec<OctaveInfo>,
    normalization_factor: f64,
}

impl NormalNoise {
    pub fn from_vanilla_id(id: &str) -> Option<Self> {
        vanilla_noise_spec(id).map(Self::from_spec)
    }

    pub fn from_spec(spec: &NormalNoiseSpec) -> Self {
        let normalize_octaves = spec.normalization != NoiseNormalization::Disabled;
        let octaves = Self::build_octaves(
            spec.base_octave,
            spec.base_amplitude,
            spec.octave_count,
            normalize_octaves,
            spec.amplitude_modifiers,
        );
        let target_amplitude: f64 = octaves.iter().map(|o| o.amplitude.abs()).sum();
        let mut normalization_factor = Self::compute_normalization_factor(target_amplitude, &octaves);
        if spec.normalization == NoiseNormalization::Legacy && normalization_factor != 0.0 {
            normalization_factor = Self::compute_parity_normalization_factor(
                spec.base_amplitude,
                spec.octave_count,
                spec.amplitude_modifiers,
            );
        }
        Self {
            octaves,
            normalization_factor,
        }
    }

    fn build_octaves(
        base_octave: i32,
        base_amplitude: f64,
        octave_count: usize,
        normalize: bool,
        amplitude_modifiers: &[f64],
    ) -> Vec<OctaveInfo> {
        let mut frequency = 2.0_f64.powi(base_octave);
        let mut amplitude = base_amplitude;
        if normalize {
            amplitude *= 0.5_f64.powi(-((octave_count as i32) - 1))
                / (0.5_f64.powi(-(octave_count as i32)) - 1.0);
        }

        let mut octaves = Vec::with_capacity(octave_count);
        for i in 0..octave_count {
            let modifier = if amplitude_modifiers.is_empty() {
                1.0
            } else {
                amplitude_modifiers[i]
            };
            if modifier != 0.0 {
                octaves.push(OctaveInfo {
                    octave_index: base_octave + i as i32,
                    frequency,
                    amplitude: amplitude * modifier,
                });
            }
            frequency *= 2.0;
            amplitude *= 0.5;
        }
        octaves
    }

    fn compute_normalization_factor(target_amplitude: f64, octaves: &[OctaveInfo]) -> f64 {
        let mut variance = 0.0_f64;
        for oct in octaves {
            let layer_dev = PerlinNoise::STANDARD_DEVIATION * oct.amplitude.abs();
            variance += layer_dev * layer_dev;
        }
        let input_dev = variance.sqrt();
        if input_dev == 0.0 {
            return 0.0;
        }
        let input_sum_dev = input_dev * 2.0_f64.sqrt();
        let target_dev = target_amplitude * TARGET_DEVIATION;
        target_dev / input_sum_dev
    }

    fn compute_parity_normalization_factor(
        base_amplitude: f64,
        octave_count: usize,
        amplitude_modifiers: &[f64],
    ) -> f64 {
        let mut min_oct = i32::MAX;
        let mut max_oct = i32::MIN;
        for i in 0..octave_count {
            let modifier = if amplitude_modifiers.is_empty() {
                1.0
            } else {
                amplitude_modifiers[i]
            };
            if modifier != 0.0 {
                min_oct = min_oct.min(i as i32);
                max_oct = max_oct.max(i as i32);
            }
        }
        let span = max_oct - min_oct;
        let expected_dev = 0.1 * (1.0 + 1.0 / (span as f64 + 1.0));
        base_amplitude * 0.5 * TARGET_DEVIATION / expected_dev
    }

    /// Instantiate a seeded `NoiseStack` matching `NormalNoise.create(RandomSource)` bit-for-bit.
    pub fn instantiate(&self, random: &mut XoroshiroRandomSource) -> NoiseStack {
        let first_random = random.fork_positional();
        let second_random = random.fork_positional();
        let mut layers = Vec::with_capacity(self.octaves.len() * 2);

        for oct in &self.octaves {
            let seed_name = format!("octave_{}", oct.octave_index);
            let first = PerlinNoise::new(&mut first_random.from_hash_of(&seed_name));
            let second = PerlinNoise::new(&mut second_random.from_hash_of(&seed_name));
            let value_factor = (self.normalization_factor * oct.amplitude) as f32;
            layers.push(NoiseLayer {
                noise: first,
                frequency: oct.frequency,
                amplitude: value_factor,
            });
            layers.push(NoiseLayer {
                noise: second,
                frequency: oct.frequency * INPUT_FACTOR,
                amplitude: value_factor,
            });
        }

        NoiseStack { layers }
    }

    /// Instantiate a seeded `NoiseStack` using `LegacyRandomSource` (for Nether/End material rules).
    pub fn instantiate_legacy(&self, random: &mut LegacyRandomSource) -> NoiseStack {
        let first_random = random.fork_positional();
        let second_random = random.fork_positional();
        let mut layers = Vec::with_capacity(self.octaves.len() * 2);

        for oct in &self.octaves {
            let seed_name = format!("octave_{}", oct.octave_index);
            let first = PerlinNoise::new(&mut first_random.from_hash_of(&seed_name));
            let second = PerlinNoise::new(&mut second_random.from_hash_of(&seed_name));
            let value_factor = (self.normalization_factor * oct.amplitude) as f32;
            layers.push(NoiseLayer {
                noise: first,
                frequency: oct.frequency,
                amplitude: value_factor,
            });
            layers.push(NoiseLayer {
                noise: second,
                frequency: oct.frequency * INPUT_FACTOR,
                amplitude: value_factor,
            });
        }

        NoiseStack { layers }
    }

    /// Bit-exact port of `NormalNoise.createForLegacyNetherBiome` + `LegacyFbmInitializer`
    /// (used for `minecraft:nether/temperature` and `minecraft:nether/vegetation`).
    pub fn instantiate_for_legacy_nether_biome(
        &self,
        spec: &NormalNoiseSpec,
        random: &mut LegacyRandomSource,
    ) -> NoiseStack {
        let amps: Vec<f64> = if spec.amplitude_modifiers.is_empty() {
            vec![1.0; spec.octave_count]
        } else {
            spec.amplitude_modifiers.to_vec()
        };
        let first = legacy_fbm_nether(random, spec.base_octave, &amps);
        let second = legacy_fbm_nether(random, spec.base_octave, &amps);
        let value_factor = (self.normalization_factor * spec.base_amplitude) as f32;

        let mut layers = Vec::with_capacity(first.layers.len() + second.layers.len());
        for l in first.layers {
            layers.push(NoiseLayer {
                noise: l.noise,
                frequency: l.frequency,
                amplitude: l.amplitude * value_factor,
            });
        }
        for l in second.layers {
            layers.push(NoiseLayer {
                noise: l.noise,
                frequency: l.frequency * INPUT_FACTOR,
                amplitude: l.amplitude * value_factor,
            });
        }
        NoiseStack { layers }
    }
}

fn legacy_fbm_nether(
    random: &mut LegacyRandomSource,
    first_octave: i32,
    amplitudes: &[f64],
) -> NoiseStack {
    let octaves = amplitudes.len();
    let zero_octave_index = -first_octave;
    let mut noise_levels: Vec<Option<PerlinNoise>> = vec![None; octaves];
    let zero_octave = PerlinNoise::new(random);
    if zero_octave_index >= 0 && (zero_octave_index as usize) < octaves {
        if amplitudes[zero_octave_index as usize] != 0.0 {
            noise_levels[zero_octave_index as usize] = Some(zero_octave);
        }
    }
    for i in (0..zero_octave_index).rev() {
        let iu = i as usize;
        if iu < octaves && amplitudes[iu] != 0.0 {
            noise_levels[iu] = Some(PerlinNoise::new(random));
        } else {
            for _ in 0..262 {
                random.next_int();
            }
        }
    }
    let mut factor = 2.0_f64.powi(-zero_octave_index);
    let mut value_factor =
        2.0_f64.powi(octaves as i32 - 1) / (2.0_f64.powi(octaves as i32) - 1.0);
    let mut layers = Vec::new();
    for (i, noise) in noise_levels.into_iter().enumerate() {
        if let Some(n) = noise {
            layers.push(NoiseLayer {
                noise: n,
                frequency: factor,
                amplitude: (value_factor * amplitudes[i]) as f32,
            });
        }
        factor *= 2.0;
        value_factor /= 2.0;
    }
    NoiseStack { layers }
}

/// Bit-exact port of Minecraft 26.3's `net.minecraft.world.level.levelgen.synth.SimplexNoise`
/// (used by `EndIslandFunction` for outer End islands).
#[derive(Debug, Clone)]
pub struct SimplexNoise {
    perms: [u8; 256],
    offset_x: f64,
    offset_y: f64,
}

impl SimplexNoise {
    const SQRT_3: f64 = 1.7320508075688772_f64;
    const F2: f64 = 0.5 * (Self::SQRT_3 - 1.0);
    const G2: f64 = (3.0 - Self::SQRT_3) / 6.0;

    pub fn new(random: &mut impl RandomSource, discard_noise_offset: bool) -> Self {
        let scale = if discard_noise_offset { 0.0 } else { 256.0 };
        let offset_x = random.next_double() * scale;
        let offset_y = random.next_double() * scale;
        let _offset_z = random.next_double() * scale;

        let mut perms = [0u8; 256];
        for (i, slot) in perms.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_int_bounded((256 - i) as i32) as usize;
            perms.swap(i, offset + i);
        }
        Self {
            perms,
            offset_x,
            offset_y,
        }
    }

    #[inline]
    fn permute(&self, x: i32) -> i32 {
        self.perms[(x & 0xFF) as usize] as i32
    }

    #[inline]
    fn corner_noise_2d(index: usize, x: f64, y: f64) -> f64 {
        let t0 = 0.5 - x * x - y * y;
        if t0 < 0.0 {
            0.0
        } else {
            let t2 = t0 * t0;
            let (gx, gy, _) = GRADIENT[index];
            t2 * t2 * ((gx as f64) * x + (gy as f64) * y)
        }
    }

    #[inline]
    pub fn get_2d(&self, in_x: f64, in_y: f64) -> f32 {
        let xin = in_x + self.offset_x;
        let yin = in_y + self.offset_y;
        let s = (xin + yin) * Self::F2;
        let i = floor_f64(xin + s);
        let j = floor_f64(yin + s);
        let t = (i + j) as f64 * Self::G2;
        let x0 = xin - (i as f64 - t);
        let y0 = yin - (j as f64 - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - i1 as f64 + Self::G2;
        let y1 = y0 - j1 as f64 + Self::G2;
        let x2 = x0 - 1.0 + 2.0 * Self::G2;
        let y2 = y0 - 1.0 + 2.0 * Self::G2;

        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gi0 = (self.permute(ii + self.permute(jj)) % 12) as usize;
        let gi1 = (self.permute(ii + i1 + self.permute(jj + j1)) % 12) as usize;
        let gi2 = (self.permute(ii + 1 + self.permute(jj + 1)) % 12) as usize;

        let n0 = Self::corner_noise_2d(gi0, x0, y0);
        let n1 = Self::corner_noise_2d(gi1, x1, y1);
        let n2 = Self::corner_noise_2d(gi2, x2, y2);
        (70.0 * (n0 + n1 + n2)) as f32
    }
}

/// Bit-exact port of Minecraft 26.3's `SmearedPerlinNoise` (used by `BlendedNoise`).
#[derive(Debug, Clone)]
pub struct SmearedPerlinNoise {
    perlin: PerlinNoise,
    fudge_y_scale: f64,
}

impl SmearedPerlinNoise {
    const SHIFT_UP_EPSILON: f64 = 1.0e-7_f32 as f64;

    pub fn new(random: &mut impl RandomSource, fudge_y_scale: f64) -> Self {
        Self {
            perlin: PerlinNoise::new(random),
            fudge_y_scale,
        }
    }

    #[inline]
    fn compute_fudge_y(&self, original_y: f64, relative_y: f64) -> f64 {
        let fudge_limit = if original_y >= 0.0 && original_y < relative_y {
            original_y
        } else {
            relative_y
        };
        (floor_f64(fudge_limit / self.fudge_y_scale + Self::SHIFT_UP_EPSILON) as f64)
            * self.fudge_y_scale
    }

    #[inline]
    pub fn get_3d(&self, in_x: f64, in_y: f64, in_z: f64) -> f32 {
        let x = wrap_noise_coord(in_x) + self.perlin.offset_x;
        let y = wrap_noise_coord(in_y) + self.perlin.offset_y;
        let z = wrap_noise_coord(in_z) + self.perlin.offset_z;

        let floor_x = floor_f64(x);
        let floor_y = floor_f64(y);
        let floor_z = floor_f64(z);

        let rx = (x - floor_x as f64) as f32;
        let ry_f64 = y - floor_y as f64;
        let rz = (z - floor_z as f64) as f32;
        let fudged_ry = (ry_f64 - self.compute_fudge_y(in_y, ry_f64)) as f32;

        self.perlin
            .sample_and_lerp(floor_x, floor_y, floor_z, rx, fudged_ry, rz, ry_f64 as f32)
    }
}

#[derive(Debug, Clone)]
pub struct SmearedNoiseLayer {
    pub noise: SmearedPerlinNoise,
    pub frequency: f64,
    pub amplitude: f32,
}

#[derive(Debug, Clone)]
pub struct SmearedNoiseStack {
    pub layers: Vec<SmearedNoiseLayer>,
}

impl SmearedNoiseStack {
    #[inline]
    pub fn get_3d(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let freq = layer.frequency;
            value += layer.amplitude * layer.noise.get_3d(x * freq, y * freq, z * freq);
        }
        value
    }
}

/// Bit-exact port of Minecraft 26.3's `BlendedNoise` (`minecraft:old_blended_noise`).
#[derive(Debug, Clone)]
pub struct BlendedNoise {
    min_limit: SmearedNoiseStack,
    max_limit: SmearedNoiseStack,
    main_noise: SmearedNoiseStack,
    xz_mult: f64,
    y_mult: f64,
    main_xz_mult: f64,
    main_y_mult: f64,
}

impl BlendedNoise {
    const BASE_SCALE: f64 = 684.412;
    const LIMIT_FACTOR: f64 = 0.99998474_f32 as f64;
    const MAIN_FACTOR: f64 = 12.75;

    pub fn new(
        random: &mut impl RandomSource,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let xz_mult = Self::BASE_SCALE * xz_scale;
        let y_mult = Self::BASE_SCALE * y_scale;
        let limit_smear_y = y_mult * smear_scale_multiplier;
        let main_smear_y = limit_smear_y / y_factor;

        let min_limit = Self::create_fbm(random, -15, limit_smear_y, Self::LIMIT_FACTOR);
        let max_limit = Self::create_fbm(random, -15, limit_smear_y, Self::LIMIT_FACTOR);
        let main_noise = Self::create_fbm(random, -7, main_smear_y, Self::MAIN_FACTOR);

        Self {
            min_limit,
            max_limit,
            main_noise,
            xz_mult,
            y_mult,
            main_xz_mult: xz_mult / xz_factor,
            main_y_mult: y_mult / y_factor,
        }
    }

    fn create_fbm(
        random: &mut impl RandomSource,
        first_octave: i32,
        smear_scale_y: f64,
        mut value_factor: f64,
    ) -> SmearedNoiseStack {
        let octaves = (-first_octave + 1) as usize;
        let mut factor = 1.0_f64;
        value_factor /= 2.0_f64.powi(octaves as i32) - 1.0;
        let mut layers = Vec::with_capacity(octaves);
        for _ in 0..octaves {
            layers.push(SmearedNoiseLayer {
                noise: SmearedPerlinNoise::new(random, smear_scale_y * factor),
                frequency: factor,
                amplitude: value_factor as f32,
            });
            factor /= 2.0;
            value_factor *= 2.0;
        }
        SmearedNoiseStack { layers }
    }

    #[inline]
    pub fn sample(&self, block_x: i32, block_y: i32, block_z: i32) -> f32 {
        let bx = block_x as f64;
        let by = block_y as f64;
        let bz = block_z as f64;

        let main_val = self
            .main_noise
            .get_3d(bx * self.main_xz_mult, by * self.main_y_mult, bz * self.main_xz_mult);
        let choice = (main_val + 0.5_f32).clamp(0.0_f32, 1.0_f32);
        if choice == 0.0_f32 {
            self.min_limit
                .get_3d(bx * self.xz_mult, by * self.y_mult, bz * self.xz_mult)
        } else if choice == 1.0_f32 {
            self.max_limit
                .get_3d(bx * self.xz_mult, by * self.y_mult, bz * self.xz_mult)
        } else {
            let min_v = self
                .min_limit
                .get_3d(bx * self.xz_mult, by * self.y_mult, bz * self.xz_mult);
            let max_v = self
                .max_limit
                .get_3d(bx * self.xz_mult, by * self.y_mult, bz * self.xz_mult);
            mc_math::lerp_f32(choice, min_v, max_v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::with_oracle;
    use serde_json::json;

    #[test]
    fn diff_perlin_and_normal_noises_bit_exact_against_java_26_3() {
        let coords: Vec<[f64; 3]> = vec![
            [0.0, 0.0, 0.0],
            [0.5, -0.5, 1.25],
            [12.345, -64.0, 987.654],
            [-12345.67, 128.125, -42.5],
            [33_554_433.5, 10.0, -33_554_430.25], // exercises wrap() across ROUND_OFF boundary!
            [1000.0, 319.0, -9999.0],
            [-0.123456, 0.789012, -0.456789],
            [512.0, 64.0, 512.0],
        ];
        let coords_json: Vec<_> = coords.iter().map(|c| json!(c)).collect();

        // 1. Raw PerlinNoise bit-exact check
        for seed in [0_i64, 42, -9876543210_i64] {
            let resp = with_oracle(|o| {
                o.call(json!({
                    "op": "noise_sample",
                    "kind": "perlin",
                    "seed": seed,
                    "coords": coords_json,
                }))
            })
            .unwrap();
            let java_bits = resp["values_bits"].as_array().unwrap();
            let perlin = PerlinNoise::new(&mut XoroshiroRandomSource::new(seed));
            for (i, &[x, y, z]) in coords.iter().enumerate() {
                let r_bits = perlin.get_3d(x, y, z).to_bits() as i32;
                let j_bits = java_bits[i].as_i64().unwrap() as i32;
                assert_eq!(
                    r_bits, j_bits,
                    "PerlinNoise mismatch seed={seed} coord={:?}: rust={} java={}",
                    coords[i],
                    f32::from_bits(r_bits as u32),
                    f32::from_bits(j_bits as u32)
                );
            }
        }

        // 2. Vanilla NormalNoise specs across Enabled, Legacy, Disabled & multi-octave modifiers
        for noise_id in [
            "minecraft:temperature",
            "minecraft:continentalness",
            "minecraft:erosion",
            "minecraft:ridge",
            "minecraft:aquifer_barrier",
            "minecraft:noodle",
            "minecraft:calcite",
        ] {
            let seed = 0x51C0_4E26_3000_i64;
            let resp = with_oracle(|o| {
                o.call(json!({
                    "op": "noise_sample",
                    "kind": "normal",
                    "noise_id": noise_id,
                    "seed": seed,
                    "coords": coords_json,
                }))
            })
            .unwrap();
            let java_bits = resp["values_bits"].as_array().unwrap();
            let nn = NormalNoise::from_vanilla_id(noise_id)
                .unwrap_or_else(|| panic!("Missing noise spec {noise_id}"));
            let stack = nn.instantiate(&mut XoroshiroRandomSource::new(seed));
            for (i, &[x, y, z]) in coords.iter().enumerate() {
                let r_bits = stack.get_3d(x, y, z).to_bits() as i32;
                let j_bits = java_bits[i].as_i64().unwrap() as i32;
                assert_eq!(
                    r_bits, j_bits,
                    "NormalNoise({noise_id}) mismatch coord={:?}: rust={} java={}",
                    coords[i],
                    f32::from_bits(r_bits as u32),
                    f32::from_bits(j_bits as u32)
                );
            }
        }
    }
}
