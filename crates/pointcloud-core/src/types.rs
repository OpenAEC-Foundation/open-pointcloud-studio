//! Core point storage.
//!
//! Points are held as a struct-of-arrays with quantised integer positions
//! rather than an array of `{f64 x3, u8 x3, u16, u8}` structs. That is 18
//! bytes per point instead of 32 (the struct padded to an 8-byte alignment),
//! and it lets each attribute be scanned, sorted and copied independently.
//!
//! Quantisation matches how LAS already stores coordinates: an i32 count of
//! scale units from an origin. At the default 1 mm scale an i32 spans about
//! ±2,100 km, far beyond any real survey.

/// Axis-aligned bounding box in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn empty() -> Self {
        Self {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
        }
    }

    pub fn expand(&mut self, p: [f64; 3]) {
        for i in 0..3 {
            if p[i] < self.min[i] {
                self.min[i] = p[i];
            }
            if p[i] > self.max[i] {
                self.max[i] = p[i];
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }

    pub fn center(&self) -> [f64; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn size(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    pub fn max_extent(&self) -> f64 {
        let s = self.size();
        s[0].max(s[1]).max(s[2])
    }

    /// The smallest cube containing this box, centred on it.
    ///
    /// The octree subdivides a cube so that a node's extent is the same on
    /// every axis; without this, thin datasets (a corridor scan, a facade)
    /// produce nodes whose screen-space size depends on which axis you
    /// happen to measure.
    pub fn to_cube(&self) -> Bounds {
        let c = self.center();
        let half = self.max_extent() * 0.5;
        Bounds {
            min: [c[0] - half, c[1] - half, c[2] - half],
            max: [c[0] + half, c[1] + half, c[2] + half],
        }
    }
}

/// Points in struct-of-arrays form with quantised positions.
///
/// `world = origin + quantised * scale`
pub struct PointCloud {
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    pub z: Vec<i32>,
    pub rgb: Vec<[u8; 3]>,
    pub intensity: Vec<u16>,
    pub classification: Vec<u8>,

    pub origin: [f64; 3],
    pub scale: [f64; 3],
    pub bounds: Bounds,

    pub has_color: bool,
    pub has_intensity: bool,
    pub has_classification: bool,
}

impl PointCloud {
    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    /// Bytes held per point, for reporting memory behaviour honestly.
    pub const BYTES_PER_POINT: usize = 4 * 3 + 3 + 2 + 1;

    pub fn world(&self, i: usize) -> [f64; 3] {
        [
            self.origin[0] + self.x[i] as f64 * self.scale[0],
            self.origin[1] + self.y[i] as f64 * self.scale[1],
            self.origin[2] + self.z[i] as f64 * self.scale[2],
        ]
    }

    /// Build from world-space coordinates, quantising to `scale`.
    pub fn from_world(
        xs: &[f64],
        ys: &[f64],
        zs: &[f64],
        rgb: Vec<[u8; 3]>,
        intensity: Vec<u16>,
        classification: Vec<u8>,
        scale: [f64; 3],
    ) -> Self {
        let n = xs.len();
        let mut bounds = Bounds::empty();
        for i in 0..n {
            bounds.expand([xs[i], ys[i], zs[i]]);
        }
        // Anchor quantisation at the box minimum so every stored value is
        // non-negative and uses the full i32 range from one end.
        let origin = if bounds.is_empty() { [0.0; 3] } else { bounds.min };

        let mut x = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        let mut z = Vec::with_capacity(n);
        for i in 0..n {
            x.push(((xs[i] - origin[0]) / scale[0]).round() as i32);
            y.push(((ys[i] - origin[1]) / scale[1]).round() as i32);
            z.push(((zs[i] - origin[2]) / scale[2]).round() as i32);
        }

        let has_color = !rgb.is_empty();
        let has_intensity = !intensity.is_empty();
        let has_classification = !classification.is_empty();

        Self {
            x, y, z, rgb, intensity, classification,
            origin, scale, bounds,
            has_color, has_intensity, has_classification,
        }
    }
}
