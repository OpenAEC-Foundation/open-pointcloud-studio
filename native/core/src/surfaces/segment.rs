//! Planes in the working set: a normal and a flatness per working point,
//! regions grown from the flattest points over neighbouring voxels, a robust
//! fit per region, merging of regions that turn out to be one plane, and the
//! side each plane is seen from.

use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;

use super::cylinder::{Cylinder, Found};
use super::voxel_cloud::{VoxelCloud, NO_STATION};
use super::{NormalSource, SurfaceDetectConfig, MIN_EDGE_ANGLE_DEG};
use crate::local_fit::{difference, dot, unit, Moments, PlaneFit};
use crate::LoadError;

/// Fewer neighbours than this give no normal.
const MIN_NEIGHBOURS: usize = 6;
/// Working points handled between two progress reports.
const BLOCK: usize = 16_384;
/// Label of a working point that belongs to no plane. Plane `n` of the list
/// has label `n + 1`.
pub(crate) const NO_LABEL: u32 = 0;
/// Rounds in which points left over beside a plane join it. Normals within
/// two voxels of an edge are mixed, so that band is not reached by growing.
const JOIN_ROUNDS: usize = 3;
/// Rounds in which the points along the line between two faces at a small
/// angle move to the face they lie nearer to and the planes are fitted
/// again.
const SETTLE_ROUNDS: usize = 4;
/// Planes a raw point is tested against: those of its voxel and of the
/// voxels around it. Three meet in a corner; one more for what stands there.
pub(crate) const CANDIDATES: usize = 4;

/// The 26 voxels around a voxel. The first 13 are one of each opposite pair.
pub(crate) const AROUND: [[i32; 3]; 26] = {
    let mut offsets = [[0; 3]; 26];
    let mut forward = 0;
    let mut backward = 13;
    let mut index: i32 = 0;
    while index < 27 {
        let offset = [index % 3 - 1, index / 3 % 3 - 1, index / 9 - 1];
        // The half that comes after the centre in this count is "forward".
        if index > 13 {
            offsets[forward] = offset;
            forward += 1;
        } else if index < 13 {
            offsets[backward] = offset;
            backward += 1;
        }
        index += 1;
    }
    offsets
};

pub(crate) fn shifted(cell: [i32; 3], offset: [i32; 3]) -> [i32; 3] {
    [
        cell[0] + offset[0],
        cell[1] + offset[1],
        cell[2] + offset[2],
    ]
}

/// Per working point the unit normal of the surface around it, without a
/// side, and how far from flat that surface is (see `surface_variation`).
/// A point with too few neighbours has no normal and an infinite variation.
pub(crate) struct Normals {
    pub(crate) normal: Vec<[f32; 3]>,
    pub(crate) variation: Vec<f32>,
}

fn point_normal(cloud: &VoxelCloud, point: u32, slab: f64) -> ([f32; 3], f32) {
    const NONE: ([f32; 3], f32) = ([0.0; 3], f32::INFINITY);
    let centre = cloud.position(point);
    let cell = cloud.cell(point);
    let mut near = [[0.0f64; 3]; 125];
    let mut count = 0;
    for dz in -2..=2 {
        for dy in -2..=2 {
            for dx in -2..=2 {
                if let Some(other) = cloud.find(shifted(cell, [dx, dy, dz])) {
                    near[count] = cloud.position(other);
                    count += 1;
                }
            }
        }
    }
    if count < MIN_NEIGHBOURS {
        return NONE;
    }
    let mut all = Moments::around(centre);
    for position in &near[..count] {
        all.add(*position);
    }
    let Some(first) = all.plane() else {
        return NONE;
    };
    // A second surface within the neighbourhood, such as the other face of a
    // step of a few centimetres, tilts nothing but makes the fit thick. Only
    // what lies in a slab about the point itself counts.
    let mut kept = Moments::around(centre);
    for position in &near[..count] {
        if dot(first.normal, difference(*position, centre)).abs() <= slab {
            kept.add(*position);
        }
    }
    let fit = if kept.count() == count as u64 {
        first
    } else if kept.count() < MIN_NEIGHBOURS as u64 {
        return NONE;
    } else {
        match kept.plane() {
            Some(fit) => fit,
            None => return NONE,
        }
    };
    (
        fit.normal.map(|value| value as f32),
        fit.surface_variation() as f32,
    )
}

/// Normals from the voxels within two of each point. `step` receives the
/// number of points done and may stop the work.
pub(crate) fn estimate_normals(
    cloud: &VoxelCloud,
    slab: f64,
    step: &mut dyn FnMut(u64) -> Result<(), LoadError>,
) -> Result<Normals, LoadError> {
    let total = cloud.len();
    let mut normals = Normals {
        normal: Vec::with_capacity(total),
        variation: Vec::with_capacity(total),
    };
    for start in (0..total).step_by(BLOCK) {
        step(start as u64)?;
        let end = (start + BLOCK).min(total);
        let block: Vec<_> = (start as u32..end as u32)
            .into_par_iter()
            .map(|point| point_normal(cloud, point, slab))
            .collect();
        for (normal, variation) in block {
            normals.normal.push(normal);
            normals.variation.push(variation);
        }
    }
    step(total as u64)?;
    Ok(normals)
}

/// The label of every working point and the points of every region.
pub(crate) struct Grown {
    pub(crate) labels: Vec<u32>,
    pub(crate) regions: Vec<Vec<u32>>,
}

/// Grow regions from the flattest points. A neighbouring voxel joins when
/// its normal and its position agree with the plane of the region so far.
/// That plane, and not the neighbour it is reached from, is the measure, so
/// a region does not follow a surface round a bend. Of more regions than
/// `max_regions` the largest are kept. `step` receives the number of seeds
/// tried, of `total`, and may stop the work.
pub(crate) fn grow(
    cloud: &VoxelCloud,
    normals: &Normals,
    config: &SurfaceDetectConfig,
    max_regions: usize,
    step: &mut dyn FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<Grown, LoadError> {
    let total = cloud.len();
    let cos_angle = config.angle_tolerance_deg.to_radians().cos();
    let mut seeds: Vec<u32> = (0..total as u32)
        .filter(|point| f64::from(normals.variation[*point as usize]) <= config.curvature_max)
        .collect();
    seeds.sort_by(|a, b| {
        normals.variation[*a as usize]
            .total_cmp(&normals.variation[*b as usize])
            .then(a.cmp(b))
    });
    let mut labels = vec![NO_LABEL; total];
    // Points of a region that was too small start no region of their own.
    let mut spent = vec![false; total];
    let mut regions: Vec<Vec<u32>> = Vec::new();
    let voxel_area = cloud.voxel() * cloud.voxel();
    let min_width = config.min_plane_width.max(3.0 * cloud.voxel());
    for (tried, seed) in seeds.iter().enumerate() {
        if tried % 1_024 == 0 {
            step(tried as u64, seeds.len() as u64)?;
        }
        if labels[*seed as usize] != NO_LABEL || spent[*seed as usize] {
            continue;
        }
        let label = regions.len() as u32 + 1;
        let start = cloud.position(*seed);
        let mut members = vec![*seed];
        labels[*seed as usize] = label;
        let mut moments = Moments::around(start);
        moments.add(start);
        let mut normal = normals.normal[*seed as usize].map(f64::from);
        let mut centre = start;
        let mut refit_at = 8;
        let mut head = 0;
        // The list of members is also the queue of the walk.
        while head < members.len() {
            if head % 65_536 == 65_535 {
                step(tried as u64, seeds.len() as u64)?;
            }
            let cell = cloud.cell(members[head]);
            head += 1;
            for offset in AROUND {
                let Some(other) = cloud.find(shifted(cell, offset)) else {
                    continue;
                };
                let slot = other as usize;
                if labels[slot] != NO_LABEL || !normals.variation[slot].is_finite() {
                    continue;
                }
                if dot(normals.normal[slot].map(f64::from), normal).abs() < cos_angle {
                    continue;
                }
                let position = cloud.position(other);
                if dot(difference(position, centre), normal).abs() > config.distance_tolerance {
                    continue;
                }
                labels[slot] = label;
                members.push(other);
                moments.add(position);
                if members.len() >= refit_at {
                    if let Some(fit) = moments.plane() {
                        normal = fit.normal;
                        centre = fit.centroid;
                    }
                    refit_at *= 2;
                }
            }
        }
        // A plane that lies on a voxel boundary fills two layers, so the
        // count overstates the area; the outline decides in the end. The
        // width is that of the voxel means, so it is asked to span three
        // voxels as well: the means of the voxels along a corner lie on the
        // diagonal between its two faces, and a row or two of them is no
        // face however large the voxels are.
        let wide_enough = moments.plane().is_some_and(|fit| {
            members.len() as f64 * voxel_area >= config.min_region_area
                && (12.0 * fit.eigenvalues[1]).sqrt() >= min_width
                && fit.rms() <= config.distance_tolerance
        });
        if wide_enough {
            regions.push(members);
        } else {
            for member in members {
                labels[member as usize] = NO_LABEL;
                spent[member as usize] = true;
            }
        }
    }
    step(seeds.len() as u64, seeds.len() as u64)?;
    if regions.len() > max_regions {
        // Keep the largest; the labels follow below.
        let mut order: Vec<usize> = (0..regions.len()).collect();
        order.sort_by(|a, b| regions[*b].len().cmp(&regions[*a].len()).then(a.cmp(b)));
        order.truncate(max_regions);
        order.sort_unstable();
        let mut kept = Vec::with_capacity(max_regions);
        for index in order {
            kept.push(std::mem::take(&mut regions[index]));
        }
        regions = kept;
        labels.fill(NO_LABEL);
        for (index, members) in regions.iter().enumerate() {
            for member in members {
                labels[*member as usize] = index as u32 + 1;
            }
        }
    }
    Ok(Grown { labels, regions })
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    *values.select_nth_unstable_by(middle, f64::total_cmp).1
}

/// The plane through the points of a region, fitted a second time without
/// the points that lie far from the first fit: what stands against a wall
/// or lies in the corner with the next one. Every working point counts for
/// the scan points it is the mean of, so the fit is that of the scan points
/// themselves: a plane on a voxel boundary fills two layers of voxels that
/// need not hold equally many.
fn robust_moments(cloud: &VoxelCloud, members: &[u32], tolerance: f64) -> Moments {
    let positions: Vec<[f64; 3]> = members.iter().map(|point| cloud.position(*point)).collect();
    // Which points lie far is judged from a fit in which every voxel counts
    // once: a voxel in a corner holds the points of two faces, and with
    // their number it would pull the plane it is measured against.
    let mut all = Moments::around(positions[0]);
    for position in &positions {
        all.add(*position);
    }
    let Some(first) = all.plane() else {
        return all;
    };
    let distances: Vec<f64> = positions
        .iter()
        .map(|position| first.signed_distance(*position))
        .collect();
    let middle = median(&mut distances.clone());
    let spread = median(
        &mut distances
            .iter()
            .map(|distance| (distance - middle).abs())
            .collect::<Vec<_>>(),
    );
    // 2.5 standard deviations, with the median deviation as the measure of
    // one, but never less than half the tolerance. A plane on a voxel
    // boundary has its means in two layers, one to each side of it; in a
    // dense scan each layer is so even that the other would count as far.
    let limit = (2.5 * 1.4826 * spread).max(0.5 * tolerance);
    let mut kept = Moments::around(positions[0]);
    for ((position, distance), member) in positions.iter().zip(&distances).zip(members) {
        if (distance - middle).abs() <= limit {
            kept.add_weighted(*position, cloud.weight(*member));
        }
    }
    if kept.plane().is_some() {
        kept
    } else {
        all
    }
}

fn find_root(parents: &mut [u32], mut node: u32) -> u32 {
    while parents[node as usize] != node {
        parents[node as usize] = parents[parents[node as usize] as usize];
        node = parents[node as usize];
    }
    node
}

fn join(parents: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find_root(parents, a), find_root(parents, b));
    // The lower number stays the root, so the numbering does not depend on
    // the order of the joins.
    parents[a.max(b) as usize] = a.min(b);
}

fn angle_cosine(a: &PlaneFit, b: &PlaneFit) -> f64 {
    dot(a.normal, b.normal).abs()
}

/// The planes and cylinders of the working set and what every working
/// point has to do with them.
pub(crate) struct Segmentation {
    /// Per working point the label of its plane or cylinder, or `NO_LABEL`.
    /// The planes have the labels from 1, the cylinders those after them.
    pub(crate) labels: Vec<u32>,
    /// Planes in the coordinates of the working set, with the normal on the
    /// side the plane is seen from.
    pub(crate) planes: Vec<PlaneFit>,
    pub(crate) normal_sources: Vec<NormalSource>,
    /// Number shared by planes that continue each other across a gap.
    pub(crate) coplanar_groups: Vec<u32>,
    /// The cylinders, in the coordinates of the working set.
    pub(crate) cylinders: Vec<Cylinder>,
    /// Per cylinder whether its points face its axis: the inside of a
    /// round shaft. Where no station tells, it is taken as a column.
    pub(crate) hollow: Vec<bool>,
    /// Per working point the labels of the planes and cylinders a raw point
    /// of its voxel can belong to, nearest first, filled up with `NO_LABEL`.
    /// Filled by `link`.
    pub(crate) candidates: Vec<[u32; CANDIDATES]>,
    /// Pairs of labels of planes that touch, lower label first. Filled by
    /// `link`.
    pub(crate) pairs: Vec<(u32, u32)>,
}

impl Segmentation {
    /// Distance of a position to the plane or cylinder with a label.
    fn apart(&self, label: u32, position: [f64; 3]) -> f64 {
        let index = label as usize - 1;
        match self.planes.get(index) {
            Some(plane) => plane.signed_distance(position).abs(),
            None => self.cylinders[index - self.planes.len()]
                .distance(position)
                .abs(),
        }
    }

    /// Take cylinders in: their working points get their labels, and the
    /// stations of those points tell which side each was scanned from.
    pub(crate) fn add_cylinders(
        &mut self,
        cloud: &VoxelCloud,
        found: Vec<Found>,
        sides: &Sides<'_>,
    ) {
        for Found { cylinder, members } in found {
            let label = (self.planes.len() + self.cylinders.len()) as u32 + 1;
            // How squarely the points face their stations, outward counted
            // as positive.
            let mut facing = 0.0;
            for member in &members {
                self.labels[*member as usize] = label;
                let position = cloud.position(*member);
                let Some(station) = sides.stations.get(cloud.station(*member) as usize) else {
                    continue;
                };
                let (_, out) = cylinder.split(position);
                if let (Some(out), Some(towards)) =
                    (unit(out), unit(difference(*station, position)))
                {
                    facing += dot(out, towards);
                }
            }
            self.cylinders.push(cylinder);
            self.hollow.push(facing < 0.0);
        }
    }

    /// Find for every working point the planes and cylinders that a raw
    /// point of its voxel can belong to: its own and those of the voxels
    /// round it. Planes that share a voxel or lie in neighbouring ones
    /// touch.
    pub(crate) fn link(&mut self, cloud: &VoxelCloud) {
        let labels = &self.labels;
        let candidates: Vec<[u32; CANDIDATES]> = (0..cloud.len() as u32)
            .into_par_iter()
            .map(|point| {
                let position = cloud.position(point);
                let cell = cloud.cell(point);
                let mut found: Vec<(f64, u32)> = Vec::new();
                let mut note = |label: u32| {
                    if label != NO_LABEL && !found.iter().any(|(_, known)| *known == label) {
                        found.push((self.apart(label, position), label));
                    }
                };
                note(labels[point as usize]);
                for offset in AROUND {
                    if let Some(other) = cloud.find(shifted(cell, offset)) {
                        note(labels[other as usize]);
                    }
                }
                found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let mut list = [NO_LABEL; CANDIDATES];
                for (slot, (_, label)) in list.iter_mut().zip(found) {
                    *slot = label;
                }
                list
            })
            .collect();
        let planes = self.planes.len() as u32;
        let mut touching = BTreeSet::new();
        for list in &candidates {
            for (index, a) in list.iter().enumerate() {
                for b in &list[index + 1..] {
                    if (1..=planes).contains(a) && (1..=planes).contains(b) {
                        touching.insert((*a.min(b), *a.max(b)));
                    }
                }
            }
        }
        self.candidates = candidates;
        self.pairs = touching.into_iter().collect();
    }
}

/// What the orientation of the planes can go by.
pub(crate) struct Sides<'a> {
    /// Every station of the sources, in the coordinates of the working set;
    /// the working points refer to them by position in this list.
    pub(crate) stations: &'a [[f64; 3]],
    /// Whether a station lies in the region.
    pub(crate) inside: &'a [bool],
    /// The middle of the box round the points.
    pub(crate) centre: [f64; 3],
}

/// Turn grown regions into planes: fit, let the left-over points beside a
/// plane join it, merge what is one plane, and turn every plane to the side
/// it was scanned from.
pub(crate) fn refine(
    cloud: &VoxelCloud,
    grown: Grown,
    config: &SurfaceDetectConfig,
    sides: &Sides<'_>,
) -> Segmentation {
    let Grown {
        mut labels,
        regions,
    } = grown;
    let total = cloud.len();
    let moments: Vec<Moments> = regions
        .par_iter()
        .map(|members| robust_moments(cloud, members, config.distance_tolerance))
        .collect();
    drop(regions);
    let fits: Vec<Option<PlaneFit>> = moments.iter().map(Moments::plane).collect();

    // Points left over beside a region join it: growing does not reach the
    // band along an edge, where the normals are those of two faces.
    for _ in 0..JOIN_ROUNDS {
        let joined: Vec<(u32, u32)> = (0..total as u32)
            .into_par_iter()
            .filter_map(|point| {
                if labels[point as usize] != NO_LABEL {
                    return None;
                }
                let position = cloud.position(point);
                let cell = cloud.cell(point);
                let mut best: Option<(f64, u32)> = None;
                for offset in AROUND {
                    let Some(other) = cloud.find(shifted(cell, offset)) else {
                        continue;
                    };
                    let label = labels[other as usize];
                    if label == NO_LABEL {
                        continue;
                    }
                    let Some(plane) = &fits[label as usize - 1] else {
                        continue;
                    };
                    let apart = plane.signed_distance(position).abs();
                    if apart <= config.distance_tolerance
                        && best.is_none_or(|best| (apart, label) < best)
                    {
                        best = Some((apart, label));
                    }
                }
                best.map(|(_, label)| (point, label))
            })
            .collect();
        if joined.is_empty() {
            break;
        }
        for (point, label) in joined {
            labels[point as usize] = label;
        }
    }

    // Regions that touch and lie in one plane are one region: growing stops
    // at a strip of poor normals, such as a step smaller than the tolerance,
    // that the plane itself runs through. Merged regions have a new plane,
    // which other regions may lie in, so this is repeated until nothing
    // merges.
    let half_angle = (config.angle_tolerance_deg * 0.5).to_radians().cos();
    let mut moments = moments;
    let mut fits = fits;
    // The pairs of regions that touch, as the last round found them.
    let mut touching: Vec<(u32, u32)>;
    loop {
        // Per pair of regions that touch: the number of contacts and the
        // summed distance of the points there to the plane of the other,
        // and how many points of the lower and of the higher label lie
        // against the other region.
        let mut contacts: BTreeMap<(u32, u32), (u64, f64, [u64; 2])> = BTreeMap::new();
        let mut sizes = vec![0u64; fits.len()];
        for point in 0..total as u32 {
            let own = labels[point as usize];
            if own == NO_LABEL {
                continue;
            }
            sizes[own as usize - 1] += 1;
            let cell = cloud.cell(point);
            let mut seen = [NO_LABEL; 26];
            let mut others = 0;
            for (index, offset) in AROUND.iter().enumerate() {
                let Some(other) = cloud.find(shifted(cell, *offset)) else {
                    continue;
                };
                let theirs = labels[other as usize];
                if theirs == NO_LABEL || theirs == own {
                    continue;
                }
                let (Some(a), Some(b)) = (&fits[own as usize - 1], &fits[theirs as usize - 1])
                else {
                    continue;
                };
                let contact = contacts
                    .entry((own.min(theirs), own.max(theirs)))
                    .or_insert((0, 0.0, [0; 2]));
                // Every pair of voxels once: the first half of the offsets.
                if index < 13 {
                    contact.0 += 1;
                    contact.1 += b.signed_distance(cloud.position(point)).abs()
                        + a.signed_distance(cloud.position(other)).abs();
                }
                if !seen[..others].contains(&theirs) {
                    seen[others] = theirs;
                    others += 1;
                    contact.2[usize::from(own > theirs)] += 1;
                }
            }
        }
        let mut parents: Vec<u32> = (0..fits.len() as u32).collect();
        // Per root the points of every region joined to it so far: a pair
        // is judged by what its two regions have become, so that a row of
        // regions that each lie in the plane of the next does not end as
        // one region round a bend.
        let mut joined = moments.clone();
        for ((a, b), (count, apart, against)) in &contacts {
            let (a, b) = (*a as usize - 1, *b as usize - 1);
            if *count == 0 {
                continue;
            }
            let near = apart / (2.0 * *count as f64) <= config.distance_tolerance;
            // Noise near the tolerance spreads one surface over two or
            // three layers of voxels, each of which grows into a region of
            // its own, further apart than the tolerance. Such regions lie
            // on top of each other, where the sides of a step meet along a
            // line: most of the smaller one lies against the other.
            let (lying, smaller) = if sizes[a] <= sizes[b] {
                (against[0], sizes[a])
            } else {
                (against[1], sizes[b])
            };
            if !near && 2 * lying < smaller {
                continue;
            }
            let (a, b) = (
                find_root(&mut parents, a as u32),
                find_root(&mut parents, b as u32),
            );
            if a == b {
                continue;
            }
            let (Some(first), Some(second)) =
                (joined[a as usize].plane(), joined[b as usize].plane())
            else {
                continue;
            };
            if angle_cosine(&first, &second) < half_angle {
                continue;
            }
            // Two faces that meet at a small angle touch as closely as two
            // parts of one plane, and so do the two sides of a bend. Only
            // the plane through both tells them apart: it has to be as flat
            // as growing demands of a region.
            let mut both = joined[a as usize];
            both.merge(&joined[b as usize]);
            if both
                .plane()
                .is_some_and(|fit| fit.rms() <= config.distance_tolerance)
            {
                join(&mut parents, a, b);
                joined[a.min(b) as usize] = both;
            }
        }
        touching = contacts.into_keys().collect();
        drop(joined);
        // New numbers in the order of the lowest old number of each plane.
        let mut renumbered = vec![NO_LABEL; fits.len()];
        let mut merged: Vec<Moments> = Vec::new();
        for old in 0..fits.len() as u32 {
            let root = find_root(&mut parents, old);
            if fits[root as usize].is_none() {
                continue;
            }
            if renumbered[root as usize] == NO_LABEL {
                merged.push(Moments::new());
                renumbered[root as usize] = merged.len() as u32;
            }
            renumbered[old as usize] = renumbered[root as usize];
            merged[renumbered[root as usize] as usize - 1].merge(&moments[old as usize]);
        }
        let settled = merged.len() == fits.len();
        for label in &mut labels {
            if *label != NO_LABEL {
                *label = renumbered[*label as usize - 1];
            }
        }
        // Every merged set holds a region that had a fit, so it has one too.
        fits = merged.iter().map(Moments::plane).collect();
        moments = merged;
        if settled {
            break;
        }
    }
    let mut planes: Vec<PlaneFit> = fits
        .into_iter()
        .map(|fit| fit.expect("a region with a plane"))
        .collect();
    // Nothing merged in the last round, so its pairs hold the final labels.
    settle(cloud, &mut labels, &mut planes, &touching, config);

    let normal_sources = orient(cloud, &labels, &mut planes, sides);

    // Planes that lie in each other's plane without touching: two parts of
    // a wall on either side of an opening.
    let mut groups: Vec<u32> = (0..planes.len() as u32).collect();
    for a in 0..planes.len() {
        for b in a + 1..planes.len() {
            if angle_cosine(&planes[a], &planes[b]) >= half_angle
                && planes[a].signed_distance(planes[b].centroid).abs() <= config.distance_tolerance
                && planes[b].signed_distance(planes[a].centroid).abs() <= config.distance_tolerance
            {
                join(&mut groups, a as u32, b as u32);
            }
        }
    }
    let coplanar_groups = (0..planes.len() as u32)
        .map(|plane| find_root(&mut groups, plane) + 1)
        .collect();

    Segmentation {
        labels,
        planes,
        normal_sources,
        coplanar_groups,
        cylinders: Vec::new(),
        hollow: Vec::new(),
        candidates: Vec::new(),
        pairs: Vec::new(),
    }
}

/// Let the points along the line between two faces at a small angle go to
/// the face they lie nearer to, and fit the planes of those faces again.
///
/// Growing takes the first of two such faces on past the line they share,
/// for as far as the other lies within the tolerance of its plane: half a
/// metre where a ramp of 3 degrees leaves a floor. The plane then leans
/// towards its neighbour, and the two faces are divided where growing
/// stopped. Two faces at a large angle have the line of their planes to
/// end on, whatever the labels say; below `MIN_EDGE_ANGLE_DEG` there is no
/// such line and the labels alone divide them. `touching` holds the pairs
/// of labels of planes that touch.
fn settle(
    cloud: &VoxelCloud,
    labels: &mut [u32],
    planes: &mut [PlaneFit],
    touching: &[(u32, u32)],
    config: &SurfaceDetectConfig,
) {
    let shallow = MIN_EDGE_ANGLE_DEG.to_radians().cos();
    let pairs: BTreeSet<(u32, u32)> = touching
        .iter()
        .filter(|(a, b)| {
            angle_cosine(&planes[*a as usize - 1], &planes[*b as usize - 1]) >= shallow
        })
        .copied()
        .collect();
    if pairs.is_empty() {
        return;
    }
    let mut involved = vec![false; planes.len()];
    for (a, b) in &pairs {
        involved[*a as usize - 1] = true;
        involved[*b as usize - 1] = true;
    }
    // A point moves only for a plane that is clearly nearer, so that noise
    // does not send it to and fro.
    let margin = config.distance_tolerance / 16.0;
    for _ in 0..SETTLE_ROUNDS {
        // The neighbouring plane a point lies nearer to than to its own.
        let nearer = |point: u32, labels: &[u32]| -> Option<u32> {
            let own = labels[point as usize];
            if own == NO_LABEL || !involved[own as usize - 1] {
                return None;
            }
            let position = cloud.position(point);
            let apart = planes[own as usize - 1].signed_distance(position).abs();
            let cell = cloud.cell(point);
            let mut best: Option<(f64, u32)> = None;
            for offset in AROUND {
                let Some(other) = cloud.find(shifted(cell, offset)) else {
                    continue;
                };
                let theirs = labels[other as usize];
                if theirs == NO_LABEL
                    || theirs == own
                    || !pairs.contains(&(own.min(theirs), own.max(theirs)))
                {
                    continue;
                }
                let off = planes[theirs as usize - 1].signed_distance(position).abs();
                if off + margin < apart && best.is_none_or(|best| (off, theirs) < best) {
                    best = Some((off, theirs));
                }
            }
            best.map(|(_, label)| label)
        };
        // The line between two faces moves a voxel at a time, so only the
        // points beside the last ones that moved are looked at again. Every
        // move is to a nearer plane, so this ends.
        let mut front: Vec<u32> = (0..cloud.len() as u32)
            .into_par_iter()
            .filter(|point| nearer(*point, labels).is_some())
            .collect();
        let mut changed: BTreeSet<u32> = BTreeSet::new();
        while !front.is_empty() {
            let moves: Vec<(u32, u32)> = front
                .par_iter()
                .filter_map(|point| nearer(*point, labels).map(|label| (*point, label)))
                .collect();
            let mut beside = Vec::with_capacity(26 * moves.len());
            for (point, label) in &moves {
                changed.insert(labels[*point as usize]);
                changed.insert(*label);
                labels[*point as usize] = *label;
                let cell = cloud.cell(*point);
                beside.extend(
                    AROUND
                        .iter()
                        .filter_map(|offset| cloud.find(shifted(cell, *offset))),
                );
            }
            beside.sort_unstable();
            beside.dedup();
            front = beside;
        }
        if changed.is_empty() {
            return;
        }
        // The planes that lost or gained points, fitted to what they hold
        // now. One that lost all keeps its plane and gets no outline.
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); planes.len()];
        for (point, label) in labels.iter().enumerate() {
            if *label != NO_LABEL && changed.contains(label) {
                members[*label as usize - 1].push(point as u32);
            }
        }
        for label in &changed {
            let members = &members[*label as usize - 1];
            if members.is_empty() {
                continue;
            }
            let fit = robust_moments(cloud, members, config.distance_tolerance).plane();
            if let Some(fit) = fit {
                planes[*label as usize - 1] = fit;
            }
        }
    }
}

/// Working points of a plane that look for the next surface in front of it
/// and behind it, and the number of voxels they look ahead.
const SIGHT_SAMPLES: usize = 256;
const SIGHT_STEPS: i32 = 256;
/// The first voxel looked at: the nearer ones hold the plane itself.
const SIGHT_FROM: i32 = 2;

/// How many voxels from a working point of a plane the next thing lies in a
/// direction; nothing when there is none within sight.
fn sight(cloud: &VoxelCloud, labels: &[u32], point: u32, direction: [f64; 3]) -> Option<i32> {
    let from = cloud.position(point);
    let own = labels[point as usize];
    (SIGHT_FROM..=SIGHT_STEPS).find(|step| {
        let reach = f64::from(*step) * cloud.voxel();
        let at = std::array::from_fn(|axis| from[axis] + reach * direction[axis]);
        cloud
            .cell_of(at)
            .and_then(|cell| cloud.find(cell))
            .is_some_and(|other| labels[other as usize] != own)
    })
}

/// The side of a plane that lies open, by a count over a sample of its
/// working points: above zero for the side of the normal, below for the
/// other, zero when the points do not say. A scanned face has the room it
/// was scanned from in front of it and at most the other face of its wall
/// or slab behind it, so the open side is the one where the next surface
/// lies further away, and a side with a surface wins from one without.
fn open_side(cloud: &VoxelCloud, labels: &[u32], normal: [f64; 3], sample: &[u32]) -> i64 {
    let back = normal.map(|value| -value);
    sample
        .iter()
        .map(|point| {
            let front = sight(cloud, labels, *point, normal);
            let behind = sight(cloud, labels, *point, back);
            match (front, behind) {
                (Some(front), Some(behind)) => i64::from((front - behind).signum()),
                (Some(_), None) => 1,
                (None, Some(_)) => -1,
                (None, None) => 0,
            }
        })
        .sum()
}

/// Turn every normal to the side its plane was scanned from: towards the
/// stations of its points where those are known, otherwise towards the
/// nearest station in the region, otherwise to the side that lies open, and
/// towards the middle of the region when nothing else says.
fn orient(
    cloud: &VoxelCloud,
    labels: &[u32],
    planes: &mut [PlaneFit],
    sides: &Sides<'_>,
) -> Vec<NormalSource> {
    // Per plane the sum of how squarely its points face their stations.
    let mut facing = vec![(0.0f64, 0u64); planes.len()];
    let mut sizes = vec![0usize; planes.len()];
    for point in 0..cloud.len() as u32 {
        let label = labels[point as usize];
        if label == NO_LABEL {
            continue;
        }
        sizes[label as usize - 1] += 1;
        let station = cloud.station(point);
        if station == NO_STATION {
            continue;
        }
        let Some(station) = sides.stations.get(station as usize) else {
            continue;
        };
        if let Some(towards) = unit(difference(*station, cloud.position(point))) {
            let sum = &mut facing[label as usize - 1];
            sum.0 += dot(planes[label as usize - 1].normal, towards);
            sum.1 += 1;
        }
    }
    // Without a station in the region, the planes that no station tells of
    // go by their open side: an even sample of each looks both ways.
    let mut samples: Vec<Vec<u32>> = vec![Vec::new(); planes.len()];
    if !sides.inside.contains(&true) {
        let mut seen = vec![0usize; planes.len()];
        for point in 0..cloud.len() as u32 {
            let label = labels[point as usize];
            if label == NO_LABEL {
                continue;
            }
            let plane = label as usize - 1;
            let (sum, known) = facing[plane];
            if known > 0 && sum != 0.0 {
                continue;
            }
            if seen[plane].is_multiple_of(sizes[plane].div_ceil(SIGHT_SAMPLES)) {
                samples[plane].push(point);
            }
            seen[plane] += 1;
        }
    }
    let open: Vec<i64> = samples
        .par_iter()
        .zip(planes.par_iter())
        .map(|(sample, plane)| open_side(cloud, labels, plane.normal, sample))
        .collect();
    planes
        .iter_mut()
        .zip(facing)
        .zip(open)
        .map(|((plane, (sum, known)), open)| {
            if known > 0 && sum != 0.0 {
                if sum < 0.0 {
                    plane.normal = plane.normal.map(|value| -value);
                }
                return NormalSource::Stations;
            }
            let nearest = sides
                .stations
                .iter()
                .zip(sides.inside)
                .filter(|(_, inside)| **inside)
                .map(|(station, _)| {
                    let apart = difference(*station, plane.centroid);
                    (dot(apart, apart), *station)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match nearest {
                Some((_, station)) => {
                    *plane = plane.towards(station);
                    NormalSource::NearestStation
                }
                None if open != 0 => {
                    if open < 0 {
                        plane.normal = plane.normal.map(|value| -value);
                    }
                    NormalSource::OpenSide
                }
                None => {
                    *plane = plane.towards(sides.centre);
                    NormalSource::Centre
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_voxels_around_are_all_there_and_the_first_half_has_no_opposites() {
        let all: BTreeSet<[i32; 3]> = AROUND.into_iter().collect();
        assert_eq!(all.len(), 26);
        assert!(!all.contains(&[0, 0, 0]));
        for offset in &AROUND[..13] {
            let opposite = offset.map(|value| -value);
            assert!(!AROUND[..13].contains(&opposite));
            assert!(AROUND[13..].contains(&opposite));
        }
    }

    #[test]
    fn median_of_a_list() {
        assert_eq!(median(&mut []), 0.0);
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 3.0);
    }
}
