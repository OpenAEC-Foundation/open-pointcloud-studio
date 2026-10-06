# Bundled native UI fonts

The fonts of the OpenAEC style book, as its desktop template carries them
(`project-templates/Tauri+React/src-tauri/tenants/openaec_foundation/fonts/`
at commit dfdcd41): static files, one for each weight the interface uses.

- `Inter-Regular.ttf` (400), `Inter-Medium.ttf` (500) and `Inter-SemiBold.ttf` (600): the font of the interface. Their license is `Inter-OFL.txt` (SIL Open Font License 1.1).
- `SpaceGrotesk-Medium.ttf` (500): the font of headings. Its license is `SpaceGrotesk-OFL.txt` (SIL Open Font License 1.1).

These font files are embedded by the Rust desktop binary (`desktop/src/fonts.rs`). No browser or web font loader is used.
