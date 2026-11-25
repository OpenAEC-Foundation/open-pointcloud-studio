//! An actual E57 file carries the calibrated image used by this projection.
use pointcloud_core::{
    file_photos, read_file_photo, write_photographic_obj_atlas, AtlasOptions, MeshGeometry,
    PhotoAtlasRequest, PhotoTextureOptions,
};
use std::io::Cursor;

#[test]
fn an_e57_pinhole_checkerboard_projects_onto_the_measured_surface() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("calibrated.e57");
    let rgba: Vec<u8> = (0..16)
        .flat_map(|y| {
            (0..16).flat_map(move |x| {
                if (x / 4 + y / 4) % 2 == 0 {
                    [210, 30, 20, 255]
                } else {
                    [20, 180, 60, 255]
                }
            })
        })
        .collect();
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, 16, 16);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
    }
    {
        let mut writer =
            e57::E57Writer::from_file(&source, "{00000000-0000-4000-8000-000000000010}").unwrap();
        {
            let mut image = writer
                .add_image("{00000000-0000-4000-8000-000000000011}")
                .unwrap();
            image.set_name("Calibrated checkerboard");
            image.set_transform(e57::Transform {
                rotation: e57::Quaternion::default(),
                translation: e57::Translation {
                    x: 0.,
                    y: 0.,
                    z: 2.,
                },
            });
            image
                .add_pinhole(
                    e57::ImageFormat::Png,
                    &mut encoded.as_slice(),
                    e57::PinholeImageProperties {
                        width: 16,
                        height: 16,
                        focal_length: 1.,
                        pixel_width: 0.125,
                        pixel_height: 0.125,
                        principal_x: 7.5,
                        principal_y: 7.5,
                    },
                    None,
                )
                .unwrap();
            image.finalize().unwrap();
        }
        writer.finalize().unwrap();
    }
    let original = std::fs::read(&source).unwrap();
    let photos = file_photos(&source).unwrap().photos;
    assert_eq!(photos.len(), 1);
    assert_eq!(photos[0].position, [0., 0., 2.]);
    assert_eq!(read_file_photo(&source, &photos[0]).unwrap(), encoded);
    let mesh = MeshGeometry {
        vertices: vec![[-1., -1., 0.], [1., -1., 0.], [-1., 1., 0.]],
        triangles: vec![[0, 1, 2]],
        colors: None,
        normals: None,
    };
    let path = directory.path().join("checkerboard.obj");
    let report = write_photographic_obj_atlas(
        PhotoAtlasRequest {
            source_mesh: &mesh,
            output_mesh: &mesh,
            photos: &photos,
            atlas: AtlasOptions {
                texel_size: 0.02,
                max_resolution: 256,
            },
            photographs: PhotoTextureOptions::default(),
        },
        &path,
        &mut |index| {
            let bytes = read_file_photo(&source, &photos[index])?;
            let mut decoder = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
            let mut rgba = vec![0; decoder.output_buffer_size()];
            let frame = decoder.next_frame(&mut rgba).unwrap();
            assert_eq!(frame.color_type, png::ColorType::Rgba);
            Ok(pointcloud_core::photo_colour::PhotoPixels {
                width: frame.width,
                height: frame.height,
                rgba,
            })
        },
        &|| false,
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.coverage.photograph_texels, 10153);
    assert_eq!(report.coverage.unobserved_texels, 0);
    let mut decoder = png::Decoder::new(
        std::fs::File::open(directory.path().join("checkerboard-atlas-000.png")).unwrap(),
    )
    .read_info()
    .unwrap();
    let mut pixels = vec![0; decoder.output_buffer_size()];
    decoder.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[(54 * 256 + 54) * 3..][..3], &[20, 180, 60]);
    assert_eq!(std::fs::read(&source).unwrap(), original);
}
