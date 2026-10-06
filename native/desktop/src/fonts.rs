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
