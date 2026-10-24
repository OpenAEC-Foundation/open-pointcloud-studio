//! Vectors, a seeded random stream, smooth noise and the point record that
//! both scenes write.

pub type V3 = [f64; 3];
pub type Rgb = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `a + b * s`.
pub fn along(a: V3, b: V3, s: f64) -> V3 {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}

pub fn normalize(a: V3) -> V3 {
    let size = length(a);
    if size > 0.0 {
        scale(a, 1.0 / size)
    } else {
        [0.0, 0.0, 1.0]
    }
}

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub fn smoothstep(from: f64, to: f64, value: f64) -> f64 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Unit vectors `u` and `v` that make a right-handed frame with `n`.
pub fn frame(n: V3) -> (V3, V3) {
    let helper = if n[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalize(cross(helper, n));
    (u, cross(n, u))
}

/// SplitMix64: spreads the bits of a seed or a coordinate.
pub fn splitmix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn unit_from(bits: u64) -> f64 {
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// A value in [0, 1) that depends only on a seed and three whole numbers,
/// for properties of one brick, tile or board.
pub fn hash(seed: u64, x: i64, y: i64, z: i64) -> f64 {
    let mut h = splitmix(seed);
    h = splitmix(h ^ x as u64);
    h = splitmix(h ^ (y as u64).rotate_left(21));
    h = splitmix(h ^ (z as u64).rotate_left(42));
    unit_from(h)
}

/// A seeded stream of random numbers (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(splitmix(seed) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Even in [0, 1).
    pub fn unit(&mut self) -> f64 {
        unit_from(self.next_u64())
    }

    pub fn range(&mut self, from: f64, to: f64) -> f64 {
        from + (to - from) * self.unit()
    }

    /// Standard normal, by Box and Muller.
    pub fn normal(&mut self) -> f64 {
        let radius = (-2.0 * (1.0 - self.unit()).ln()).sqrt();
        radius * (std::f64::consts::TAU * self.unit()).cos()
    }

    /// Even on the unit sphere.
    pub fn direction(&mut self) -> V3 {
        let z = self.range(-1.0, 1.0);
        let angle = self.range(0.0, std::f64::consts::TAU);
        let across = (1.0 - z * z).sqrt();
        [across * angle.cos(), across * angle.sin(), z]
    }

    pub fn chance(&mut self, probability: f64) -> bool {
        self.unit() < probability
    }
}

fn fade(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in [0, 1] with features about one unit apart.
pub fn noise2(seed: u64, x: f64, y: f64) -> f64 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (tx, ty) = (fade(x - fx), fade(y - fy));
    let corner = |dx: i64, dy: i64| hash(seed, ix + dx, iy + dy, 0);
    let bottom = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * tx;
    let top = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * tx;
    bottom + (top - bottom) * ty
}

/// Smooth value noise in [0, 1] in space.
pub fn noise3(seed: u64, p: V3) -> f64 {
    let floor = p.map(f64::floor);
    let cell = floor.map(|value| value as i64);
    let t = [
        fade(p[0] - floor[0]),
        fade(p[1] - floor[1]),
        fade(p[2] - floor[2]),
    ];
    let corner = |dx: i64, dy: i64, dz: i64| hash(seed, cell[0] + dx, cell[1] + dy, cell[2] + dz);
    let edge =
        |dy: i64, dz: i64| corner(0, dy, dz) + (corner(1, dy, dz) - corner(0, dy, dz)) * t[0];
    let face = |dz: i64| edge(0, dz) + (edge(1, dz) - edge(0, dz)) * t[1];
    face(0) + (face(1) - face(0)) * t[2]
}

/// Noise of several octaves, each twice as fine and half as strong, in [0, 1].
pub fn fbm2(seed: u64, x: f64, y: f64, octaves: u32) -> f64 {
    let (mut sum, mut weight, mut total, mut frequency) = (0.0, 1.0, 0.0, 1.0);
    for octave in 0..octaves {
        sum += weight
            * noise2(
                seed.wrapping_add(u64::from(octave)),
                x * frequency,
                y * frequency,
            );
        total += weight;
        weight *= 0.5;
        frequency *= 2.03;
    }
    sum / total
}

pub fn fbm3(seed: u64, p: V3, octaves: u32) -> f64 {
    let (mut sum, mut weight, mut total, mut frequency) = (0.0, 1.0, 0.0, 1.0);
    for octave in 0..octaves {
        sum += weight * noise3(seed.wrapping_add(u64::from(octave)), scale(p, frequency));
        total += weight;
        weight *= 0.5;
        frequency *= 2.03;
    }
    sum / total
}

/// One point as both scenes write it: registered coordinates, a colour in
/// [0, 1] per channel, a return strength in [0, 1], the ASPRS class and the
/// station or part it comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub xyz: V3,
    pub rgb: Rgb,
    pub intensity: f64,
    pub class: u8,
    pub source: u16,
}

/// ASPRS classes the scenes use.
pub mod class {
    pub const UNCLASSIFIED: u8 = 1;
    pub const GROUND: u8 = 2;
    pub const LOW_VEGETATION: u8 = 3;
    pub const MEDIUM_VEGETATION: u8 = 4;
    pub const HIGH_VEGETATION: u8 = 5;
    pub const BUILDING: u8 = 6;
}
