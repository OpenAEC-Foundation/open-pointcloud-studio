use crate::LoadError;

/// A rigid registration followed by a positive uniform change of units.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneTransform {
    pub rotation: [[f64; 3]; 3],
    pub translation: [f64; 3],
    #[serde(default = "unit_scale")]
    pub scale: f64,
}
fn unit_scale() -> f64 {
    1.0
}

impl Default for SceneTransform {
    fn default() -> Self {
        Self {
            rotation: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            translation: [0.; 3],
            scale: 1.,
        }
    }
}

impl SceneTransform {
    pub fn validate(self) -> Result<Self, LoadError> {
        use crate::local_fit::{cross, dot};
        if !self.scale.is_finite()
            || self.scale <= 0.
            || self.translation.iter().any(|v| !v.is_finite())
            || self.rotation.iter().flatten().any(|v| !v.is_finite())
            || (0..3).any(|i| (dot(self.rotation[i], self.rotation[i]) - 1.).abs() > 1e-6)
            || dot(self.rotation[0], self.rotation[1]).abs() > 1e-6
            || (dot(cross(self.rotation[0], self.rotation[1]), self.rotation[2]) - 1.).abs() > 1e-6
        {
            return Err(LoadError::InvalidData(
                "invalid rigid registration matrix".into(),
            ));
        }
        Ok(self)
    }
    pub fn direction(self, p: [f64; 3]) -> [f64; 3] {
        self.rotation.map(|row| crate::local_fit::dot(row, p))
    }
    pub fn xyz(self, p: [f64; 3]) -> [f64; 3] {
        let r = self.direction(p);
        std::array::from_fn(|i| r[i] * self.scale + self.translation[i])
    }
    pub fn then(self, next: Self) -> Self {
        Self {
            rotation: std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    (0..3)
                        .map(|k| next.rotation[i][k] * self.rotation[k][j])
                        .sum()
                })
            }),
            translation: next.xyz(self.translation),
            scale: self.scale * next.scale,
        }
    }
    pub fn pose(self, pose: Option<e57::Transform>) -> e57::Transform {
        let pose = pose.unwrap_or_default();
        let p = self.xyz([pose.translation.x, pose.translation.y, pose.translation.z]);
        let a = quaternion(self.rotation);
        let b = [
            pose.rotation.w,
            pose.rotation.x,
            pose.rotation.y,
            pose.rotation.z,
        ];
        let q = unit_quaternion([
            a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
            a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
            a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
            a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
        ]);
        e57::Transform {
            rotation: e57::Quaternion {
                w: q[0],
                x: q[1],
                y: q[2],
                z: q[3],
            },
            translation: e57::Translation {
                x: p[0],
                y: p[1],
                z: p[2],
            },
        }
    }
    pub fn matrix(self) -> [[f64; 4]; 4] {
        std::array::from_fn(|i| {
            std::array::from_fn(|j| {
                if i == 3 {
                    if j == 3 {
                        1.
                    } else {
                        0.
                    }
                } else if j == 3 {
                    self.translation[i]
                } else {
                    self.rotation[i][j] * self.scale
                }
            })
        })
    }
}

pub(crate) fn quaternion(r: [[f64; 3]; 3]) -> [f64; 4] {
    let trace = r[0][0] + r[1][1] + r[2][2];
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            s / 4.,
            (r[2][1] - r[1][2]) / s,
            (r[0][2] - r[2][0]) / s,
            (r[1][0] - r[0][1]) / s,
        ]
    } else {
        let i = (0..3).max_by(|&a, &b| r[a][a].total_cmp(&r[b][b])).unwrap();
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let s = (1. + r[i][i] - r[j][j] - r[k][k]).max(0.).sqrt() * 2.;
        let mut q = [0.; 4];
        q[0] = (r[k][j] - r[j][k]) / s;
        q[i + 1] = s / 4.;
        q[j + 1] = (r[j][i] + r[i][j]) / s;
        q[k + 1] = (r[k][i] + r[i][k]) / s;
        q
    };
    unit_quaternion(q)
}

fn unit_quaternion(q: [f64; 4]) -> [f64; 4] {
    let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let sign = if q[0] < 0. { -1. } else { 1. };
    q.map(|v| v * sign / length)
}
