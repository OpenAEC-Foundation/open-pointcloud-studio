//! Non-destructive photo edits. The source E57 is immutable; a small sidecar
//! records camera changes and references to original external panorama files.
use crate::{FilePhoto, FilePhotos, LoadError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXTERNAL_PHOTO: u64 = 1 << 63;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhotoEdits {
    pub changes: BTreeMap<u64, Option<FilePhoto>>,
    pub added: BTreeMap<u64, ExternalPhoto>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExternalPhoto {
    pub path: PathBuf,
    pub photo: FilePhoto,
    source_length: u64,
    source_modified: Option<u128>,
}
impl ExternalPhoto {
    pub fn validate(&self) -> Result<(), LoadError> {
        let (length, modified) = source_identity(&self.path)?;
        if length != self.source_length || modified != self.source_modified {
            return Err(LoadError::InvalidData(
                "the added panorama changed since it was placed; add the new image again".into(),
            ));
        }
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedEdits {
    version: u32,
    source_length: u64,
    source_modified: Option<u128>,
    edits: PhotoEdits,
}
fn source_identity(source: &Path) -> Result<(u64, Option<u128>), LoadError> {
    let metadata = std::fs::metadata(source)?;
    Ok((
        metadata.len(),
        metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos()),
    ))
}

impl PhotoEdits {
    pub fn path(source: &Path) -> PathBuf {
        let mut path = source.as_os_str().to_owned();
        path.push(".ops-photos.json");
        PathBuf::from(path)
    }

    pub fn load(source: &Path) -> Result<Self, LoadError> {
        match std::fs::read(Self::path(source)) {
            Ok(bytes) => {
                let saved: SavedEdits = serde_json::from_slice(&bytes).map_err(|error| {
                    LoadError::InvalidData(format!("invalid photo edits: {error}"))
                })?;
                let (length, modified) = source_identity(source)?;
                if saved.version != 1
                    || saved.source_length != length
                    || saved.source_modified != modified
                {
                    return Err(LoadError::InvalidData("photo edits belong to an earlier revision of this scan; restore the original source before using them".into()));
                }
                Ok(saved.edits)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, source: &Path) -> Result<(), LoadError> {
        use std::io::Write;
        for photo in self
            .changes
            .values()
            .flatten()
            .chain(self.added.values().map(|e| &e.photo))
        {
            validate_photo(photo)?;
        }
        let path = Self::path(source);
        let mut temporary =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        let (source_length, source_modified) = source_identity(source)?;
        let bytes = serde_json::to_vec_pretty(&SavedEdits {
            version: 1,
            source_length,
            source_modified,
            edits: self.clone(),
        })
        .map_err(|error| LoadError::InvalidData(error.to_string()))?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| LoadError::Io(error.error))?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.added.is_empty()
    }

    pub fn apply(&self, mut photos: FilePhotos) -> FilePhotos {
        photos.photos = photos
            .photos
            .into_iter()
            .filter_map(|photo| {
                self.changes
                    .get(&photo.offset)
                    .cloned()
                    .unwrap_or(Some(photo))
            })
            .collect();
        photos
            .photos
            .extend(self.added.values().filter_map(|external| {
                self.changes
                    .get(&external.photo.offset)
                    .cloned()
                    .unwrap_or(Some(external.photo.clone()))
            }));
        photos
    }

    pub fn add(&mut self, path: PathBuf, mut photo: FilePhoto) -> Result<u64, LoadError> {
        validate_photo(&photo)?;
        let (source_length, source_modified) = source_identity(&path)?;
        if source_length > 256 * 1024 * 1024 {
            return Err(LoadError::InvalidData(
                "panorama image exceeds 256 MiB".into(),
            ));
        }
        let id = self
            .added
            .keys()
            .next_back()
            .copied()
            .unwrap_or(EXTERNAL_PHOTO)
            + 1;
        photo.offset = id;
        self.added.insert(
            id,
            ExternalPhoto {
                path,
                photo,
                source_length,
                source_modified,
            },
        );
        Ok(id)
    }
}

fn validate_photo(photo: &FilePhoto) -> Result<(), LoadError> {
    if photo.width == 0 || photo.height == 0 {
        return Err(LoadError::InvalidData(
            "photo dimensions must be positive".into(),
        ));
    }
    crate::scene_transform::SceneTransform {
        rotation: std::array::from_fn(|i| std::array::from_fn(|j| photo.axes[j][i])),
        translation: photo.position,
        scale: 1.,
    }
    .validate()?;
    Ok(())
}

pub(crate) fn photo_pose(photo: &FilePhoto) -> e57::Transform {
    let rotation = std::array::from_fn(|i| std::array::from_fn(|j| photo.axes[j][i]));
    let q = crate::scene_transform::quaternion(rotation);
    e57::Transform {
        rotation: e57::Quaternion {
            w: q[0],
            x: q[1],
            y: q[2],
            z: q[3],
        },
        translation: e57::Translation {
            x: photo.position[0],
            y: photo.position[1],
            z: photo.position[2],
        },
    }
}

pub(crate) fn write_added(
    writer: &mut e57::E57Writer<std::fs::File>,
    edits: &PhotoEdits,
    transform: crate::scene_transform::SceneTransform,
    associated_guid: Option<&str>,
) -> Result<(), LoadError> {
    for external in edits.added.values() {
        let photo = match edits.changes.get(&external.photo.offset) {
            Some(None) => continue,
            Some(Some(photo)) => photo,
            None => &external.photo,
        };
        external.validate()?;
        let mut input = std::fs::File::open(&external.path)?;
        let guid = format!("{{{}}}", uuid::Uuid::new_v4());
        let mut image = writer.add_image(&guid)?;
        image.set_name(photo.name.as_deref().unwrap_or("Panorama"));
        if let Some(guid) = associated_guid {
            image.set_pointcloud_guid(guid);
        }
        image.set_transform(transform.pose(Some(photo_pose(photo))));
        let format = match photo.format {
            crate::ScanImageFormat::Jpeg => e57::ImageFormat::Jpeg,
            crate::ScanImageFormat::Png => e57::ImageFormat::Png,
        };
        let crate::PhotoProjection::Spherical { pixel_size } = photo.projection else {
            return Err(LoadError::InvalidData(
                "external images must be spherical panoramas".into(),
            ));
        };
        image.add_spherical(
            format,
            &mut input,
            e57::SphericalImageProperties {
                width: photo.width,
                height: photo.height,
                pixel_width: pixel_size[0],
                pixel_height: pixel_size[1],
            },
            None,
        )?;
        image.finalize()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_refuses_a_replaced_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scan.xyz");
        std::fs::write(&source, b"0 0 0\n").unwrap();
        PhotoEdits::default().save(&source).unwrap();
        assert!(PhotoEdits::load(&source).is_ok());
        std::fs::write(&source, b"0 0 0\n1 1 1\n").unwrap();
        assert!(PhotoEdits::load(&source).is_err());
    }
}
