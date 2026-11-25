//! Balanced coordinate index shared by registration and texture sampling.
use crate::local_fit::{difference, dot};

struct Node {
    point: usize,
    left: Option<usize>,
    right: Option<usize>,
    axis: usize,
}
pub(crate) struct PointIndex {
    nodes: Vec<Node>,
    root: Option<usize>,
}
impl PointIndex {
    pub(crate) fn new(points: &[[f64; 3]]) -> Self {
        fn build(
            ids: &mut [usize],
            depth: usize,
            tree: &mut PointIndex,
            points: &[[f64; 3]],
        ) -> Option<usize> {
            if ids.is_empty() {
                return None;
            }
            let axis = depth % 3;
            let mid = ids.len() / 2;
            ids.select_nth_unstable_by(mid, |&a, &b| points[a][axis].total_cmp(&points[b][axis]));
            let index = tree.nodes.len();
            tree.nodes.push(Node {
                point: ids[mid],
                left: None,
                right: None,
                axis,
            });
            let (left, right) = ids.split_at_mut(mid);
            tree.nodes[index].left = build(left, depth + 1, tree, points);
            tree.nodes[index].right = build(&mut right[1..], depth + 1, tree, points);
            Some(index)
        }
        let mut tree = Self {
            nodes: Vec::with_capacity(points.len()),
            root: None,
        };
        tree.root = build(
            &mut (0..points.len()).collect::<Vec<_>>(),
            0,
            &mut tree,
            points,
        );
        tree
    }
    pub(crate) fn nearest_into(
        &self,
        points: &[[f64; 3]],
        p: [f64; 3],
        count: usize,
        max_squared: f64,
        found: &mut Vec<(f64, usize)>,
    ) {
        fn visit(
            tree: &PointIndex,
            points: &[[f64; 3]],
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
            let target = points[node.point];
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
            visit(tree, points, near, p, count, max_squared, found);
            let limit = if found.len() == count {
                found.last().unwrap().0
            } else {
                max_squared
            };
            if delta * delta <= limit {
                visit(tree, points, far, p, count, max_squared, found);
            }
        }
        found.clear();
        if count > 0 {
            visit(self, points, self.root, p, count, max_squared, found);
        }
    }
}
