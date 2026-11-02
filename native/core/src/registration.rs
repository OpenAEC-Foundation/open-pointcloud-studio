//! Rigid registration from measured corresponding points in two scans.
//! The result maps moving coordinates into the fixed scan's coordinate frame.

use crate::LoadError;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RigidRegistration {
    pub rotation: [[f64; 3]; 3],
    pub translation: [f64; 3],
    pub rms_error: f64,
    pub max_error: f64,
    pub p95_error: f64,
    pub residuals: Vec<f64>,
    pub correspondences: usize,
}

impl RigidRegistration {
    pub fn xyz(&self, point: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| {
            self.translation[row]
                + (0..3)
                    .map(|col| self.rotation[row][col] * point[col])
                    .sum::<f64>()
        })
    }
}

/// Least-squares Horn alignment, without scale or reflection. Corresponding
/// points are ordered `(moving, fixed)` and must describe a 3D frame rather
/// than one line. Scanner-independent coordinates are accepted in metres.
pub fn register_correspondences(
    pairs: &[([f64; 3], [f64; 3])],
) -> Result<RigidRegistration, LoadError> {
    if pairs.len() < 3 {
        return Err(LoadError::InvalidData(
            "scan registration needs at least three point pairs".into(),
        ));
    }
    if pairs
        .iter()
        .any(|(a, b)| a.iter().chain(b).any(|v| !v.is_finite()))
    {
        return Err(LoadError::InvalidData(
            "scan registration contains non-finite coordinates".into(),
        ));
    }
    let n = pairs.len() as f64;
    let mut mean_a = [0.0; 3];
    let mut mean_b = [0.0; 3];
    for (a, b) in pairs {
        for i in 0..3 {
            mean_a[i] += a[i] / n;
            mean_b[i] += b[i] / n;
        }
    }
    let mut h = [[0.0; 3]; 3];
    let mut spread = 0.0;
    let mut max_cross = 0.0_f64;
    let centered: Vec<_> = pairs
        .iter()
        .map(|(a, b)| {
            let a = std::array::from_fn::<_, 3, _>(|i| a[i] - mean_a[i]);
            let b = std::array::from_fn::<_, 3, _>(|i| b[i] - mean_b[i]);
            spread += a.iter().map(|v| v * v).sum::<f64>();
            (a, b)
        })
        .collect();
    let anchor = centered
        .iter()
        .map(|(a, _)| *a)
        .find(|a| a.iter().map(|v| v * v).sum::<f64>() > f64::EPSILON)
        .unwrap_or([0.0; 3]);
    let anchor_squared = anchor.iter().map(|v| v * v).sum::<f64>();
    let mut largest_squared = 0.0_f64;
    for (a, _) in &centered {
        largest_squared = largest_squared.max(a.iter().map(|v| v * v).sum::<f64>());
        let cross = [
            anchor[1] * a[2] - anchor[2] * a[1],
            anchor[2] * a[0] - anchor[0] * a[2],
            anchor[0] * a[1] - anchor[1] * a[0],
        ];
        max_cross = max_cross.max(cross.iter().map(|v| v * v).sum::<f64>());
    }
    if spread <= f64::EPSILON || max_cross <= anchor_squared * largest_squared * 1e-12 {
        return Err(LoadError::InvalidData(
            "scan registration points are coincident or collinear".into(),
        ));
    }
    for (a, b) in &centered {
        for row in 0..3 {
            for col in 0..3 {
                h[row][col] += a[row] * b[col];
            }
        }
    }
    let [sxx, sxy, sxz] = h[0];
    let [syx, syy, syz] = h[1];
    let [szx, szy, szz] = h[2];
    let mut k = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let mut eigenvectors = [[0.0; 4]; 4];
    for i in 0..4 {
        eigenvectors[i][i] = 1.0;
    }
    // Symmetric Jacobi diagonalisation avoids power iteration's ambiguity
    // when the leading eigenvalue has a negative counterpart of equal size.
    for _ in 0..64 {
        let mut p = 0;
        let mut q = 1;
        for i in 0..4 {
            for j in i + 1..4 {
                if k[i][j].abs() > k[p][q].abs() {
                    p = i;
                    q = j;
                }
            }
        }
        if k[p][q].abs() < 1e-14 * spread.max(1.0) {
            break;
        }
        let theta = 0.5 * (2.0 * k[p][q]).atan2(k[q][q] - k[p][p]);
        let (s, c) = theta.sin_cos();
        let app = k[p][p];
        let aqq = k[q][q];
        let apq = k[p][q];
        k[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        k[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        k[p][q] = 0.0;
        k[q][p] = 0.0;
        for i in 0..4 {
            if i != p && i != q {
                let kip = k[i][p];
                let kiq = k[i][q];
                k[i][p] = c * kip - s * kiq;
                k[p][i] = k[i][p];
                k[i][q] = s * kip + c * kiq;
                k[q][i] = k[i][q];
            }
            let vip = eigenvectors[i][p];
            let viq = eigenvectors[i][q];
            eigenvectors[i][p] = c * vip - s * viq;
            eigenvectors[i][q] = s * vip + c * viq;
        }
    }
    let best = (0..4).max_by(|&a, &b| k[a][a].total_cmp(&k[b][b])).unwrap();
    let [w, x, y, z] = std::array::from_fn(|i| eigenvectors[i][best]);
    let rotation = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let translation = std::array::from_fn(|row| {
        mean_b[row]
            - (0..3)
                .map(|col| rotation[row][col] * mean_a[col])
                .sum::<f64>()
    });
    let mut result = RigidRegistration {
        rotation,
        translation,
        rms_error: 0.0,
        max_error: 0.0,
        p95_error: 0.0,
        residuals: Vec::with_capacity(pairs.len()),
        correspondences: pairs.len(),
    };
    let mut sum_squared = 0.0;
    for (a, b) in pairs {
        let mapped = result.xyz(*a);
        let distance = (0..3)
            .map(|i| (mapped[i] - b[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        sum_squared += distance * distance;
        result.max_error = result.max_error.max(distance);
        result.residuals.push(distance);
    }
    result.rms_error = (sum_squared / n).sqrt();
    let mut ordered = result.residuals.clone();
    ordered.sort_by(f64::total_cmp);
    result.p95_error = ordered[((ordered.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_rotation_and_translation() {
        let source = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 4.0],
        ];
        let pairs: Vec<_> = source
            .into_iter()
            .map(|p| (p, [10.0 - p[1], -2.0 + p[0], 5.0 + p[2]]))
            .collect();
        let fitted = register_correspondences(&pairs).unwrap();
        assert!(fitted.rms_error < 1e-10, "{fitted:?}");
        assert!(fitted.max_error < 1e-10);
        assert!(fitted.p95_error < 1e-10);
        assert_eq!(fitted.residuals.len(), pairs.len());
        for (a, b) in pairs {
            let got = fitted.xyz(a);
            assert!((0..3).all(|i| (got[i] - b[i]).abs() < 1e-10));
        }
    }

    #[test]
    fn rejects_collinear_pairs() {
        let pairs = [
            ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            ([2.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
        ];
        assert!(register_correspondences(&pairs).is_err());
    }
}
