# License and bundled material

The OpenAEC crate code is licensed under the GNU Lesser General Public License version 3 only (`LGPL-3.0-only`), as for all OpenAEC Foundation repositories. [`LICENSE.md`](LICENSE.md) holds the LGPL 3.0 text followed by the GNU GPL 3.0 text it builds on. The material below keeps its own license; the `license` field in `Cargo.toml` names all four.

| Material | License | Text |
| --- | --- | --- |
| Crate code, including library 0.4 in `src/design/` | LGPL-3.0-only | [`LICENSE.md`](LICENSE.md) |
| The six fonts in `assets/fonts/` | SIL Open Font License 1.1 | `licenses/*-OFL.txt` |
| `assets/icons/microphone.svg` | Apache License 2.0 | [`licenses/Open-Speech-Studio-Apache-2.0.txt`](licenses/Open-Speech-Studio-Apache-2.0.txt) |
| Style Book vectors: `assets/ribbon-icons/`, `src/ribbon_icons.rs` and `assets/icons/settings.svg` | CC BY-SA 4.0 | [`licenses/Style-Book-CC-BY-SA-4.0.txt`](licenses/Style-Book-CC-BY-SA-4.0.txt) |

The Style Book vectors belong to the earlier prototype. Library 0.4 (`src/design/`) does not use them.

## Bundled font notices

The six font binaries in `assets/fonts/` came from the pinned OpenAEC Style Book Tauri template. The typefaces have separate SIL Open Font License 1.1 notices:

| Typeface | Bundled files | License text | Upstream |
| --- | --- | --- | --- |
| Inter | `Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf` | [`licenses/Inter-OFL.txt`](licenses/Inter-OFL.txt) | [rsms/inter](https://github.com/rsms/inter) |
| Space Grotesk | `SpaceGrotesk-Medium.ttf`, `SpaceGrotesk-Bold.ttf` | [`licenses/SpaceGrotesk-OFL.txt`](licenses/SpaceGrotesk-OFL.txt) | [floriankarsten/space-grotesk](https://github.com/floriankarsten/space-grotesk) |
| JetBrains Mono | `JetBrainsMono-Regular.ttf` | [`licenses/JetBrainsMono-OFL.txt`](licenses/JetBrainsMono-OFL.txt) | [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono) |

The upstream license texts were retrieved on 15 October 2026 and are included with the crate. The fonts keep these licenses; `LICENSE.md` does not apply to them.

The `microphone.svg` control icon adapts the inline microphone geometry in Open Speech Studio's `src/components/Sidebar.tsx` at commit `4e8aa61`. That source is distributed under Apache License 2.0; the [license text](licenses/Open-Speech-Studio-Apache-2.0.txt) accompanies this package. The icon uses the shared 1.75 px stroke and rounded joins of the OpenAEC control set.

The 32 vectors in `assets/ribbon-icons/` derive from SVG strings in the OpenAEC Style Book Tauri ribbon at commit `dfdcd415b26604081b1699560621d3e9ae7ea6bf`. Its repository uses CC BY-SA 4.0, which requires derived material to keep that license, so these vectors and the generated `src/ribbon_icons.rs` stay under CC BY-SA 4.0. The extraction adds the SVG namespace without changing the source geometry. Viewer and Extensions icons are excluded because the pinned Tauri ribbon does not render those tabs; the Report eye icon used for Save As is excluded in favor of the existing `Icon::SaveAs`.

The canonical `settings.svg` uses the same pinned Style Book Tauri gear geometry, with the shared 1.75 px stroke, and is CC BY-SA 4.0 for the same reason. The previous slider artwork remains available as `controls.svg` for view and property controls. Both meanings are kept separate in the Rust `Icon` enum.
