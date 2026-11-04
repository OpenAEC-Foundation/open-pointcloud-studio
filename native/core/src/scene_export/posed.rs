//! Keep scan boundaries while the source parser and E57 writer stream once.
use super::{point_values, SceneSource};
use crate::{scene_transform::SceneTransform, LoadError, Point, ScanPose};
use e57::{E57Writer, Record};
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::path::Path;
use std::sync::mpsc::{sync_channel, SyncSender};

enum Event {
    Begin(Option<ScanPose>),
    Points(Vec<Point>),
}

fn produce(path: &Path, sender: SyncSender<Event>) -> Result<(), LoadError> {
    let batch = RefCell::new(Vec::with_capacity(4096));
    let stopped = Cell::new(false);
    let flush = || -> Result<(), LoadError> {
        if !batch.borrow().is_empty() {
            let points = std::mem::replace(&mut *batch.borrow_mut(), Vec::with_capacity(4096));
            sender
                .send(Event::Points(points))
                .map_err(|_| LoadError::Cancelled)?;
        }
        Ok(())
    };
    crate::visit_points_with_poses(
        path,
        &mut |point| {
            if stopped.get() {
                return Err(LoadError::Cancelled);
            }
            batch.borrow_mut().push(point);
            if batch.borrow().len() == 4096 {
                flush()?;
            }
            Ok(())
        },
        &mut |pose| {
            if flush().is_err() || sender.send(Event::Begin(pose)).is_err() {
                stopped.set(true);
            }
        },
    )?;
    if stopped.get() {
        return Err(LoadError::Cancelled);
    }
    flush()
}

pub(super) fn write(
    writer: &mut E57Writer<File>,
    source: &SceneSource<'_>,
    prototype: Vec<Record>,
    first_guid: &str,
    include: &mut impl FnMut(u64, &Point) -> bool,
    progress: &mut impl FnMut(u64) -> Result<(), LoadError>,
) -> Result<u64, LoadError> {
    std::thread::scope(|scope| {
        // At most two 4096-point batches wait; no full-cloud copy or spool.
        let (sender, receiver) = sync_channel(2);
        let parser = scope.spawn(move || produce(&source.cloud.path, sender));
        let result = (|| {
            // Own the receiver here so any write/cancellation error drops it
            // before joining, releasing a parser blocked by backpressure.
            let receiver = receiver;
            let mut next = receiver.recv().ok();
            let mut ordinal = 0;
            let mut written = 0;
            let mut scans = 0;
            while next.is_some() {
                let pose = match next.take().unwrap() {
                    Event::Begin(pose) => pose,
                    points @ Event::Points(_) => {
                        next = Some(points);
                        None
                    }
                };
                let guid = if scans == 0 {
                    first_guid.to_owned()
                } else {
                    format!("{{{}}}", uuid::Uuid::new_v4())
                };
                scans += 1;
                let mut output = writer.add_pointcloud(&guid, prototype.clone())?;
                let station = pose
                    .as_ref()
                    .map(|pose| {
                        let axes = pose.axes.unwrap_or(SceneTransform::default().rotation);
                        SceneTransform {
                            rotation: std::array::from_fn(|i| std::array::from_fn(|j| axes[j][i])),
                            translation: pose.position,
                            scale: 1.,
                        }
                        .validate()
                    })
                    .transpose()?;
                if let Some(station) = station {
                    output.set_transform(Some(source.transform.pose(Some(station.pose(None)))));
                }
                let filename = source
                    .cloud
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                output.set_name(Some(match pose {
                    Some(pose) => format!("{filename} — {}", pose.label),
                    None => format!("{filename} — block {scans}"),
                }));
                loop {
                    match next.take().or_else(|| receiver.recv().ok()) {
                        Some(begin @ Event::Begin(_)) => {
                            next = Some(begin);
                            break;
                        }
                        Some(Event::Points(points)) => {
                            for point in points {
                                if include(ordinal, &point) {
                                    let xyz = if let Some(station) = station {
                                        std::array::from_fn(|i| {
                                            (0..3)
                                                .map(|j| {
                                                    station.rotation[j][i]
                                                        * (point.xyz[j] - station.translation[j])
                                                })
                                                .sum::<f64>()
                                                * source.transform.scale
                                        })
                                    } else {
                                        source.transform.xyz(point.xyz)
                                    };
                                    output.add_point(point_values(source.cloud, point, xyz))?;
                                    written += 1;
                                }
                                ordinal += 1;
                                if ordinal % 4096 == 0 {
                                    progress(ordinal)?;
                                }
                            }
                        }
                        None => break,
                    }
                }
                output.finalize()?;
            }
            if ordinal != source.cloud.total_points {
                return Err(LoadError::InvalidData("source point count changed".into()));
            }
            progress(ordinal)?;
            Ok(written)
        })();
        let parsed = parser
            .join()
            .map_err(|_| LoadError::InvalidData("source parser panicked".into()))?;
        let written = result?;
        parsed?;
        Ok(written)
    })
}
