//! Bounded point-to-plane ICP with a balanced k-d tree and trimmed residuals.
use crate::{
    local_fit::{cross, difference, dot, Moments},
    scene_transform::SceneTransform,
    LoadError,
};
use rayon::prelude::*;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IcpOptions {
    pub iterations: usize,
    pub max_distance: f64,
    pub trim_fraction: f64,
    pub tolerance: f64,
}
impl Default for IcpOptions {
    fn default() -> Self {
        Self {
            iterations: 40,
            max_distance: 0.5,
            trim_fraction: 0.8,
            tolerance: 0.00001,
        }
    }
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct IcpResult {
    pub transform: SceneTransform,
    pub rms: f64,
    pub p95: f64,
    pub matched: usize,
    pub iterations: usize,
    pub converged: bool,
    /// Signed point-to-plane deviations in moving sample order; missing
    /// overlap is null. These drive the deviation colours of the preview.
    pub deviations: Vec<Option<f64>>,
}
struct Node {
    point: usize,
    left: Option<usize>,
    right: Option<usize>,
    axis: usize,
}
struct Tree<'a> {
    points: &'a [[f64; 3]],
    nodes: Vec<Node>,
    root: Option<usize>,
}
impl<'a> Tree<'a> {
    fn new(points: &'a [[f64; 3]]) -> Self {
        fn build(ids: &mut [usize], depth: usize, tree: &mut Tree<'_>) -> Option<usize> {
            if ids.is_empty() {
                return None;
            }
            let axis = depth % 3;
            let mid = ids.len() / 2;
            ids.select_nth_unstable_by(mid, |&a, &b| {
                tree.points[a][axis].total_cmp(&tree.points[b][axis])
            });
            let index = tree.nodes.len();
            tree.nodes.push(Node {
                point: ids[mid],
                left: None,
                right: None,
                axis,
            });
            let (left, right) = ids.split_at_mut(mid);
            tree.nodes[index].left = build(left, depth + 1, tree);
            tree.nodes[index].right = build(&mut right[1..], depth + 1, tree);
            Some(index)
        }
        let mut tree = Self {
            points,
            nodes: Vec::with_capacity(points.len()),
            root: None,
        };
        tree.root = build(&mut (0..points.len()).collect::<Vec<_>>(), 0, &mut tree);
        tree
    }
    fn nearest(&self, p: [f64; 3], count: usize, max_squared: f64) -> Vec<(f64, usize)> {
        fn visit(
            tree: &Tree<'_>,
            node: Option<usize>,
            p: [f64; 3],
            count: usize,
            max_squared: f64,
            found: &mut Vec<(f64, usize)>,
        ) {
            let Some(node) = node else {
                return;
            };
            let node = &tree.nodes[node];
            let target = tree.points[node.point];
            let d = difference(p, target);
            let squared = dot(d, d);
            if squared <= max_squared {
                let at = found.partition_point(|(old, _)| *old < squared);
                if at < count {
                    found.insert(at, (squared, node.point));
                    found.truncate(count);
                }
            }
            let delta = p[node.axis] - target[node.axis];
            let (near, far) = if delta < 0. {
                (node.left, node.right)
            } else {
                (node.right, node.left)
            };
            visit(tree, near, p, count, max_squared, found);
            let limit = if found.len() == count {
                found.last().unwrap().0
            } else {
                max_squared
            };
            if delta * delta <= limit {
                visit(tree, far, p, count, max_squared, found);
            }
        }
        let mut found = Vec::with_capacity(count + 1);
        visit(self, self.root, p, count, max_squared, &mut found);
        found
    }
}

fn solve(mut a: [[f64; 6]; 6], mut b: [f64; 6]) -> Result<[f64; 6], LoadError> {
    let largest = (0..6).map(|i| a[i][i].abs()).fold(0., f64::max);
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += largest * 1e-9;
    }
    for col in 0..6 {
        let pivot = (col..6)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap();
        if a[pivot][col].abs() < largest * 1e-12 || largest == 0. {
            return Err(LoadError::InvalidData(
                "ICP overlap does not constrain a 3D transform".into(),
            ));
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let divisor = a[col][col];
        for value in a[col].iter_mut().skip(col) {
            *value /= divisor;
        }
        b[col] /= divisor;
        let pivot_row = a[col];
        for (i, row) in a.iter_mut().enumerate() {
            if i != col {
                let factor = row[col];
                for (value, pivot) in row.iter_mut().zip(pivot_row).skip(col) {
                    *value -= factor * pivot;
                }
                b[i] -= factor * b[col];
            }
        }
    }
    Ok(b)
}

pub fn refine_point_to_plane(
    fixed: &[[f64; 3]],
    moving: &[[f64; 3]],
    initial: SceneTransform,
    options: IcpOptions,
    progress: &mut dyn FnMut(usize, f64) -> Result<(), LoadError>,
) -> Result<IcpResult, LoadError> {
    initial.validate()?;
    if fixed.len() < 12
        || moving.len() < 12
        || fixed.len() > 500_000
        || moving.len() > 500_000
        || fixed.iter().chain(moving).flatten().any(|v| !v.is_finite())
        || !(1..=200).contains(&options.iterations)
        || !options.max_distance.is_finite()
        || options.max_distance <= 0.
        || !(0.2..=1.).contains(&options.trim_fraction)
        || !options.tolerance.is_finite()
        || options.tolerance <= 0.
    {
        return Err(LoadError::InvalidData(
            "invalid ICP samples or settings (12–500000 finite points per scan)".into(),
        ));
    }
    let tree = Tree::new(fixed);
    let normals: Vec<_> = fixed
        .par_iter()
        .map(|&p| {
            let neighbours = tree.nearest(p, 16, f64::INFINITY);
            let fit = Moments::from_points(neighbours.iter().map(|(_, i)| fixed[*i])).plane()?;
            (fit.surface_variation() < 0.12).then_some(fit.normal)
        })
        .collect();
    let origin = Moments::from_points(fixed.iter().copied())
        .plane()
        .map(|p| p.centroid)
        .unwrap_or(fixed[0]);
    let maximum = options.max_distance * options.max_distance;
    let mut transform = initial;
    let mut previous = f64::INFINITY;
    let mut converged = false;
    let mut iterations = 0;
    for iteration in 0..options.iterations {
        progress(iteration, if previous.is_finite() { previous } else { 0. })?;
        let mut matches: Vec<_> = moving
            .par_iter()
            .filter_map(|&point| {
                let p = transform.xyz(point);
                let (_, index) = tree.nearest(p, 1, maximum).first().copied()?;
                let n = normals[index]?;
                let residual = dot(n, difference(p, fixed[index]));
                Some((residual, p, n))
            })
            .collect();
        if matches.len() < 12 {
            return Err(LoadError::InvalidData("ICP found fewer than twelve overlapping surface points; increase overlap distance or align point pairs first".into()));
        }
        matches.sort_by(|a, b| a.0.abs().total_cmp(&b.0.abs()));
        matches.truncate(((matches.len() as f64 * options.trim_fraction).ceil() as usize).max(12));
        let mut a = [[0.; 6]; 6];
        let mut b = [0.; 6];
        let mut squared = 0.;
        for &(residual, p, n) in &matches {
            let r = cross(difference(p, origin), n);
            let row = [r[0], r[1], r[2], n[0], n[1], n[2]];
            squared += residual * residual;
            for i in 0..6 {
                b[i] -= row[i] * residual;
                for j in 0..6 {
                    a[i][j] += row[i] * row[j];
                }
            }
        }
        let normal_matrix = std::array::from_fn(|i| std::array::from_fn(|j| a[i + 3][j + 3]));
        let (eigenvalues, _) = crate::local_fit::symmetric_eigen3(normal_matrix);
        if eigenvalues[0] < eigenvalues[2] * 1e-6 {
            return Err(LoadError::InvalidData("ICP overlap does not constrain all three directions; include surfaces with different orientations".into()));
        }
        let rms = (squared / matches.len() as f64).sqrt();
        let delta = solve(a, b)?;
        let angle = (delta[..3].iter().map(|v| v * v).sum::<f64>()).sqrt();
        if angle > 0.3 {
            return Err(LoadError::InvalidData(
                "ICP rotation step is too large; improve coarse alignment".into(),
            ));
        }
        let rotation = if angle < 1e-14 {
            SceneTransform::default().rotation
        } else {
            let axis = [delta[0] / angle, delta[1] / angle, delta[2] / angle];
            let (s, c) = angle.sin_cos();
            let skew = [
                [0., -axis[2], axis[1]],
                [axis[2], 0., -axis[0]],
                [-axis[1], axis[0], 0.],
            ];
            std::array::from_fn(|i| {
                std::array::from_fn(
                    |j| if i == j { c } else { 0. } + (1. - c) * axis[i] * axis[j] + s * skew[i][j],
                )
            })
        };
        let mut step = SceneTransform {
            rotation,
            translation: [0.; 3],
            scale: 1.,
        };
        let turned = step.xyz(origin);
        step.translation = std::array::from_fn(|i| origin[i] - turned[i] + delta[i + 3]);
        transform = transform.then(step);
        iterations = iteration + 1;
        let moved = delta[3..].iter().map(|v| v * v).sum::<f64>().sqrt();
        if (previous - rms).abs() < options.tolerance && angle < 1e-5 && moved < options.tolerance {
            converged = true;
            break;
        }
        previous = rms;
    }
    let deviations: Vec<_> = moving
        .par_iter()
        .map(|&point| {
            let p = transform.xyz(point);
            let (_, index) = tree.nearest(p, 1, maximum).first().copied()?;
            Some(dot(normals[index]?, difference(p, fixed[index])))
        })
        .collect();
    let mut errors: Vec<_> = deviations.iter().flatten().map(|v| v.abs()).collect();
    if errors.is_empty() {
        return Err(LoadError::InvalidData("ICP has no overlap".into()));
    }
    errors.sort_by(f64::total_cmp);
    let matched = errors.len();
    let rms = (errors.iter().map(|v| v * v).sum::<f64>() / matched as f64).sqrt();
    let p95 = errors[(matched as f64 * 0.95).ceil() as usize - 1];
    progress(iterations, rms)?;
    Ok(IcpResult {
        transform,
        rms,
        p95,
        matched,
        iterations,
        converged,
        deviations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shifted_three_sided_room_recovers_translation() {
        let mut fixed = Vec::new();
        for i in 0..21 {
            for j in 0..21 {
                let a = i as f64 * 0.1;
                let b = j as f64 * 0.1;
                fixed.extend([[a, b, 0.], [a, 0., b], [0., a, b]]);
            }
        }
        let moving: Vec<_> = fixed
            .iter()
            .map(|p| [p[0] + 0.035, p[1] - 0.025, p[2] + 0.045])
            .collect();
        let result = refine_point_to_plane(
            &fixed,
            &moving,
            SceneTransform::default(),
            IcpOptions::default(),
            &mut |_, _| Ok(()),
        )
        .unwrap();
        assert!(result.rms < 0.0001, "{result:?}");
        assert!((result.transform.translation[0] + 0.035).abs() < 0.0001);
        assert!((result.transform.translation[1] - 0.025).abs() < 0.0001);
        assert!((result.transform.translation[2] + 0.045).abs() < 0.0001);
    }
}
