//! A sheet written as a PDF of one page the size of its paper: the lines,
//! fills and points of its drawings and its title block as vectors, the
//! pictures of its 3D views as images, and its texts in Helvetica, one of
//! the standard fonts every PDF reader has. The content is drawn in
//! millimetres and compressed.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use pdf_writer::types::{LineCapStyle, LineJoinStyle};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use super::plot::{text_width, Align, Mark, Plot, CAP_HEIGHT};

/// Points to a millimetre.
const POINTS: f64 = 72.0 / 25.4;
const FONT: Name<'static> = Name(b"F1");

fn compressed(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .map_err(|error| error.to_string())
}

/// A text in the WinAnsi encoding of the standard fonts: what it cannot
/// hold becomes a question mark.
pub(crate) fn win_ansi(value: &str) -> Vec<u8> {
    value
        .chars()
        .map(|character| match character {
            ' '..='~' => character as u8,
            '\u{a0}'..='\u{ff}' => character as u32 as u8,
            '€' => 0x80,
            '‚' => 0x82,
            '„' => 0x84,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            _ => b'?',
        })
        .collect()
}

fn rgb(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|value| f32::from(value) / 255.0)
}

/// The PDF of a plot, with the PNG pictures of its 3D views by key.
pub(crate) fn pdf_bytes(
    plot: &Plot,
    pictures: &HashMap<String, Arc<Vec<u8>>>,
    title: &str,
) -> Result<Vec<u8>, String> {
    let catalog = Ref::new(1);
    let tree = Ref::new(2);
    let page = Ref::new(3);
    let font = Ref::new(4);
    let contents = Ref::new(5);
    let info = Ref::new(6);
    let mut next = 7;
    let mut pdf = Pdf::new();

    // The pictures, each once.
    let mut images: Vec<(String, Ref, Vec<u8>, [u32; 2])> = Vec::new();
    for (mark, _) in plot.marks() {
        let Mark::Image { key, .. } = mark else {
            continue;
        };
        if images.iter().any(|(known, ..)| known == key) {
            continue;
        }
        let Some(png) = pictures.get(key) else {
            continue;
        };
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|error| format!("the picture of a 3D view could not be read: {error}"))?
            .to_rgb8();
        let size = [decoded.width(), decoded.height()];
        images.push((
            key.clone(),
            Ref::new(next),
            compressed(decoded.as_raw())?,
            size,
        ));
        next += 1;
    }

    let mut content = Content::new();
    // Everything is drawn in millimetres from the lower left corner.
    let k = POINTS as f32;
    content.transform([k, 0.0, 0.0, k, 0.0, 0.0]);
    content.set_line_cap(LineCapStyle::RoundCap);
    content.set_line_join(LineJoinStyle::RoundJoin);
    for group in &plot.groups {
        content.save_state();
        if let Some([min, max]) = group.clip {
            content.rect(
                min[0] as f32,
                min[1] as f32,
                (max[0] - min[0]) as f32,
                (max[1] - min[1]) as f32,
            );
            content.clip_nonzero();
            content.end_path();
        }
        let mut dots: Option<[u8; 3]> = None;
        for mark in &group.marks {
            // Points of one colour in a row are filled together.
            let dot = match mark {
                Mark::Dot { rgb, .. } => Some(*rgb),
                _ => None,
            };
            if dots.is_some() && dot != dots {
                content.fill_nonzero();
                dots = None;
            }
            write_mark(&mut content, mark, &images, &mut dots);
        }
        if dots.is_some() {
            content.fill_nonzero();
        }
        content.restore_state();
    }
    let stream = compressed(&content.finish())?;

    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let [width, height] = plot.size;
    {
        let mut written = pdf.page(page);
        written
            .parent(tree)
            .media_box(Rect::new(
                0.0,
                0.0,
                (width * POINTS) as f32,
                (height * POINTS) as f32,
            ))
            .contents(contents);
        let mut resources = written.resources();
        resources.fonts().pair(FONT, font);
        if !images.is_empty() {
            let mut objects = resources.x_objects();
            for (place, (_, id, ..)) in images.iter().enumerate() {
                let name = format!("Im{place}");
                objects.pair(Name(name.as_bytes()), *id);
            }
        }
    }
    pdf.type1_font(font)
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.stream(contents, &stream).filter(Filter::FlateDecode);
    for (_, id, data, [width, height]) in &images {
        let mut image = pdf.image_xobject(*id, data);
        image.filter(Filter::FlateDecode);
        image.width(*width as i32);
        image.height(*height as i32);
        image.color_space().device_rgb();
        image.bits_per_component(8);
        image.interpolate(true);
        image.finish();
    }
    pdf.document_info(info)
        .title(TextStr(title))
        .creator(TextStr("Open Pointcloud Studio"))
        .producer(TextStr("Open Pointcloud Studio"));
    Ok(pdf.finish())
}

fn write_mark(
    content: &mut Content,
    mark: &Mark,
    images: &[(String, Ref, Vec<u8>, [u32; 2])],
    dots: &mut Option<[u8; 3]>,
) {
    match mark {
        Mark::Line {
            points,
            closed,
            width,
            rgb: colour,
        } => {
            let Some(first) = points.first() else {
                return;
            };
            let [r, g, b] = rgb(*colour);
            content.set_stroke_rgb(r, g, b);
            content.set_line_width(*width as f32);
            content.move_to(first[0] as f32, first[1] as f32);
            for point in &points[1..] {
                content.line_to(point[0] as f32, point[1] as f32);
            }
            if *closed {
                content.close_path();
            }
            content.stroke();
        }
        Mark::Fill { rings, rgb: colour } => {
            let [r, g, b] = rgb(*colour);
            content.set_fill_rgb(r, g, b);
            for ring in rings.iter().filter(|ring| ring.len() >= 3) {
                content.move_to(ring[0][0] as f32, ring[0][1] as f32);
                for point in &ring[1..] {
                    content.line_to(point[0] as f32, point[1] as f32);
                }
                content.close_path();
            }
            content.fill_even_odd();
        }
        Mark::Dot { at, rgb: colour } => {
            if dots.is_none() {
                let [r, g, b] = rgb(*colour);
                content.set_fill_rgb(r, g, b);
                *dots = Some(*colour);
            }
            let side = super::plot::DOT_SIZE as f32;
            content.rect(
                at[0] as f32 - side / 2.0,
                at[1] as f32 - side / 2.0,
                side,
                side,
            );
        }
        Mark::Text {
            at,
            height,
            rotation,
            value,
            rgb: colour,
            align,
        } => {
            let width = text_width(value, *height);
            let shift = match align {
                Align::Left => 0.0,
                Align::Centre => -width / 2.0,
                Align::Right => -width,
            };
            let (sin, cos) = rotation.sin_cos();
            let x = at[0] + shift * cos;
            let y = at[1] + shift * sin;
            let [r, g, b] = rgb(*colour);
            content.set_fill_rgb(r, g, b);
            content.begin_text();
            content.set_font(FONT, (*height / CAP_HEIGHT) as f32);
            content.set_text_matrix([
                cos as f32,
                sin as f32,
                -sin as f32,
                cos as f32,
                x as f32,
                y as f32,
            ]);
            content.show(Str(&win_ansi(value)));
            content.end_text();
        }
        Mark::Image { rect, key } => {
            let Some(place) = images.iter().position(|(known, ..)| known == key) else {
                return;
            };
            let [min, max] = rect;
            let name = format!("Im{place}");
            content.save_state();
            content.transform([
                (max[0] - min[0]) as f32,
                0.0,
                0.0,
                (max[1] - min[1]) as f32,
                min[0] as f32,
                min[1] as f32,
            ]);
            content.x_object(Name(name.as_bytes()));
            content.restore_state();
        }
    }
}

/// Write the PDF of a plot to `path`, beside it first and then in its
/// place, so that a failed export leaves an earlier file as it was.
/// Answers the size of the file.
pub(crate) fn write_pdf(
    path: &Path,
    plot: &Plot,
    pictures: &HashMap<String, Arc<Vec<u8>>>,
    title: &str,
) -> Result<u64, String> {
    let bytes = pdf_bytes(plot, pictures, title)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    Ok(bytes.len() as u64)
}
