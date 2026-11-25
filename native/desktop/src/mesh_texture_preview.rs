//! Native preview of the UV charts that were actually exported. Atlas pages
//! are decoded one at a time; their overview images share a 128 MiB budget.

use std::path::Path;
use std::sync::Arc;

use pointcloud_core::{AtlasChart, AtlasReport, MeshGeometry, PhotoTextureCoverage};

const MAX_PREVIEW_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct AtlasPixels {
    pub(crate) side: u32,
    pub(crate) layers: u32,
    pub(crate) rgba: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct BakedTexture {
    pub(crate) mesh: Arc<MeshGeometry>,
    /// page, chart X, chart Y, chart side: one entry per source triangle.
    pub(crate) charts: Vec<[u32; 4]>,
    pub(crate) report: AtlasReport,
    pub(crate) pixels: AtlasPixels,
    pub(crate) coverage: Option<PhotoTextureCoverage>,
    pub(crate) mirrored: bool,
}

fn preview_side(report: AtlasReport) -> Result<u32, String> {
    if !(1..=512).contains(&report.pages) || !(256..=4096).contains(&report.max_resolution) {
        return Err("Invalid texture atlas dimensions".into());
    }
    let stored_pages = report.pages.div_ceil(2) * 2;
    let budget = (MAX_PREVIEW_BYTES as f64 / (f64::from(stored_pages) * 4.0))
        .sqrt()
        .floor() as u32;
    Ok(report
        .max_resolution
        .min(budget)
        .min(crate::gpu_viewport::texture_max_page_side()))
}

impl BakedTexture {
    pub(crate) fn faces(&self) -> impl Iterator<Item = [u32; 4]> + '_ {
        self.mesh.triangles.iter().map(|face| {
            if self.mirrored {
                [face[0], face[2], face[1], 0]
            } else {
                [face[0], face[1], face[2], 0]
            }
        })
    }
    pub(crate) fn read(
        path: &Path,
        mesh: Arc<MeshGeometry>,
        report: AtlasReport,
        charts: Vec<AtlasChart>,
        coverage: Option<PhotoTextureCoverage>,
        mirrored: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let side = preview_side(report)?;
        if charts.len() != mesh.triangles.len()
            || charts.iter().any(|chart| {
                chart.page >= report.pages
                    || chart.side < 2
                    || chart.side > report.max_resolution - 2
                    || chart.position.iter().any(|v| {
                        v.checked_add(chart.side + 2)
                            .is_none_or(|end| end > report.max_resolution)
                    })
            })
        {
            return Err("Texture charts do not match the measured mesh".into());
        }

        let layers = report.pages.div_ceil(2);
        let mut rgba = vec![0; side as usize * side as usize * 2 * layers as usize * 4];
        let stem = path
            .file_stem()
            .ok_or("Texture has no file name")?
            .to_string_lossy();
        let directory = path.parent().unwrap_or(Path::new("."));
        for page in 0..report.pages {
            if cancelled() {
                return Err("Texture preview cancelled after export".into());
            }
            let mut reader =
                ::image::io::Reader::open(directory.join(format!("{stem}-atlas-{page:03}.png")))
                    .map_err(|error| error.to_string())?;
            reader.set_format(::image::ImageFormat::Png);
            let mut limits = ::image::io::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let decoded = reader.decode().map_err(|error| error.to_string())?;
            if decoded.width() != report.max_resolution || decoded.height() != report.max_resolution
            {
                return Err("Texture page dimensions changed after export".into());
            }
            let decoded = if side < report.max_resolution {
                decoded.resize_exact(side, side, ::image::imageops::FilterType::Triangle)
            } else {
                decoded
            }
            .into_rgba8();
            let source = decoded.as_raw();
            for row in 0..side as usize {
                let target = (((page / 2) as usize * side as usize + row) * side as usize * 2
                    + (page % 2) as usize * side as usize)
                    * 4;
                rgba[target..target + side as usize * 4]
                    .copy_from_slice(&source[row * side as usize * 4..][..side as usize * 4]);
            }
        }
        Ok(Self {
            mesh,
            report,
            coverage,
            mirrored,
            pixels: AtlasPixels { side, layers, rgba },
            charts: charts
                .into_iter()
                .map(|chart| [chart.page, chart.position[0], chart.position[1], chart.side])
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report(pages: u32, resolution: u32) -> AtlasReport {
        AtlasReport {
            triangles: 1,
            pages,
            resolution_limited: 0,
            texel_size: 0.02,
            max_resolution: resolution,
        }
    }
    #[test]
    fn every_supported_page_count_stays_within_the_preview_memory_budget() {
        for pages in 1..=512 {
            let report = report(pages, 4096);
            let side = preview_side(report).unwrap() as usize;
            assert!(side * side * 2 * pages.div_ceil(2) as usize * 4 <= MAX_PREVIEW_BYTES);
            assert!((256..=4096).contains(&side));
        }
        assert!(preview_side(report(0, 4096)).is_err());
        assert!(preview_side(report(513, 4096)).is_err());
    }
    #[test]
    fn preview_retains_the_exported_uv_chart_and_measured_pixels() {
        let mesh = Arc::new(MeshGeometry {
            vertices: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[25, 80, 140]; 3]),
            normals: None,
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mesh.obj");
        let baked = pointcloud_core::write_colored_obj_atlas_with_layout(
            &mesh,
            &path,
            pointcloud_core::AtlasOptions {
                max_resolution: 256,
                ..Default::default()
            },
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        let chart = baked.charts[0];
        let preview = BakedTexture::read(
            &path,
            Arc::clone(&mesh),
            baked.atlas,
            baked.charts,
            None,
            false,
            &|| false,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&preview.mesh, &mesh));
        assert_eq!(
            preview.charts[0],
            [chart.page, chart.position[0], chart.position[1], chart.side]
        );
        assert_eq!(
            &preview.pixels.rgba[(512 + 1) * 4..][..4],
            &[25, 80, 140, 255]
        );
    }
}
