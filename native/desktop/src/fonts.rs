//! The fonts of the window and their roles, as the OpenAEC style book has
//! them: Inter for all text of the interface in three weights, and Space
//! Grotesk Medium for headings only. Each weight is a static font file of its
//! own, since iced picks a weight from the files it has and applies no axis
//! of a variable font.

use iced::font::{Family, Weight};
use iced::Font;

/// The font files the window brings along.
pub const FILES: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/SpaceGrotesk-Medium.ttf"),
];

/// Text of the interface: rows, labels, buttons, tooltips, the label of a
/// ribbon button.
pub const REGULAR: Font = Font::with_name("Inter");

/// Tabs of the ribbon and of Settings, the captions of ribbon groups, the
/// label of an active ribbon button, the value of a status bar item.
pub const MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..REGULAR
};

/// Panel and section heads, dialog titles, the File button, card titles,
/// counts.
pub const SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..REGULAR
};

/// Headings: the title of a page of the File view, the empty scene.
pub const HEADING: Font = Font {
    family: Family::Name("Space Grotesk"),
    weight: Weight::Medium,
    ..Font::DEFAULT
};

/// How wide `content` is on one line in `font` at `size` pixels, as a
/// canvas draws it.
pub fn width(content: &str, size: f32, font: Font) -> f32 {
    use iced::advanced::text::Paragraph as _;
    type Paragraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;
    Paragraph::with_text(iced::advanced::Text {
        content,
        bounds: iced::Size::INFINITY,
        size: iced::Pixels(size),
        line_height: iced::widget::text::LineHeight::default(),
        font,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Top,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
    })
    .min_width()
}

/// The texts of a drawing and of a sheet: not the interface but what the
/// drawing says, which its PDF sets in Helvetica. A sans-serif of the
/// system shows them.
pub const DRAWING: Font = Font::DEFAULT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_render;
    use iced::widget::text;
    use iced::{Element, Size, Theme};

    /// How many columns of `picture` hold ink: a pixel darker than the
    /// white it was drawn on.
    fn inked_columns(picture: &test_render::Picture) -> u32 {
        (0..picture.width)
            .filter(|&x| (0..picture.height).any(|y| picture.rgb(x, y)[0] < 200))
            .count() as u32
    }

    fn drawn(font: Font) -> u32 {
        let line: Element<'_, crate::Message> = iced::widget::container(
            text("Project Browser Properties 0123456789")
                .size(14)
                .font(font)
                .color(iced::Color::BLACK),
        )
        .style(|_| iced::widget::container::Style::default().background(iced::Color::WHITE))
        .width(iced::Fill)
        .height(iced::Fill)
        .into();
        inked_columns(&test_render::render(
            line,
            &Theme::Light,
            Size::new(400.0, 30.0),
        ))
    }

    #[test]
    fn every_weight_is_drawn_from_its_own_file() {
        let regular = drawn(REGULAR);
        let medium = drawn(MEDIUM);
        let semibold = drawn(SEMIBOLD);
        let heading = drawn(HEADING);
        assert!(regular > 200, "the text is drawn: {regular}");
        // A heavier weight of Inter is wider; were it not found, the text
        // would fall back to the regular face and be as wide.
        assert!(medium > regular, "{medium} against {regular}");
        assert!(semibold > medium, "{semibold} against {medium}");
        // Space Grotesk is a font of its own.
        assert_ne!(heading, medium);
    }

    /// The sources of the window, by their path under `src`.
    fn sources() -> Vec<(String, String)> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        let mut folders = vec![directory.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let name = path.strip_prefix(&directory).unwrap().display().to_string();
                    sources.push((name, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        sources
    }

    #[test]
    fn every_text_drawn_on_a_canvas_names_its_font() {
        // The font of a canvas text is not the default font of the window:
        // without one of its own a text falls back to a font of the system.
        let mut found = Vec::new();
        let mut texts = 0;
        for (name, source) in sources() {
            if name == "fonts.rs" {
                continue;
            }
            for (at, _) in source.match_indices("Text::default()") {
                texts += 1;
                let literal = source[..at].rfind("Text {").map(|start| &source[start..at]);
                if !source[..at].ends_with("..canvas::")
                    || !literal.is_some_and(|literal| literal.contains("font:"))
                {
                    found.push(format!("{name}: a text without a font at {at}"));
                }
            }
            // A text given as a string takes the default font too.
            for (at, _) in source.match_indices("fill_text(") {
                let argument = source[at + "fill_text(".len()..].trim_start();
                if argument.starts_with('"')
                    || argument.starts_with("format!")
                    || argument.starts_with("tr(")
                {
                    found.push(format!("{name}: a text without a font at {at}"));
                }
            }
        }
        assert!(texts > 25, "{texts}");
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn the_debian_copyright_names_the_fonts_that_ship_with_their_licence() {
        let native = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let copyright = std::fs::read_to_string(native.join("packaging/linux/copyright")).unwrap();
        let folder = native.join("assets/fonts");
        let fonts: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".ttf"))
            .collect();
        assert_eq!(fonts.len(), FILES.len(), "{fonts:?}");
        let mut covered = Vec::new();
        for stanza in copyright.split(
            "

",
        ) {
            let field = |name: &str| {
                stanza
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(str::trim)
            };
            let Some(pattern) =
                field("Files:").and_then(|files| files.strip_prefix("native/assets/fonts/"))
            else {
                continue;
            };
            let (start, end) = pattern.split_once('*').unwrap_or((pattern, ""));
            let named: Vec<&String> = fonts
                .iter()
                .filter(|font| {
                    if pattern.contains('*') {
                        font.starts_with(start) && font.ends_with(end)
                    } else {
                        *font == pattern
                    }
                })
                .collect();
            assert!(!named.is_empty(), "{pattern} names no font that ships");
            covered.extend(named);
            // The holder and the year are those of the licence beside them.
            let licence = field("Comment:")
                .and_then(|comment| {
                    comment
                        .split_whitespace()
                        .find(|word| word.ends_with("-OFL.txt"))
                })
                .expect("the licence file");
            let licence = std::fs::read_to_string(folder.join(licence)).unwrap();
            let holder = field("Copyright:").unwrap();
            assert!(
                licence
                    .lines()
                    .next()
                    .unwrap()
                    .starts_with(&format!("Copyright {holder}")),
                "{holder}"
            );
        }
        covered.sort();
        covered.dedup();
        assert_eq!(covered.len(), fonts.len(), "{covered:?} of {fonts:?}");
    }

    /// Whether the table directory of a TrueType file lists `tag`.
    fn has_table(file: &[u8], tag: &[u8; 4]) -> bool {
        let tables = usize::from(u16::from_be_bytes([file[4], file[5]]));
        (0..tables).any(|index| &file[12 + 16 * index..16 + 16 * index] == tag)
    }

    #[test]
    fn the_fonts_are_static_inter_and_space_grotesk() {
        for (file, (family, weight)) in FILES.into_iter().zip([
            ("Inter", 400),
            ("Inter", 500),
            ("Inter", 600),
            ("Space Grotesk", 500),
        ]) {
            let mut database = iced_tiny_skia::graphics::text::cosmic_text::fontdb::Database::new();
            database.load_font_data(file.to_vec());
            let info = database.faces().next().expect("a face in the file");
            assert_eq!(info.families[0].0, family);
            assert_eq!(info.weight.0, weight);
            // No axes of a variable font, which iced would not apply.
            assert!(has_table(file, b"name") && !has_table(file, b"fvar"));
        }
    }
}
