//! What of the window a job reads: the layers where they stand, their
//! deleted points, the classes shown, and the box it reads in. The Section
//! drawing, Closed mesh and Detect faces tools make one from the section box
//! of the window; a job of the Pointcloud to Drawing wizard reads a box of its own,
//! such as one storey, whatever the section box is.

use std::sync::Arc;

use pointcloud_core::region_source::SourceTransform;
use pointcloud_core::{Bounds, LoadError, OctreeIndex, OrientedBox, Point, PointCloud};

use crate::closed_mesh::UNINDEXED_LIMIT;
use crate::cloud_transform::CloudTransform;
use crate::selection::{ClassFilter, DeletionMask};
use crate::{display_name, CloudEntry};

/// One layer as a job reads it.
pub(crate) struct JobLayer {
    /// The cloud the job reads: that of the layer, or the same cloud with
    /// the station of every point when an earlier job found those out.
    pub(crate) cloud: Arc<PointCloud>,
    /// What tells the layer from the others for as long as it is open.
    pub(crate) identity: Arc<PointCloud>,
    /// The file name of the scan.
    pub(crate) name: String,
    pub(crate) index: Option<Arc<OctreeIndex>>,
    pub(crate) transform: CloudTransform,
    pub(crate) deleted: Option<Arc<DeletionMask>>,
}

impl JobLayer {
    /// A layer of the project list where it stands, read through `cloud`.
    pub(crate) fn of(entry: &CloudEntry, cloud: Arc<PointCloud>) -> Self {
        Self {
            cloud,
            identity: Arc::clone(&entry.load_identity),
            name: display_name(&entry.cloud.path).to_owned(),
            index: entry.index.as_ref().map(Arc::clone),
            transform: entry.transform,
            deleted: entry.deleted.as_ref().map(Arc::clone),
        }
    }

    /// A scan file without a window: no deleted points, where it lies.
    pub(crate) fn of_file(cloud: PointCloud, index: Option<OctreeIndex>) -> Self {
        let cloud = Arc::new(cloud);
        Self {
            identity: Arc::clone(&cloud),
            name: display_name(&cloud.path).to_owned(),
            cloud,
            index: index.map(Arc::new),
            transform: CloudTransform::default(),
            deleted: None,
        }
    }

    /// The file name of the scan without its extension, which names the
    /// layer of its points in a drawing.
    pub(crate) fn stem(&self) -> &str {
        self.cloud
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("scan")
    }

    /// Where the points of the source go in the scene.
    pub(crate) fn source_transform(&self) -> SourceTransform {
        SourceTransform {
            scale: self.transform.scale,
            offset: self.transform.offset,
        }
    }

    /// Whether a job holds the points of this layer in memory: a layer
    /// without an index that is small enough for that. A larger one is read
    /// from its file.
    pub(crate) fn resident(&self) -> bool {
        self.index.is_none() && self.cloud.total_points <= UNINDEXED_LIMIT
    }

    /// Whether the file of this layer is read from start to end.
    pub(crate) fn streamed(&self) -> bool {
        self.index.is_none() && !self.resident()
    }
}

/// The layers a job reads, with the box it reads in and the classes shown.
pub(crate) struct JobScene {
    /// The box the job reads in, turned or not; `None` for everything the
    /// layers hold. The core is given the box around it, and `accepts`
    /// leaves out what lies in its corners.
    pub(crate) section: Option<OrientedBox>,
    /// The classes shown. Its own box is none: the box of the job is the
    /// one above.
    pub(crate) filter: ClassFilter,
    pub(crate) layers: Vec<JobLayer>,
}

impl JobScene {
    /// The layers read inside `section`, whatever the section box of the
    /// window is.
    pub(crate) fn for_box(
        section: OrientedBox,
        filter: ClassFilter,
        layers: Vec<JobLayer>,
    ) -> Self {
        Self::new(Some(section), filter, layers)
    }

    /// The layers read inside `section`, or wherever they have points.
    pub(crate) fn new(
        section: Option<OrientedBox>,
        filter: ClassFilter,
        layers: Vec<JobLayer>,
    ) -> Self {
        Self {
            section,
            filter: ClassFilter {
                section: None,
                ..filter
            },
            layers,
        }
    }

    /// An index is read without a look at the file it was built from, so
    /// that the file is still the one that was opened is checked first.
    pub(crate) fn validate(&self) -> Result<(), LoadError> {
        for layer in self.layers.iter().filter(|layer| layer.index.is_some()) {
            layer.cloud.validate_source()?;
        }
        Ok(())
    }

    /// The region the core reads: the box around the box of the job.
    pub(crate) fn region(&self) -> Option<Bounds> {
        self.section.map(|section| section.aabb())
    }

    /// Whether a point the core read from a layer takes part: it was not
    /// deleted and its class is shown.
    pub(crate) fn keeps(&self, position: usize, ordinal: u64, point: &Point) -> bool {
        self.layers[position]
            .deleted
            .as_ref()
            .is_none_or(|mask| !mask.contains(ordinal))
            && self.filter.accepts(point)
    }

    /// As `keeps`, and the point lies inside a turned box as well, not only
    /// in the box around it that the core reads.
    pub(crate) fn accepts(&self, position: usize, ordinal: u64, point: &Point) -> bool {
        self.keeps(position, ordinal, point)
            && self
                .section
                .is_none_or(|section| !section.is_turned() || section.contains(point.xyz))
    }

    /// The part of the box of the job that can hold points of the layers,
    /// as the core takes it, or the box around the layers without a box.
    /// `None` without layers.
    pub(crate) fn bounds(&self) -> Option<Bounds> {
        let data = self
            .layers
            .iter()
            .map(|layer| layer.transform.bounds(layer.cloud.bounds))
            .reduce(|all, bounds| Bounds {
                min: std::array::from_fn(|axis| all.min[axis].min(bounds.min[axis])),
                max: std::array::from_fn(|axis| all.max[axis].max(bounds.max[axis])),
            })?;
        Some(match self.region() {
            Some(section) => Bounds {
                min: std::array::from_fn(|axis| section.min[axis].max(data.min[axis])),
                max: std::array::from_fn(|axis| section.max[axis].min(data.max[axis])),
            },
            None => data,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pointcloud_core::IndexedPoint;

    use super::*;
    use crate::selection::{ClassVisibility, SelectionMask};
    use crate::{Message, Studio};

    /// Every class shown, as on the command line.
    fn every_class() -> ClassFilter {
        ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        }
    }

    /// The walls of a room of 4 by 3 m and 2.5 m high, a point every 0.1 m.
    fn room_points() -> Vec<[f64; 3]> {
        let mut points = Vec::new();
        for level in 0..=25 {
            let z = f64::from(level) * 0.1;
            for step in 0..=40 {
                let x = f64::from(step) * 0.1;
                points.push([x, 0.0, z]);
                points.push([x, 3.0, z]);
            }
            for step in 1..30 {
                let y = f64::from(step) * 0.1;
                points.push([0.0, y, z]);
                points.push([4.0, y, z]);
            }
        }
        points
    }

    fn studio_with_room(directory: &Path) -> Studio {
        let path = directory.join("room.xyz");
        let text: String = room_points()
            .iter()
            .map(|[x, y, z]| format!("{x:.3} {y:.3} {z:.3}\n"))
            .collect();
        std::fs::write(&path, text).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 1_000_000).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio
    }

    fn layers(studio: &Studio) -> Vec<JobLayer> {
        studio
            .clouds
            .iter()
            .map(|entry| JobLayer::of(entry, Arc::clone(&entry.cloud)))
            .collect()
    }

    fn point(xyz: [f64; 3], classification: Option<u8>) -> Point {
        Point {
            xyz,
            rgb: None,
            intensity: None,
            classification,
        }
    }

    #[test]
    fn a_box_of_its_own_is_read_whatever_the_section_box_is() {
        let directory = tempfile::tempdir().unwrap();
        let studio = studio_with_room(directory.path());
        assert!(studio.section_box().is_none());
        let storey = OrientedBox::new(
            Bounds {
                min: [0.5, -1.0, 0.0],
                max: [3.5, 2.0, 1.25],
            },
            30.0,
        );
        let scene = JobScene::for_box(storey, studio.mesh_filter(), layers(&studio));
        assert_eq!(scene.section, Some(storey));
        assert_eq!(scene.region(), Some(storey.aabb()));
        assert!(scene.validate().is_ok());
        let layer = &scene.layers[0];
        assert_eq!((layer.name.as_str(), layer.stem()), ("room.xyz", "room"));
        assert!(layer.resident() && !layer.streamed());

        // Every point inside the turned box is read, and only those; what
        // the box around it holds in its corners is left out.
        let points = room_points();
        let inside = points.iter().filter(|xyz| storey.contains(**xyz)).count();
        let around = points
            .iter()
            .filter(|xyz| {
                let aabb = storey.aabb();
                (0..3).all(|axis| (aabb.min[axis]..=aabb.max[axis]).contains(&xyz[axis]))
            })
            .count();
        let accepted = points
            .iter()
            .enumerate()
            .filter(|(ordinal, xyz)| scene.accepts(0, *ordinal as u64, &point(**xyz, None)))
            .count();
        assert_eq!(accepted, inside);
        assert!(0 < inside && inside < around, "{inside} of {around}");
        assert!(points.iter().enumerate().all(|(ordinal, xyz)| scene.keeps(
            0,
            ordinal as u64,
            &point(*xyz, None)
        )));

        // The box cut back to where the room has points, in height too.
        let bounds = scene.bounds().unwrap();
        let aabb = storey.aabb();
        assert_eq!(
            bounds.min,
            [aabb.min[0].max(0.0), aabb.min[1].max(0.0), 0.0]
        );
        assert_eq!(
            bounds.max,
            [aabb.max[0].min(4.0), aabb.max[1].min(3.0), 1.25]
        );

        // Without a box the whole room is read.
        let everywhere = JobScene::new(None, studio.mesh_filter(), layers(&studio));
        assert_eq!(everywhere.region(), None);
        let room = everywhere.bounds().unwrap();
        assert_eq!((room.min, room.max), ([0.0; 3], [4.0, 3.0, 2.5]));
        assert!(points
            .iter()
            .all(|xyz| everywhere.accepts(0, 0, &point(*xyz, None))));
    }

    #[test]
    fn deleted_points_and_hidden_classes_stay_out_and_the_filter_has_no_box() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_room(directory.path());
        let entry = &mut studio.clouds[0];
        let total = entry.cloud.total_points;
        let mut deleted = DeletionMask::new(total).unwrap();
        let gone = entry.cloud.point_ordinals[5];
        let kept = entry.cloud.point_ordinals[6];
        let record = IndexedPoint {
            point: entry.cloud.points[5],
            ordinal: gone,
        };
        deleted
            .apply(&SelectionMask::single(total, record).unwrap())
            .unwrap();
        entry.deleted = Some(Arc::new(deleted));

        let mut filter = every_class();
        filter.ground = false;
        filter.section = Some(OrientedBox::new(
            Bounds {
                min: [10.0; 3],
                max: [11.0; 3],
            },
            0.0,
        ));
        let scene = JobScene::new(None, filter, layers(&studio));
        // The box of the job is the only box: the filter keeps none.
        assert_eq!(scene.filter.section, None);
        let wall = point([0.0, 1.0, 1.0], None);
        assert!(scene.keeps(0, kept, &wall));
        assert!(!scene.keeps(0, gone, &wall), "a deleted point");
        assert!(!scene.accepts(0, gone, &wall));
        let ground = point([0.0, 1.0, 1.0], Some(2));
        assert!(!scene.keeps(0, kept, &ground), "a class that is hidden");
        assert!(scene.keeps(0, kept, &point([0.0, 1.0, 1.0], Some(6))));
    }

    #[test]
    fn a_scan_file_is_read_where_it_lies_with_all_its_points() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hall.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = pointcloud_core::open(&path, 10).unwrap();
        let layer = JobLayer::of_file(cloud, None);
        assert!(Arc::ptr_eq(&layer.cloud, &layer.identity));
        assert_eq!((layer.name.as_str(), layer.stem()), ("hall.xyz", "hall"));
        assert!(layer.deleted.is_none() && layer.transform.is_identity());
        let transform = layer.source_transform();
        assert_eq!((transform.scale, transform.offset), ([1.0; 3], [0.0; 3]));
        let scene = JobScene::for_box(
            OrientedBox::new(
                Bounds {
                    min: [-1.0; 3],
                    max: [1.0; 3],
                },
                0.0,
            ),
            every_class(),
            vec![layer],
        );
        let bounds = scene.bounds().unwrap();
        assert_eq!((bounds.min, bounds.max), ([0.0; 3], [1.0, 1.0, 1.0]));
    }
}
