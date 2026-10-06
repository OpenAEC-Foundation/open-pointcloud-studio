//! What a sheet shows, as a list of marks on the paper in millimetres: the
//! border, the title block, and per viewport its drawing clipped to its
//! crop region, the image of its 3D view or why it shows neither, with its
//! title under it. The canvas draws the list and the PDF writer writes it,
//! so the window and the file show the same.

use crate::drawing_view::{ink, DrawScene};
use crate::i18n::tr;

use super::model::{paper_mm, scale_label, Layout, PlacedKind, Viewport, MARGIN, TITLE_ROOM};

/// Line widths on the paper, in millimetres.
pub const BORDER_WIDTH: f64 = 0.5;
pub const FRAME_WIDTH: f64 = 0.35;
pub const THIN_WIDTH: f64 = 0.18;
pub const DRAWING_WIDTH: f64 = 0.18;
/// A point of a drawing is a square of this side.
pub const DOT_SIZE: f64 = 0.12;
/// Texts smaller than this are left out.
const MIN_TEXT_HEIGHT: f64 = 0.4;
/// The heights of the title of a viewport and of its scale under it.
pub const TITLE_HEIGHT: f64 = 2.5;
const SCALE_HEIGHT: f64 = 1.8;
/// The heights of the labels and the values of the title block.
const LABEL_HEIGHT: f64 = 1.6;
const VALUE_HEIGHT: f64 = 2.8;
const BIG_HEIGHT: f64 = 4.0;
/// The ink of the border, the title block and the titles.
pub const INK: [u8; 3] = [24, 24, 27];
/// The ink of what says that a view is missing or not made yet.
const QUIET: [u8; 3] = [130, 130, 138];

/// Where a text stands on its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// One thing drawn on the paper.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Line {
        points: Vec<[f64; 2]>,
        closed: bool,
        width: f64,
        rgb: [u8; 3],
    },
    /// Rings filled by the even-odd rule, the outer one first.
    Fill {
        rings: Vec<Vec<[f64; 2]>>,
        rgb: [u8; 3],
    },
    Dot {
        at: [f64; 2],
        rgb: [u8; 3],
    },
    /// A line of text: `at` the left, middle or right end of its baseline,
    /// `height` the height of its capitals, turned by `rotation` radians
    /// counter-clockwise about `at`.
    Text {
        at: [f64; 2],
        height: f64,
        rotation: f64,
        value: String,
        rgb: [u8; 3],
        align: Align,
    },
    /// The picture of a 3D view, by the identifier of the view.
    Image {
        rect: [[f64; 2]; 2],
        key: String,
    },
}

/// Marks drawn inside a clip rectangle, or on the whole paper.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Group {
    pub clip: Option<[[f64; 2]; 2]>,
    pub marks: Vec<Mark>,
}

/// Where a viewport lies, for the pointer and the outline the canvas draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub id: String,
    pub rect: [[f64; 2]; 2],
    /// A locked viewport is selected by a click but not dragged.
    pub locked: bool,
}

/// A sheet as it is drawn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plot {
    pub size: [f64; 2],
    pub groups: Vec<Group>,
    pub viewports: Vec<Placed>,
    /// The notes on the paper with their marks, for the pointer.
    pub notes: Vec<(String, Vec<crate::drawing_notes::NoteMark>)>,
}

impl Plot {
    /// Every mark, with the clip of its group.
    pub fn marks(&self) -> impl Iterator<Item = (&Mark, Option<[[f64; 2]; 2]>)> {
        self.groups
            .iter()
            .flat_map(|group| group.marks.iter().map(move |mark| (mark, group.clip)))
    }

    /// The texts of the sheet, in the order they are drawn.
    #[cfg(test)]
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.marks().filter_map(|(mark, _)| match mark {
            Mark::Text { value, .. } => Some(value.as_str()),
            _ => None,
        })
    }

    /// The viewport under a point of the paper; the one drawn last first.
    pub fn viewport_at(&self, at: [f64; 2]) -> Option<&Placed> {
        self.viewports
            .iter()
            .rev()
            .find(|placed| super::model::contains(placed.rect, at))
    }
}

/// What a viewport shows.
pub enum Content<'a> {
    /// A drawing of VIEWS as it is made, with its crop region in metres in
    /// the frame of the drawing.
    Drawing {
        scene: &'a DrawScene,
        rect: [[f64; 2]; 2],
        /// Its annotations, at points of the model on its plane, with that
        /// plane and the scale their values are rounded at.
        notes: &'a [crate::drawing_notes::DrawingNote],
        frame: pointcloud_core::DrawingFrame,
        value_scale: f64,
    },
    /// The picture of a 3D view, of so many pixels.
    Image { key: String, pixels: [u32; 2] },
    /// It is not there yet, with why.
    Waiting(String),
    /// Its view or drawing was deleted.
    Missing,
}

/// The width of a text of capitals `height` high, in the proportions of
/// Helvetica, the font of the PDF.
pub fn text_width(value: &str, height: f64) -> f64 {
    let em = height / CAP_HEIGHT;
    value
        .chars()
        .map(|character| char_width(character) * em)
        .sum()
}

/// The height of the capitals of Helvetica as a part of its size.
pub const CAP_HEIGHT: f64 = 0.718;

/// The width of a character of Helvetica as a part of its size.
pub fn char_width(character: char) -> f64 {
    const ASCII: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    let code = character as u32;
    let width = if (32..127).contains(&code) {
        ASCII[(code - 32) as usize]
    } else {
        556
    };
    f64::from(width) / 1000.0
}

/// The ink of a colour of a drawing on white paper, as the Drawing view
/// shows it.
fn paper_ink(rgb: [u8; 3]) -> [u8; 3] {
    let color = ink(rgb);
    [color.r, color.g, color.b].map(|value| (value * 255.0).round() as u8)
}

fn line(points: Vec<[f64; 2]>, closed: bool, width: f64) -> Mark {
    Mark::Line {
        points,
        closed,
        width,
        rgb: INK,
    }
}

fn rectangle(rect: [[f64; 2]; 2], width: f64, rgb: [u8; 3]) -> Mark {
    let [min, max] = rect;
    Mark::Line {
        points: vec![min, [max[0], min[1]], max, [min[0], max[1]]],
        closed: true,
        width,
        rgb,
    }
}

fn text(at: [f64; 2], height: f64, value: impl Into<String>, align: Align) -> Mark {
    Mark::Text {
        at,
        height,
        rotation: 0.0,
        value: value.into(),
        rgb: INK,
        align,
    }
}

/// The marks of a sheet, with what each viewport shows from `content`.
pub fn plot<'a>(layout: &Layout, content: impl Fn(&Viewport) -> Content<'a>) -> Plot {
    let mut plot = Plot {
        size: layout.size(),
        ..Plot::default()
    };
    let mut paper = Group::default();
    paper
        .marks
        .push(rectangle(layout.border(), BORDER_WIDTH, INK));
    title_block(layout, &mut paper.marks);
    plot.groups.push(paper);
    for viewport in &layout.viewports {
        let shown = content(viewport);
        let size = match &shown {
            Content::Drawing { rect, .. } => super::model::drawing_size(*rect, viewport.scale),
            _ => viewport.size,
        };
        let rect = super::model::rect_around(viewport.centre, size);
        let mut group = Group {
            clip: Some(rect),
            marks: Vec::new(),
        };
        match shown {
            Content::Drawing {
                scene,
                rect: crop,
                notes,
                frame,
                value_scale,
            } => {
                drawing_marks(scene, crop, viewport, &mut group.marks);
                note_marks(notes, &frame, crop, viewport, value_scale, &mut group.marks);
            }
            // A picture keeps its proportions in a frame of others.
            Content::Image { key, pixels } => group.marks.push(Mark::Image {
                rect: super::model::contained(rect, pixels),
                key,
            }),
            Content::Waiting(why) => absent(rect, &why, &mut group.marks),
            Content::Missing => absent(rect, tr("view missing"), &mut group.marks),
        }
        plot.groups.push(group);
        plot.groups.push(Group {
            clip: None,
            marks: viewport_title(viewport, rect),
        });
        plot.viewports.push(Placed {
            id: viewport.id.clone(),
            rect,
            locked: viewport.locked,
        });
    }
    let mut paper = Group::default();
    for note in &layout.notes {
        let marks = crate::drawing_notes::paper_marks(note);
        plot.notes.push((note.id().to_owned(), marks.clone()));
        paper
            .marks
            .extend(marks.into_iter().map(|mark| paper_mark(mark, |at| at, 1.0)));
    }
    plot.groups.push(paper);
    plot
}

/// A mark of an annotation as a mark of the plot, its points through `place`
/// and its text heights times `grow`.
fn paper_mark(
    mark: crate::drawing_notes::NoteMark,
    place: impl Fn([f64; 2]) -> [f64; 2],
    grow: f64,
) -> Mark {
    use crate::drawing_notes::NoteMark;
    match mark {
        NoteMark::Line(points) => Mark::Line {
            points: points.into_iter().map(place).collect(),
            closed: false,
            width: THIN_WIDTH,
            rgb: INK,
        },
        NoteMark::Fill(points) => Mark::Fill {
            rings: vec![points.into_iter().map(place).collect()],
            rgb: INK,
        },
        NoteMark::Text {
            at,
            height,
            rotation,
            value,
            align,
        } => Mark::Text {
            at: place(at),
            height: height * grow,
            rotation,
            value,
            rgb: INK,
            align,
        },
    }
}

/// The annotations of a drawing in its viewport: at its scale, with the
/// sizes they have on paper.
fn note_marks(
    notes: &[crate::drawing_notes::DrawingNote],
    frame: &pointcloud_core::DrawingFrame,
    crop: [[f64; 2]; 2],
    viewport: &Viewport,
    value_scale: f64,
    marks: &mut Vec<Mark>,
) {
    let middle = [
        (crop[0][0] + crop[1][0]) / 2.0,
        (crop[0][1] + crop[1][1]) / 2.0,
    ];
    let scale = viewport.scale;
    let to_paper = |point: [f64; 2]| {
        [
            viewport.centre[0] + paper_mm(point[0] - middle[0], scale),
            viewport.centre[1] + paper_mm(point[1] - middle[1], scale),
        ]
    };
    for note in notes {
        for mark in crate::drawing_notes::note_marks(note, frame, scale, value_scale) {
            marks.push(paper_mark(mark, to_paper, 1000.0 / scale));
        }
    }
}

/// A viewport whose view is not shown: its outline and why, in its middle.
fn absent(rect: [[f64; 2]; 2], why: &str, marks: &mut Vec<Mark>) {
    marks.push(rectangle(rect, THIN_WIDTH, QUIET));
    let middle = [
        (rect[0][0] + rect[1][0]) / 2.0,
        (rect[0][1] + rect[1][1]) / 2.0,
    ];
    let width = rect[1][0] - rect[0][0];
    let height = (width / (text_width(why, 1.0) + 1.0)).clamp(1.0, 3.5);
    marks.push(Mark::Text {
        at: [middle[0], middle[1] - height / 2.0],
        height,
        rotation: 0.0,
        value: why.to_owned(),
        rgb: QUIET,
        align: Align::Centre,
    });
}

/// The title under a viewport, on a line as wide as the viewport, and the
/// scale of a drawing under that line.
fn viewport_title(viewport: &Viewport, rect: [[f64; 2]; 2]) -> Vec<Mark> {
    let left = rect[0][0];
    let base = rect[0][1] - TITLE_ROOM / 2.0;
    let title = viewport.shown_title().to_owned();
    let reach = (rect[1][0] - left).max(text_width(&title, TITLE_HEIGHT) + 2.0);
    let mut marks = vec![
        text([left, base], TITLE_HEIGHT, title, Align::Left),
        line(
            vec![[left, base - 1.0], [left + reach, base - 1.0]],
            false,
            THIN_WIDTH,
        ),
    ];
    if viewport.kind == PlacedKind::Drawing {
        marks.push(text(
            [left, base - 1.6 - SCALE_HEIGHT],
            SCALE_HEIGHT,
            scale_label(viewport.scale),
            Align::Left,
        ));
    }
    marks
}

/// The marks of a drawing at the scale of its viewport: every layer, the
/// fills first, then the points, the lines and the texts. What lies wholly
/// outside the crop region is left out; the rest is clipped by its group.
fn drawing_marks(
    scene: &DrawScene,
    crop: [[f64; 2]; 2],
    viewport: &Viewport,
    marks: &mut Vec<Mark>,
) {
    let factor = scene.units.factor();
    let middle = [
        (crop[0][0] + crop[1][0]) / 2.0,
        (crop[0][1] + crop[1][1]) / 2.0,
    ];
    let scale = viewport.scale;
    let to_paper = |point: [f64; 2]| {
        [
            viewport.centre[0] + paper_mm(point[0] / factor - middle[0], scale),
            viewport.centre[1] + paper_mm(point[1] / factor - middle[1], scale),
        ]
    };
    // The crop region in drawing units, a little larger for what lies on
    // its edge.
    let slack = (crop[1][0] - crop[0][0]).max(crop[1][1] - crop[0][1]) * 0.01;
    let reach = [
        [(crop[0][0] - slack) * factor, (crop[0][1] - slack) * factor],
        [(crop[1][0] + slack) * factor, (crop[1][1] + slack) * factor],
    ];
    let inside = |point: &[f64; 2]| {
        (0..2).all(|axis| reach[0][axis] <= point[axis] && point[axis] <= reach[1][axis])
    };
    let touches = |points: &[[f64; 2]]| {
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for point in points {
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
        (0..2).all(|axis| low[axis] <= reach[1][axis] && reach[0][axis] <= high[axis])
    };
    for layer in &scene.layers {
        let rgb = paper_ink(layer.rgb);
        for fill in &layer.fills {
            if !fill.first().is_some_and(|outer| touches(outer)) {
                continue;
            }
            marks.push(Mark::Fill {
                rings: fill
                    .iter()
                    .map(|ring| ring.iter().copied().map(to_paper).collect())
                    .collect(),
                rgb,
            });
        }
    }
    for layer in &scene.layers {
        let rgb = paper_ink(layer.rgb);
        for point in layer.points.iter().filter(|point| inside(&point.at)) {
            marks.push(Mark::Dot {
                at: to_paper(point.at),
                rgb: point.rgb.map_or(rgb, paper_ink),
            });
        }
    }
    for layer in &scene.layers {
        let rgb = paper_ink(layer.rgb);
        for drawn in layer
            .lines
            .iter()
            .filter(|drawn| drawn.points.len() >= 2 && touches(&drawn.points))
        {
            marks.push(Mark::Line {
                points: drawn.points.iter().copied().map(to_paper).collect(),
                closed: drawn.closed,
                width: DRAWING_WIDTH,
                rgb,
            });
        }
        for label in layer.texts.iter().filter(|label| inside(&label.at)) {
            let height = paper_mm(label.height / factor, scale);
            if height < MIN_TEXT_HEIGHT {
                continue;
            }
            marks.push(Mark::Text {
                at: to_paper(label.at),
                height,
                rotation: label.rotation,
                value: label.value.clone(),
                rgb,
                align: Align::Left,
            });
        }
    }
}

/// The title block: the project at the top, the name of the sheet under
/// it, and a row with the number, the scale, the date and who drew it.
fn title_block(layout: &Layout, marks: &mut Vec<Mark>) {
    let [min, max] = layout.title_block();
    let width = max[0] - min[0];
    marks.push(rectangle([min, max], FRAME_WIDTH, INK));
    let row = min[1] + 9.0;
    let middle = min[1] + 20.5;
    marks.push(line(vec![[min[0], row], [max[0], row]], false, THIN_WIDTH));
    marks.push(line(
        vec![[min[0], middle], [max[0], middle]],
        false,
        THIN_WIDTH,
    ));
    let cell = width / 4.0;
    for place in 1..4 {
        let x = min[0] + cell * place as f64;
        marks.push(line(vec![[x, min[1]], [x, row]], false, THIN_WIDTH));
    }
    let pad = 1.5;
    let label = |at: [f64; 2], value: &str| text(at, LABEL_HEIGHT, value, Align::Left);
    marks.push(label(
        [min[0] + pad, max[1] - pad - LABEL_HEIGHT],
        tr("Project"),
    ));
    marks.push(text(
        [min[0] + pad, middle + 2.0],
        BIG_HEIGHT,
        layout.project.clone(),
        Align::Left,
    ));
    marks.push(label(
        [min[0] + pad, middle - pad - LABEL_HEIGHT],
        tr("Sheet"),
    ));
    marks.push(text(
        [min[0] + pad, row + 2.0],
        BIG_HEIGHT,
        layout.name.clone(),
        Align::Left,
    ));
    let scale = layout.scale_text().unwrap_or_else(|| "—".to_owned());
    let cells = [
        (tr("Number"), layout.number.clone()),
        (tr("Drawing scale"), scale),
        (tr("Date"), layout.date.clone()),
        (tr("Drawn by"), layout.drawn_by.clone()),
    ];
    for (place, (caption, value)) in cells.into_iter().enumerate() {
        let x = min[0] + cell * place as f64 + pad;
        marks.push(label([x, row - pad - LABEL_HEIGHT], caption));
        marks.push(text([x, min[1] + pad], VALUE_HEIGHT, value, Align::Left));
    }
    // The program the sheet was made with, in the corner under the border.
    let [_, paper_max] = layout.border();
    marks.push(Mark::Text {
        at: [paper_max[0], MARGIN - 4.0],
        height: 1.4,
        rotation: 0.0,
        value: "Open Pointcloud Studio".into(),
        rgb: QUIET,
        align: Align::Right,
    });
}
