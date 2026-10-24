//! Embedded fonts ensure that host machine fonts do not change the shell.
use iced::{font::Weight, Font};
pub const BODY: Font = Font::with_name("Inter");
pub const MEDIUM: Font = Font { weight: Weight::Medium, ..BODY };
pub const STRONG: Font = Font { weight: Weight::Semibold, ..BODY };
pub const HEADING: Font = Font { weight: Weight::Medium, ..Font::with_name("Space Grotesk") };
pub const MONO: Font = Font::with_name("JetBrains Mono");
pub const FILES: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
];
