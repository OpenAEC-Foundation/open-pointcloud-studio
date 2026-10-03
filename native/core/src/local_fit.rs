//! Local least-squares geometry shared by the meshers and the shape
//! detection: the symmetric 3x3 eigen decomposition, the accumulated moments
//! of a point set and the plane fitted through them. This file is the single
//! owner of that mathematics.

pub fn difference(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The direction of `v` at unit length, or nothing for a vector too short to
/// have a direction.
pub fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = dot(v, v).sqrt();
    (length > 1e-12).then(|| [v[0] / length, v[1] / length, v[2] / length])
}

/// Eigenvalues and eigenvectors of a symmetric 3x3 matrix by Jacobi
/// rotations. The eigenvalues come in ascending order and `vectors[i]`
/// belongs to `values[i]`; the vectors are orthogonal and of unit length up
/// to rounding. Only the upper triangle is read.
///
/// Rotations stop once every off-diagonal element is below 1e-12 in absolute
/// terms, so a matrix of very small numbers is best scaled up first, as
/// `Moments::plane` does.
pub fn symmetric_eigen3(mut matrix: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    matrix[1][0] = matrix[0][1];
    matrix[2][0] = matrix[0][2];
    matrix[2][1] = matrix[1][2];
    // Columns are the eigenvectors.
    let mut eigenvectors = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..20 {
        let (mut p, mut q) = (0, 1);
        for (a, b) in [(0, 2), (1, 2)] {
            if matrix[a][b].abs() > matrix[p][q].abs() {
                (p, q) = (a, b);
            }
        }
        if matrix[p][q].abs() < 1e-12 {
            break;
        }
        let angle = 0.5 * (2.0 * matrix[p][q]).atan2(matrix[q][q] - matrix[p][p]);
        let (s, c) = angle.sin_cos();
        let app = matrix[p][p];
        let aqq = matrix[q][q];
        let apq = matrix[p][q];
        matrix[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        matrix[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        matrix[p][q] = 0.0;
        matrix[q][p] = 0.0;
        for r in 0..3 {
            if r != p && r != q {
                let arp = matrix[r][p];
                let arq = matrix[r][q];
                matrix[r][p] = c * arp - s * arq;
                matrix[p][r] = matrix[r][p];
                matrix[r][q] = s * arp + c * arq;
                matrix[q][r] = matrix[r][q];
            }
            let vrp = eigenvectors[r][p];
            let vrq = eigenvectors[r][q];
            eigenvectors[r][p] = c * vrp - s * vrq;
            eigenvectors[r][q] = s * vrp + c * vrq;
        }
    }
    // A stable sort keeps the first of equal eigenvalues first.
    let mut order = [0usize, 1, 2];
    order.sort_by(|a, b| matrix[*a][*a].total_cmp(&matrix[*b][*b]));
    (
        order.map(|index| matrix[index][index]),
        order.map(|index| {
            [
                eigenvectors[0][index],
                eigenvectors[1][index],
                eigenvectors[2][index],
            ]
        }),
    )
}

/// How far a neighbourhood is from flat: the smallest eigenvalue's share of
/// the three. Zero on a plane, and at most one third when the points spread
/// equally in all directions. Takes the eigenvalues in ascending order.
pub fn surface_variation(eigenvalues: [f64; 3]) -> f64 {
    let sum = eigenvalues[0] + eigenvalues[1] + eigenvalues[2];
    if sum > 0.0 && sum.is_finite() {
        (eigenvalues[0] / sum).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The plane fitted through a set of points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneFit {
    /// Points the plane was fitted through.
    pub count: u64,
    /// Mean of the points; the plane passes through it.
    pub centroid: [f64; 3],
    /// Unit normal. A fit has no front or back: the normal is given with its
    /// largest component positive until `towards` or `away_from` orients it.
    pub normal: [f64; 3],
    /// Variance of the points along each principal direction, ascending: the
    /// first is across the plane, the other two are in it.
    pub eigenvalues: [f64; 3],
    /// The principal directions that belong to `eigenvalues`: the normal
    /// (before any orientation) and then two directions in the plane.
    pub axes: [[f64; 3]; 3],
}

impl PlaneFit {
    /// Root mean square distance of the fitted points to the plane.
    pub fn rms(&self) -> f64 {
        self.eigenvalues[0].max(0.0).sqrt()
    }

    /// See `surface_variation`.
    pub fn surface_variation(&self) -> f64 {
        surface_variation(self.eigenvalues)
    }

    /// Distance of a point to the plane, positive on the side of the normal.
    pub fn signed_distance(&self, point: [f64; 3]) -> f64 {
        dot(self.normal, difference(point, self.centroid))
    }

    /// The plane is `normal . x = offset`.
    pub fn offset(&self) -> f64 {
        dot(self.normal, self.centroid)
    }

    /// The same plane with the normal on the side of `target`, such as the
    /// scanner station that saw the points.
    pub fn towards(mut self, target: [f64; 3]) -> Self {
        if self.signed_distance(target) < 0.0 {
            self.normal = self.normal.map(|value| -value);
        }
        self
    }

    /// The same plane with the normal on the side away from `target`.
    pub fn away_from(mut self, target: [f64; 3]) -> Self {
        if self.signed_distance(target) > 0.0 {
            self.normal = self.normal.map(|value| -value);
        }
        self
    }
}

/// Count, sum and sum of outer products of a set of points: all that a
/// centroid, a covariance and a plane fit need, in constant memory, and two
/// sets combine by adding theirs.
///
/// The sums are kept relative to an origin near the points, because squares
/// of national grid coordinates leave no digits for millimetres. `new` takes
/// the first point as that origin; `around` fixes it, which makes equal
/// point sets give equal sums whatever point came first.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Moments {
    count: u64,
    origin: Option<[f64; 3]>,
    sum: [f64; 3],
    /// xx, xy, xz, yy, yz, zz of the offsets from the origin.
    outer: [f64; 6],
}

impl Moments {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moments kept relative to `origin`, which should lie near the points.
    pub fn around(origin: [f64; 3]) -> Self {
        Self {
            origin: Some(origin),
            ..Self::default()
        }
    }

    pub fn from_points(points: impl IntoIterator<Item = [f64; 3]>) -> Self {
        let mut moments = Self::new();
        for point in points {
            moments.add(point);
        }
        moments
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn add(&mut self, point: [f64; 3]) {
        let origin = *self.origin.get_or_insert(point);
        let [x, y, z] = difference(point, origin);
        self.count += 1;
        self.sum[0] += x;
        self.sum[1] += y;
        self.sum[2] += z;
        self.outer[0] += x * x;
        self.outer[1] += x * y;
        self.outer[2] += x * z;
        self.outer[3] += y * y;
        self.outer[4] += y * z;
        self.outer[5] += z * z;
    }

    /// Add a point that stands for `weight` points at its position, such as
    /// the mean of the points of a voxel.
    pub fn add_weighted(&mut self, point: [f64; 3], weight: u64) {
        if weight == 0 {
            return;
        }
        let origin = *self.origin.get_or_insert(point);
        let [x, y, z] = difference(point, origin);
        let w = weight as f64;
        self.count += weight;
        self.sum[0] += w * x;
        self.sum[1] += w * y;
        self.sum[2] += w * z;
        self.outer[0] += w * x * x;
        self.outer[1] += w * x * y;
        self.outer[2] += w * x * z;
        self.outer[3] += w * y * y;
        self.outer[4] += w * y * z;
        self.outer[5] += w * z * z;
    }

    /// Add the points of another set. The two may have different origins.
    pub fn merge(&mut self, other: &Self) {
        let Some(theirs) = other.origin.filter(|_| other.count > 0) else {
            return;
        };
        let origin = *self.origin.get_or_insert(theirs);
        // Their offsets are ours plus this shift.
        let d = difference(theirs, origin);
        let n = other.count as f64;
        let s = other.sum;
        self.count += other.count;
        for axis in 0..3 {
            self.sum[axis] += s[axis] + n * d[axis];
        }
        for (slot, (a, b)) in [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)]
            .into_iter()
            .enumerate()
        {
            self.outer[slot] += other.outer[slot] + d[a] * s[b] + s[a] * d[b] + n * d[a] * d[b];
        }
    }

    pub fn centroid(&self) -> Option<[f64; 3]> {
        let origin = self.origin.filter(|_| self.count > 0)?;
        let n = self.count as f64;
        Some(std::array::from_fn(|axis| {
            origin[axis] + self.sum[axis] / n
        }))
    }

    /// Covariance of the points about their centroid (divided by the count).
    pub fn covariance(&self) -> Option<[[f64; 3]; 3]> {
        if self.count == 0 {
            return None;
        }
        let n = self.count as f64;
        let mut matrix = [[0.0; 3]; 3];
        for (slot, (a, b)) in [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)]
            .into_iter()
            .enumerate()
        {
            let value = (self.outer[slot] - self.sum[a] * self.sum[b] / n) / n;
            matrix[a][b] = value;
            matrix[b][a] = value;
        }
        Some(matrix)
    }

    /// The least-squares plane through the points, from three points on.
    /// Points on one line still give a fit; its second eigenvalue is then
    /// zero as well and the normal is any direction across the line. A set
    /// that holds a point with a coordinate that is not finite gives none.
    pub fn plane(&self) -> Option<PlaneFit> {
        if self.count < 3 {
            return None;
        }
        let centroid = self.centroid()?;
        let covariance = self.covariance()?;
        // One point that is no number spoils every sum it went into. Each
        // value is tested: a maximum would pass over the bad ones.
        if !centroid
            .iter()
            .chain(covariance.iter().flatten())
            .all(|value| value.is_finite())
        {
            return None;
        }
        let largest = (0..3)
            .map(|axis| covariance[axis][axis])
            .fold(0.0, f64::max);
        // The eigen routine stops at an absolute threshold, so the matrix is
        // brought near one first. A power of two changes no digit.
        let scale = Some(2f64.powi(-(largest.log2().ceil() as i32)))
            .filter(|scale| largest > 0.0 && scale.is_finite() && *scale > 0.0)
            .unwrap_or(1.0);
        let (values, axes) = symmetric_eigen3(covariance.map(|row| row.map(|value| value * scale)));
        let mut normal = unit(axes[0])?;
        let dominant = (0..3)
            .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
            .unwrap_or(2);
        if normal[dominant] < 0.0 {
            normal = normal.map(|value| -value);
        }
        Some(PlaneFit {
            count: self.count,
            centroid,
            normal,
            // Rounding can leave the smallest one just below zero.
            eigenvalues: values.map(|value| (value / scale).max(0.0)),
            axes,
        })
    }
}

/// The least-squares plane through a set of points.
pub fn fit_plane(points: impl IntoIterator<Item = [f64; 3]>) -> Option<PlaneFit> {
    Moments::from_points(points).plane()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::Rng;

    fn length(v: [f64; 3]) -> f64 {
        dot(v, v).sqrt()
    }

    fn angle_degrees(a: [f64; 3], b: [f64; 3]) -> f64 {
        (dot(a, b).abs() / (length(a) * length(b)))
            .min(1.0)
            .acos()
            .to_degrees()
    }

    #[test]
    fn a_weighted_point_counts_as_that_many_points() {
        let points = [
            [1.0, 2.0, 3.0],
            [2.0, 2.5, 3.5],
            [0.5, 4.0, 1.0],
            [3.0, 1.0, 2.0],
        ];
        let mut repeated = Moments::around([1.0, 1.0, 1.0]);
        let mut weighted = Moments::around([1.0, 1.0, 1.0]);
        for (index, point) in points.iter().enumerate() {
            for _ in 0..=index {
                repeated.add(*point);
            }
            weighted.add_weighted(*point, index as u64 + 1);
        }
        weighted.add_weighted([9.0, 9.0, 9.0], 0);
        assert_eq!(weighted.count(), 10);
        let (a, b) = (repeated.plane().unwrap(), weighted.plane().unwrap());
        for axis in 0..3 {
            assert!((a.centroid[axis] - b.centroid[axis]).abs() < 1e-12);
            assert!((a.normal[axis] - b.normal[axis]).abs() < 1e-9);
            assert!((a.eigenvalues[axis] - b.eigenvalues[axis]).abs() < 1e-12);
        }
        // The first point given becomes the origin when none is set.
        let mut fresh = Moments::new();
        fresh.add_weighted([5.0, 6.0, 7.0], 3);
        assert_eq!(fresh.centroid(), Some([5.0, 6.0, 7.0]));
    }

    #[test]
    fn vector_helpers_follow_the_right_hand_rule() {
        assert_eq!(
            difference([3.0, 2.0, 1.0], [1.0, 1.0, 1.0]),
            [2.0, 1.0, 0.0]
        );
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(cross([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(unit([0.0, 0.0, -4.0]), Some([0.0, 0.0, -1.0]));
        assert_eq!(unit([0.0; 3]), None);
    }

    #[test]
    fn eigen_decomposition_of_a_known_matrix() {
        // Eigenvalues 1, 3 and 5 with the vectors (1,-1,0), (1,1,0), (0,0,1).
        let (values, vectors) =
            symmetric_eigen3([[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]]);
        for (value, expected) in values.iter().zip([1.0, 3.0, 5.0]) {
            assert!((value - expected).abs() < 1e-12, "{values:?}");
        }
        let half = 0.5f64.sqrt();
        assert!(angle_degrees(vectors[0], [half, -half, 0.0]) < 1e-6);
        assert!(angle_degrees(vectors[1], [half, half, 0.0]) < 1e-6);
        assert!(angle_degrees(vectors[2], [0.0, 0.0, 1.0]) < 1e-6);
    }

    #[test]
    fn eigen_pairs_satisfy_the_matrix_for_random_input() {
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let v: [f64; 6] = std::array::from_fn(|_| rng.unit() * 20.0 - 10.0);
            let matrix = [[v[0], v[1], v[2]], [v[1], v[3], v[4]], [v[2], v[4], v[5]]];
            let (values, vectors) = symmetric_eigen3(matrix);
            assert!(values[0] <= values[1] && values[1] <= values[2]);
            let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
            assert!((values.iter().sum::<f64>() - trace).abs() < 1e-9);
            for (value, vector) in values.iter().zip(vectors) {
                assert!((length(vector) - 1.0).abs() < 1e-12);
                for (row, component) in matrix.iter().zip(vector) {
                    assert!((dot(*row, vector) - value * component).abs() < 1e-9);
                }
            }
            for (a, b) in [(0, 1), (0, 2), (1, 2)] {
                assert!(dot(vectors[a], vectors[b]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn eigen_reads_only_the_upper_triangle_and_keeps_the_order_of_equals() {
        let (values, vectors) =
            symmetric_eigen3([[2.0, 1.0, 0.0], [99.0, 2.0, 0.0], [99.0, 99.0, 5.0]]);
        assert!((values[0] - 1.0).abs() < 1e-12 && (values[2] - 5.0).abs() < 1e-12);
        assert!(angle_degrees(vectors[2], [0.0, 0.0, 1.0]) < 1e-6);
        let (values, vectors) =
            symmetric_eigen3([[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]]);
        assert_eq!(values, [4.0; 3]);
        assert_eq!(vectors, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    /// A tilted plane through `origin` with Gaussian noise across it.
    fn noisy_plane(origin: [f64; 3], normal: [f64; 3], sigma: f64, count: usize) -> Vec<[f64; 3]> {
        let normal = unit(normal).unwrap();
        let u = unit(cross(normal, [0.3, -0.2, 0.9])).unwrap();
        let v = cross(normal, u);
        let mut rng = Rng::new(11);
        (0..count)
            .map(|_| {
                let (a, b) = (rng.unit() * 2.0 - 1.0, rng.unit() * 1.5 - 0.75);
                let noise = rng.gaussian() * sigma;
                std::array::from_fn(|axis| {
                    origin[axis] + a * u[axis] + b * v[axis] + noise * normal[axis]
                })
            })
            .collect()
    }

    #[test]
    fn plane_fit_recovers_normal_and_residual_under_noise() {
        let normal = [0.2, -0.5, 0.84];
        let points = noisy_plane([1.0, 2.0, 3.0], normal, 0.003, 20_000);
        let fit = fit_plane(points.iter().copied()).unwrap();
        assert_eq!(fit.count, 20_000);
        assert!(angle_degrees(fit.normal, normal) < 1.0);
        assert!((fit.rms() - 0.003).abs() < 0.0002, "{}", fit.rms());
        assert!(fit.signed_distance([1.0, 2.0, 3.0]).abs() < 0.0005);
        assert!((fit.offset() - dot(fit.normal, fit.centroid)).abs() < 1e-12);
        // The plane is 2 by 1.5 m: uniform spread gives a variance of side^2 / 12.
        assert!((fit.eigenvalues[2] - 4.0 / 12.0).abs() < 0.02);
        assert!((fit.eigenvalues[1] - 2.25 / 12.0).abs() < 0.02);
        assert!(fit.surface_variation() < 1e-4);
        // The axes are the normal and two directions in the plane.
        assert!(angle_degrees(fit.axes[0], fit.normal) < 1e-9);
        assert!(dot(fit.axes[1], fit.normal).abs() < 1e-9);
        assert!(dot(fit.axes[2], fit.normal).abs() < 1e-9);
        assert!(dot(fit.axes[1], fit.axes[2]).abs() < 1e-9);
        // The measured residual is what the points really have.
        let mean_square = points
            .iter()
            .map(|point| fit.signed_distance(*point).powi(2))
            .sum::<f64>()
            / points.len() as f64;
        assert!((mean_square.sqrt() - fit.rms()).abs() < 1e-9);
    }

    #[test]
    fn plane_fit_keeps_millimetres_at_national_grid_coordinates() {
        let origin = [207_000.0, 474_000.0, 10.0];
        let normal = [0.6, 0.0, 0.8];
        let points = noisy_plane(origin, normal, 0.0, 5_000);
        let fit = fit_plane(points.iter().copied()).unwrap();
        assert!(angle_degrees(fit.normal, normal) < 1e-4);
        assert!(fit.rms() < 1e-6, "{}", fit.rms());
        let noisy = noisy_plane(origin, normal, 0.002, 5_000);
        let fit = fit_plane(noisy.iter().copied()).unwrap();
        assert!((fit.rms() - 0.002).abs() < 0.0002, "{}", fit.rms());
    }

    #[test]
    fn surface_variation_tells_a_plane_from_a_corner() {
        let wall = noisy_plane([0.0; 3], [1.0, 0.0, 0.0], 0.001, 4_000);
        assert!(fit_plane(wall.iter().copied()).unwrap().surface_variation() < 0.001);
        // Two faces of 0.2 m meeting at a right angle.
        let mut rng = Rng::new(3);
        let corner: Vec<[f64; 3]> = (0..4_000)
            .map(|index| {
                let (a, b) = (rng.unit() * 0.2, rng.unit() * 0.2);
                if index % 2 == 0 {
                    [a, 0.0, b]
                } else {
                    [0.0, a, b]
                }
            })
            .collect();
        let variation = fit_plane(corner.iter().copied())
            .unwrap()
            .surface_variation();
        assert!(variation > 0.05, "{variation}");
        assert!(variation <= 1.0 / 3.0);
        assert_eq!(surface_variation([0.0; 3]), 0.0);
        assert!((surface_variation([1.0, 1.0, 1.0]) - 1.0 / 3.0).abs() < 1e-12);
    }

    fn assert_same(a: &Moments, b: &Moments) {
        assert_eq!(a.count(), b.count());
        let (ca, cb) = (a.centroid().unwrap(), b.centroid().unwrap());
        let (ma, mb) = (a.covariance().unwrap(), b.covariance().unwrap());
        for axis in 0..3 {
            assert!((ca[axis] - cb[axis]).abs() < 1e-9);
            for other in 0..3 {
                assert!((ma[axis][other] - mb[axis][other]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn merged_moments_equal_adding_every_point_at_once() {
        let points = noisy_plane([207_000.0, 474_000.0, 10.0], [0.1, 0.9, 0.3], 0.004, 3_000);
        let all = Moments::from_points(points.iter().copied());
        let mut merged = Moments::new();
        // Parts with their own origins, one of them fixed far from the rest.
        for (index, part) in points.chunks(700).enumerate() {
            let mut moments = if index == 1 {
                Moments::around([207_010.0, 473_990.0, 0.0])
            } else {
                Moments::new()
            };
            for point in part {
                moments.add(*point);
            }
            merged.merge(&moments);
        }
        assert_same(&all, &merged);
        let (a, b) = (all.plane().unwrap(), merged.plane().unwrap());
        assert!(angle_degrees(a.normal, b.normal) < 1e-6);
        assert!((a.rms() - b.rms()).abs() < 1e-9);

        let before = merged;
        merged.merge(&Moments::new());
        merged.merge(&Moments::around([5.0, 5.0, 5.0]));
        assert_eq!(merged, before);
        let mut empty = Moments::new();
        empty.merge(&all);
        assert_same(&all, &empty);
    }

    #[test]
    fn moments_of_known_points() {
        let moments = Moments::from_points([
            [1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 2.0, 0.0],
        ]);
        assert_eq!(moments.count(), 4);
        assert_eq!(moments.centroid(), Some([2.0, 1.0, 0.0]));
        let covariance = moments.covariance().unwrap();
        assert_eq!(covariance[0], [1.0, 0.0, 0.0]);
        assert_eq!(covariance[1], [0.0, 1.0, 0.0]);
        assert_eq!(covariance[2], [0.0, 0.0, 0.0]);
        let fit = moments.plane().unwrap();
        assert_eq!(fit.normal, [0.0, 0.0, 1.0]);
        assert_eq!(fit.rms(), 0.0);
        assert_eq!(fit.signed_distance([9.0, 9.0, 0.25]), 0.25);
    }

    #[test]
    fn too_few_points_give_no_plane() {
        let mut moments = Moments::new();
        assert!(moments.is_empty());
        assert_eq!(moments.centroid(), None);
        assert_eq!(moments.covariance(), None);
        moments.add([1.0, 2.0, 3.0]);
        moments.add([2.0, 2.0, 3.0]);
        assert_eq!(moments.centroid(), Some([1.5, 2.0, 3.0]));
        assert!(moments.plane().is_none());
        assert!(fit_plane([[0.0; 3]; 2]).is_none());
        // Three points on one line: a fit whose plane is not determined.
        let line = fit_plane([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]).unwrap();
        assert!(line.eigenvalues[1].abs() < 1e-12);
        assert!(dot(line.normal, [1.0, 0.0, 0.0]).abs() < 1e-9);
    }

    #[test]
    fn a_point_that_is_no_number_gives_no_plane() {
        let plane = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]];
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for axis in 0..3 {
                for position in 0..=plane.len() {
                    let mut point = [0.25, 0.75, 0.0];
                    point[axis] = bad;
                    let mut points = plane.to_vec();
                    points.insert(position, point);
                    assert_eq!(
                        fit_plane(points),
                        None,
                        "{bad} on axis {axis} at {position}"
                    );
                }
            }
        }
        // Merged in from another set, it spoils the fit as well.
        let mut moments = Moments::from_points(plane);
        assert!(moments.plane().is_some());
        moments.merge(&Moments::from_points([[f64::NAN, 0.0, 0.0]]));
        assert_eq!(moments.plane(), None);
        // Large but finite coordinates still fit.
        let far = plane.map(|[x, y, z]| [x + 1e9, y - 1e9, z + 1e6]);
        let fit = fit_plane(far).unwrap();
        assert!(angle_degrees(fit.normal, [0.0, 0.0, 1.0]) < 1e-3);
        assert!(fit.rms() < 1e-6);
    }

    #[test]
    fn a_fit_is_oriented_towards_or_away_from_a_position() {
        let floor = fit_plane([
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ])
        .unwrap();
        assert_eq!(floor.normal, [0.0, 0.0, 1.0]);
        assert_eq!(floor.towards([0.5, 0.5, 1.5]).normal, [0.0, 0.0, 1.0]);
        assert_eq!(floor.towards([0.5, 0.5, -1.5]).normal, [0.0, 0.0, -1.0]);
        assert_eq!(floor.away_from([0.5, 0.5, 1.5]).normal, [0.0, 0.0, -1.0]);
        assert_eq!(floor.away_from([0.5, 0.5, -1.5]).normal, [0.0, 0.0, 1.0]);
        let below = floor.towards([0.0, 0.0, -1.0]);
        assert_eq!(below.signed_distance([0.0, 0.0, -0.3]), 0.3);
    }
}
