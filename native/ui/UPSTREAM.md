# OpenAEC UI crate snapshot

This portable snapshot comes from the sibling `OpenAEC-app-kopie` Style Repo.
The private [OpenAEC-Style-Repo](https://github.com/OpenAEC-Foundation/OpenAEC-Style-Repo)
is the source of truth. The desktop application uses this local workspace crate
so a public checkout builds without private GitHub access or another directory
on the developer's machine. Run `python3 scripts/check_portable_ui.py` in the
Style Repo before updating this snapshot.

The integration uses its canonical semantic icons for titlebar commands,
its bundled Inter and Space Grotesk fonts, 77 common color-token fields in
each of the five themes, and the shared ribbon-button state styling. Pointcloud keeps local colors for its 3D scene,
drawings, sheets, result categories, shadows, and tooltips. The domain ribbon
and panels remain in the application while they are migrated to the shared
shell. Keep the source snapshot and license in sync when updating it.
The snapshot includes the crate's font license texts and the Apache-2.0
notice for the source-reviewed Speech microphone icon. It also carries the
32 source-reviewed Tauri Style Book `RibbonIcon` vectors as a typed API;
Pointcloud does not yet call those template-specific commands. Their CC BY-SA
provenance is recorded in `NOTICE.md`. The Style Repo's
`scripts/check_portable_ui.py` verifies these files alongside code and assets.

The desktop now uses the source Tauri shell heights from `openaec_ui::model::metrics`:
32 px title, 28 px ribbon tabs, 94 px ribbon tools, 30 px document strip and
22 px status. The document strip spans the explorer, canvas and properties.
`openaec_ui::frame::FrameSlots` now composes the complete Pointcloud window,
including File view. Pointcloud still owns the commands, adaptive ribbon groups
and 3D drawing; those domain behaviors are not replaced by the crate's demo shell.
The software-rendered [Light](../design/pointcloud-shell-light.png) and
[Forge](../design/pointcloud-shell-forge.png) views are review baselines.
Quick-access actions on the title strip now use `widgets::title_action` from
the shared crate for their 30 × 26 px geometry, hover state and tooltip.
Pointcloud still supplies its actual command, translated label and icon.
The shared settings action now uses the reviewed gear icon; `Icon::Controls`
retains the former slider artwork for view and property controls. Light and
Forge shell and settings baselines were regenerated for this semantic change.
The settings dialog now uses `widgets::settings_modal` for the pinned Tauri
template's 560 × 500 px card, 120 px sidebar, header, footer and inert backdrop.
Pointcloud owns the language, theme, CAD viewer and save/cancel behavior.
Its General, Appearance and About navigation now uses the crate's square 11 px
`widgets::settings_tab`, including the shared hover and selected theme tokens.
The Appearance page also uses `widgets::settings_section_heading` and
`widgets::settings_theme_picker`: the shared 200 px trigger expands the five
source-colored 14 px swatch rows. Selection previews the theme and closes the
anchored overlay menu; an outside click closes it without moving the form.
Cancel still restores the original theme.
Reset now opens the crate's 340 px `widgets::settings_reset_confirmation`.
Confirming resets the preview, while Cancel and Escape dismiss the prompt.
The open theme menu closes before the prompt appears.
The native software review renderer draws widget overlays as well as the base
window, so its settings screenshots include the expanded menu.
The software renders are in `native/design/pointcloud-settings-{light,forge}.png`.
The second modal is rendered in `native/design/pointcloud-settings-reset-forge.png`.
Ribbon, panel and pointer-following tips now use the crate's
`widgets::themed_tooltip` for consistent text size, padding and width; the
pointer variant keeps its original zero-gap placement.
The six X/Y/Z section-box sliders and editable layout viewport fields now
use `widgets::property_grid_control_with`. Pointcloud still owns their values
and messages; the crate supplies the 28 px rows and aligned label/control
columns. Native Light and Forge captures are in
`native/design/pointcloud-section-box-{light,forge}.png`.
The 2D plan/elevation/section dialog now uses the crate's 480 px
`widgets::dialog_modal` for its square border, 30 px header, 44 px footer and
inert overlay. Its form and Create action remain Pointcloud-owned. Light and
Forge captures are in `native/design/pointcloud-sheet-dialog-{light,forge}.png`.
The modal header now uses the pinned Style Book's vertical theme gradient,
and the card uses its theme-specific 0 × 4 px, 16 px-blur shadow. Both
modal variants now support bounded header dragging and a 90% viewport-height
cap; tests cover short windows and the close-button exclusion.
Legacy OpenCAD command SVGs with fixed dark strokes are tinted to the shared
text color on dark themes. They need final per-icon vector harmonization; the
current tint keeps every command legible during that work.

A field-by-field comparison found 378 values identical to the old Pointcloud
tokens and seven deliberate changes to the Style Repo values: the five ribbon
group labels and Light-theme status text/label. These visible differences
align Pointcloud with the common Tauri-derived palette.

The portable core now has 30 semantic controls. Print, Save As, and window
Restore were compared with pinned Geotechniek UI usage at 16, 24, and 32 px
in Light and Forge before entering the crate. The source-design review and
semantic crosswalk remain in the Style Repo; the three approved SVGs and Rust
`Icon` variants are byte-for-byte synchronized here. Three reviewed Geotechniek
placement symbols (Sondering, Boring and Sonderingsraster) now live separately
in `DomainIcon` with dedicated 16 px artwork. The ambiguous actions and other
seven Geotechniek domain meanings remain design work. Five additional
CAD editing meanings and three drawing/frame meanings are included as crate assets for future use but are not
yet Pointcloud commands. Dimension, Note, Move and Line now use the same reviewed domain icons in Pointcloud and Geotechniek;
the existing Pointcloud actions still own their behavior.

The shared crate also carries fifteen PDF ribbon domain meanings, including
Proofread and Split Document from public PDF main `ae1d787`. The earlier PDF
meanings were selected after reviewing seven source SVG collisions; the two
new commands have separate 16 px optical variants and a light/dark review.
The package now exposes 36 `DomainIcon` variants; Pointcloud keeps its own
commands and does not invoke these PDF-only meanings.
The original PDF Studio JavaScript application has not been migrated to the
shared crate. The Style Repo keeps the source mapping and native comparison.

The sidebar-first frame API is synchronized with the Style Repo: `SidebarFrameProfile::WORKSPACE_COMPOSER` preserves the 32/220/22 px source geometry, and `ChromeMode::SidebarCompact` exposes a raw navigation slot without a ribbon or document band. Pointcloud continues to use its existing ribbon shell; the new variant is available to future sidebar-first OpenAEC tools. The native Pointcloud desktop package still passes `cargo check` with this shared API.

The shared `ChromeMode::ThreePaneFixed` variant now accepts exact left and right pane widths for source-specific review screens. The Style Repo uses it for the Open Calc Studio budget screen at 320/240 px, based on the pinned Dutch capture. Pointcloud continues to use its existing resizable ribbon workspace.
