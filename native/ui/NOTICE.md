# Bundled font notices

The six font binaries in `assets/fonts/` came from the pinned OpenAEC Style Book Tauri template. The typefaces have separate SIL Open Font License 1.1 notices:

| Typeface | Bundled files | License text | Upstream |
| --- | --- | --- | --- |
| Inter | `Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf` | [`licenses/Inter-OFL.txt`](licenses/Inter-OFL.txt) | [rsms/inter](https://github.com/rsms/inter) |
| Space Grotesk | `SpaceGrotesk-Medium.ttf`, `SpaceGrotesk-Bold.ttf` | [`licenses/SpaceGrotesk-OFL.txt`](licenses/SpaceGrotesk-OFL.txt) | [floriankarsten/space-grotesk](https://github.com/floriankarsten/space-grotesk) |
| JetBrains Mono | `JetBrainsMono-Regular.ttf` | [`licenses/JetBrainsMono-OFL.txt`](licenses/JetBrainsMono-OFL.txt) | [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono) |

The upstream license texts were retrieved on 15 October 2026 and are included with the crate. The package's `LICENSE` applies to the OpenAEC crate material, subject to these separate font licenses.

The `microphone.svg` control icon adapts the inline microphone geometry in Open Speech Studio's `src/components/Sidebar.tsx` at commit `4e8aa61`. That source is distributed under Apache License 2.0; the [license text](licenses/Open-Speech-Studio-Apache-2.0.txt) accompanies this package. The icon uses the shared 1.75 px stroke and rounded joins of the OpenAEC control set.

The 32 vectors in `assets/ribbon-icons/` derive from SVG strings in the OpenAEC Style Book Tauri ribbon at commit `dfdcd415b26604081b1699560621d3e9ae7ea6bf`. Its repository and this crate use CC BY-SA 4.0. The extraction adds the SVG namespace without changing the source geometry. Viewer and Extensions icons are excluded because the pinned Tauri ribbon does not render those tabs; the Report eye icon used for Save As is excluded in favor of the existing `Icon::SaveAs`.

The canonical `settings.svg` uses the same pinned Style Book Tauri gear geometry, with the shared 1.75 px stroke. The previous slider artwork remains available as `controls.svg` for view and property controls. Both meanings are kept separate in the Rust `Icon` enum.
