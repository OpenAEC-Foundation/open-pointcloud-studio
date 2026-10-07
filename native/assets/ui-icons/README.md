# Icons of the window

The icons of Open Pointcloud Studio, drawn as the OpenAEC style book draws its
icons: lines in one colour, `currentColor`, on a square of 24, two units wide
with round ends and corners. The window gives them the colour of the text
they stand with, and the accent of the theme while their button is on
(`desktop/src/icons.rs`). A test holds that no file names another colour.

Taken from the style book (OpenAEC-Foundation/OpenAEC-style-book at commit
dfdcd41), where the code is licensed under the MIT licence; the licence, with
the copyright line of the OpenAEC Foundation, is `OpenAEC-icons-MIT.txt`:

- from `packages/ribbon/src/icons.ts`: `import.svg` and `export.svg` (the
  import and export icons of its IFC tab), `folder.svg` (its project icon,
  with three points added), `fit.svg`, `measure_distance.svg`, `eye_dome.svg`
  (its orbit icon) and `model.svg` (its load icon) of its 3D viewer tab,
  `line.svg` (its arrow), `undo.svg`, `redo.svg`, `duplicate.svg` (its copy
  icon), `rename.svg` (its pencil), `settings.svg` (its gear),
  `drawing_file.svg` (its new report icon) and `sheet.svg` (its report
  template icon);
- from `project-templates/Tauri+React/src/components/TitleBar.tsx`:
  `save.svg`, and the glyphs of the buttons of the window,
  `window_minimize.svg`, `window_maximize.svg`, `window_restore.svg` and
  `window_close.svg`, which are drawn on a square of 10 as the template
  draws them.

The other icons are drawn for this application in the same style.
