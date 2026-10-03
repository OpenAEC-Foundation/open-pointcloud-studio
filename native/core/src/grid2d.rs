//! A 2D count and occupancy grid on a plane, and the outlines of what it
//! holds: threshold, morphological closing and opening, removal of small
//! components, hole filling, contour tracing with holes, simplification,
//! area and perimeter. The cut of a 2D drawing and the boundary of a detected
//! face are both built on it.
//!
//! Plane coordinates are `[u, v]` with u to the right and v up. Cell
//! `(x, y)` covers `origin + [x, y] * cell` up to `origin + [x + 1, y + 1] *
//! cell`, and cells are stored row by row from `y = 0`.

use crate::LoadError;

/// The most cells a grid may have, as a guard against a mistaken extent.
pub const MAX_GRID_CELLS: usize = 1 << 26;

/// Which neighbours make cells one component: those sharing an edge, or
/// those sharing a corner as well. Empty cells always connect the other way
/// round, so that a contour never crosses itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Four,
    Eight,
}

impl Connectivity {
    fn opposite(self) -> Self {
        match self {
            Self::Four => Self::Eight,
            Self::Eight => Self::Four,
        }
    }

    fn neighbours(self) -> &'static [(i64, i64)] {
        static ALL: [(i64, i64); 8] = [
            (1, 0),
            (0, 1),
            (-1, 0),
            (0, -1),
            (1, 1),
            (-1, 1),
            (-1, -1),
            (1, -1),
        ];
        match self {
            Self::Four => &ALL[..4],
            Self::Eight => &ALL,
        }
    }
}

/// Where a grid lies on its plane: the corner of cell (0, 0), the cell size
/// and the number of cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridFrame {
    pub origin: [f64; 2],
    pub cell: f64,
    pub width: u32,
    pub height: u32,
}

impl GridFrame {
    pub fn new(origin: [f64; 2], cell: f64, width: u32, height: u32) -> Result<Self, LoadError> {
        if !origin.iter().all(|value| value.is_finite()) || !cell.is_finite() || cell <= 0.0 {
            return Err(LoadError::InvalidData(
                "a grid needs a finite origin and a positive cell size".into(),
            ));
        }
        if width == 0 || height == 0 || width as u64 * height as u64 > MAX_GRID_CELLS as u64 {
            return Err(LoadError::InvalidData(format!(
                "a grid needs between 1 and {MAX_GRID_CELLS} cells"
            )));
        }
        Ok(Self {
            origin,
            cell,
            width,
            height,
        })
    }

    /// The grid over the rectangle from `min` to `max`, both corners
    /// included. A cell size that would need more than `max_cells` cells is
    /// doubled until the grid fits; the size used is in the result.
    pub fn covering(
        min: [f64; 2],
        max: [f64; 2],
        cell: f64,
        max_cells: usize,
    ) -> Result<Self, LoadError> {
        if !min.iter().chain(&max).all(|value| value.is_finite())
            || min[0] > max[0]
            || min[1] > max[1]
            || !cell.is_finite()
            || cell <= 0.0
            || max_cells == 0
        {
            return Err(LoadError::InvalidData(
                "a grid needs a finite extent and a positive cell size".into(),
            ));
        }
        let limit = max_cells.min(MAX_GRID_CELLS) as f64;
        let mut cell = cell;
        loop {
            // One more than fits, so that a point on the far edge has a cell.
            let width = ((max[0] - min[0]) / cell).floor() + 1.0;
            let height = ((max[1] - min[1]) / cell).floor() + 1.0;
            if width * height <= limit {
                return Self::new(min, cell, width as u32, height as u32);
            }
            cell *= 2.0;
            if !cell.is_finite() {
                return Err(LoadError::InvalidData(
                    "the grid extent is too large".into(),
                ));
            }
        }
    }

    pub fn cells(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Position of cell `(x, y)` in the row-by-row cell storage.
    pub fn index(&self, x: u32, y: u32) -> usize {
        y as usize * self.width as usize + x as usize
    }

    /// The cell that holds a plane position, or nothing outside the grid.
    pub fn cell_of(&self, uv: [f64; 2]) -> Option<[u32; 2]> {
        let x = ((uv[0] - self.origin[0]) / self.cell).floor();
        let y = ((uv[1] - self.origin[1]) / self.cell).floor();
        // Written so that a NaN fails the test.
        (x >= 0.0 && y >= 0.0 && x < self.width as f64 && y < self.height as f64)
            .then_some([x as u32, y as u32])
    }

    pub fn cell_center(&self, x: u32, y: u32) -> [f64; 2] {
        [
            self.origin[0] + (x as f64 + 0.5) * self.cell,
            self.origin[1] + (y as f64 + 0.5) * self.cell,
        ]
    }

    /// Plane position of a cell corner; corner `[x, y]` is the lower left
    /// corner of cell `(x, y)`.
    pub fn vertex(&self, corner: [i32; 2]) -> [f64; 2] {
        [
            self.origin[0] + corner[0] as f64 * self.cell,
            self.origin[1] + corner[1] as f64 * self.cell,
        ]
    }

    /// The corner opposite the origin.
    pub fn max(&self) -> [f64; 2] {
        self.vertex([self.width as i32, self.height as i32])
    }

    pub fn cell_area(&self) -> f64 {
        self.cell * self.cell
    }

    /// The smallest number of cells that covers an area.
    pub fn cells_for_area(&self, area: f64) -> usize {
        (area / self.cell_area()).ceil().max(0.0) as usize
    }

    /// A length in whole cells, rounded to the nearest.
    pub fn cells_for_length(&self, length: f64) -> u32 {
        (length / self.cell).round().max(0.0) as u32
    }
}

/// How many points fell in each cell.
#[derive(Debug, Clone, PartialEq)]
pub struct CountGrid {
    frame: GridFrame,
    counts: Vec<u32>,
}

impl CountGrid {
    pub fn new(frame: GridFrame) -> Self {
        Self {
            frame,
            counts: vec![0; frame.cells()],
        }
    }

    pub fn frame(&self) -> GridFrame {
        self.frame
    }

    /// Counts row by row, see `GridFrame::index`.
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }

    pub fn count(&self, x: u32, y: u32) -> u32 {
        self.counts[self.frame.index(x, y)]
    }

    /// Count a point. Returns the position of its cell in the storage, so
    /// that a caller can keep further sums per cell beside the counts, or
    /// nothing for a point outside the grid.
    pub fn add(&mut self, uv: [f64; 2]) -> Option<usize> {
        let [x, y] = self.frame.cell_of(uv)?;
        let index = self.frame.index(x, y);
        self.counts[index] = self.counts[index].saturating_add(1);
        Some(index)
    }

    /// Cells with at least one point.
    pub fn occupied(&self) -> usize {
        self.counts.iter().filter(|count| **count > 0).count()
    }

    /// The median count of the cells that hold a point, zero when none does.
    /// It tells whether a threshold suits the density of the cloud.
    pub fn median_occupied(&self) -> u32 {
        let mut counts: Vec<u32> = self
            .counts
            .iter()
            .copied()
            .filter(|count| *count > 0)
            .collect();
        if counts.is_empty() {
            return 0;
        }
        let middle = counts.len() / 2;
        *counts.select_nth_unstable(middle).1
    }

    /// The same counts in cells of twice the size, for a cloud too sparse
    /// for the cell size asked.
    pub fn coarsened(&self) -> Self {
        let frame = GridFrame {
            cell: self.frame.cell * 2.0,
            width: self.frame.width.div_ceil(2),
            height: self.frame.height.div_ceil(2),
            ..self.frame
        };
        let mut counts = vec![0u32; frame.cells()];
        for y in 0..self.frame.height {
            for x in 0..self.frame.width {
                let target = &mut counts[frame.index(x / 2, y / 2)];
                *target = target.saturating_add(self.count(x, y));
            }
        }
        Self { frame, counts }
    }

    /// The cells with at least `min_count` points.
    pub fn threshold(&self, min_count: u32) -> Mask {
        Mask {
            frame: self.frame,
            cells: self
                .counts
                .iter()
                .map(|count| *count >= min_count)
                .collect(),
        }
    }
}

/// The components of a mask: a label per cell and the size of each.
#[derive(Debug, Clone, PartialEq)]
pub struct Components {
    /// Per cell, row by row: zero for an empty cell, otherwise the number of
    /// its component. Components are numbered from one in the order their
    /// first cell appears.
    pub labels: Vec<u32>,
    /// Cells in each component; component `n` is at `n - 1`.
    pub sizes: Vec<u32>,
}

impl Components {
    pub fn count(&self) -> usize {
        self.sizes.len()
    }
}

/// Occupied and empty cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    frame: GridFrame,
    cells: Vec<bool>,
}

impl Mask {
    /// A mask with every cell empty.
    pub fn new(frame: GridFrame) -> Self {
        Self {
            frame,
            cells: vec![false; frame.cells()],
        }
    }

    /// A mask whose cell `(x, y)` is occupied where `occupied(x, y)` says so.
    pub fn from_fn(frame: GridFrame, mut occupied: impl FnMut(u32, u32) -> bool) -> Self {
        let mut cells = Vec::with_capacity(frame.cells());
        for y in 0..frame.height {
            for x in 0..frame.width {
                cells.push(occupied(x, y));
            }
        }
        Self { frame, cells }
    }

    pub fn frame(&self) -> GridFrame {
        self.frame
    }

    /// Cells row by row, see `GridFrame::index`.
    pub fn cells(&self) -> &[bool] {
        &self.cells
    }

    /// Whether a cell is occupied; a cell outside the grid is empty.
    pub fn get(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && x < self.frame.width as i64
            && y < self.frame.height as i64
            && self.cells[y as usize * self.frame.width as usize + x as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, occupied: bool) {
        let index = self.frame.index(x, y);
        self.cells[index] = occupied;
    }

    /// Occupied cells.
    pub fn count(&self) -> usize {
        self.cells.iter().filter(|cell| **cell).count()
    }

    /// Occupy every cell within `radius_x` cells to the side and `radius_y`
    /// cells up or down of an occupied one. What would fall outside the grid
    /// is lost.
    pub fn dilate(&mut self, radius_x: u32, radius_y: u32) {
        let (width, height) = (self.frame.width as usize, self.frame.height as usize);
        let rows = window_pass(&self.cells, width, height, radius_x as usize, true, false);
        self.cells = window_pass(&rows, width, height, radius_y as usize, false, false);
    }

    /// Keep only the cells whose whole neighbourhood of `radius_x` by
    /// `radius_y` cells is occupied. Outside the grid counts as empty.
    pub fn erode(&mut self, radius_x: u32, radius_y: u32) {
        let (width, height) = (self.frame.width as usize, self.frame.height as usize);
        let rows = window_pass(&self.cells, width, height, radius_x as usize, true, true);
        self.cells = window_pass(&rows, width, height, radius_y as usize, false, true);
    }

    /// Morphological closing with a square of `2 * radius + 1` cells: fills
    /// every gap of at most `2 * radius` cells between occupied cells, and
    /// leaves a wider gap as wide as it was. The square keeps corners sharp.
    /// Nothing grows outward, so the grid needs no margin.
    pub fn close(&mut self, radius: u32) {
        if radius == 0 {
            return;
        }
        let r = radius as usize;
        let (width, height) = (self.frame.width as usize, self.frame.height as usize);
        // The dilated shape needs room: eroding at the edge of the grid
        // itself would let shapes there grow.
        let (wide, high) = (width + 2 * r, height + 2 * r);
        let mut padded = vec![false; wide * high];
        for y in 0..height {
            padded[(y + r) * wide + r..(y + r) * wide + r + width]
                .copy_from_slice(&self.cells[y * width..(y + 1) * width]);
        }
        let padded = window_pass(&padded, wide, high, r, true, false);
        let padded = window_pass(&padded, wide, high, r, false, false);
        let padded = window_pass(&padded, wide, high, r, true, true);
        let padded = window_pass(&padded, wide, high, r, false, true);
        for y in 0..height {
            self.cells[y * width..(y + 1) * width]
                .copy_from_slice(&padded[(y + r) * wide + r..(y + r) * wide + r + width]);
        }
    }

    /// Morphological opening with a square of `2 * radius + 1` cells: removes
    /// everything narrower than that square and keeps the rest as it was.
    pub fn open(&mut self, radius: u32) {
        self.erode(radius, radius);
        self.dilate(radius, radius);
    }

    /// Label the connected components of the occupied cells.
    pub fn components(&self, connectivity: Connectivity) -> Components {
        label_cells(&self.cells, true, self.frame, connectivity)
    }

    /// Empty every component of fewer than `min_cells` cells. Returns how
    /// many components were removed.
    pub fn remove_small_components(
        &mut self,
        min_cells: usize,
        connectivity: Connectivity,
    ) -> usize {
        let components = self.components(connectivity);
        let small: Vec<bool> = components
            .sizes
            .iter()
            .map(|size| (*size as usize) < min_cells)
            .collect();
        for (cell, label) in self.cells.iter_mut().zip(&components.labels) {
            if *label > 0 && small[*label as usize - 1] {
                *cell = false;
            }
        }
        small.iter().filter(|small| **small).count()
    }

    /// Occupy every hole of fewer than `min_cells` cells. A hole is a group
    /// of empty cells that does not reach the edge of the grid; `connectivity`
    /// is that of the occupied cells, as given to `trace`. Returns how many
    /// holes were filled.
    pub fn fill_small_holes(&mut self, min_cells: usize, connectivity: Connectivity) -> usize {
        let empty = label_cells(&self.cells, false, self.frame, connectivity.opposite());
        let (width, height) = (self.frame.width as usize, self.frame.height as usize);
        let mut fill: Vec<bool> = empty
            .sizes
            .iter()
            .map(|size| (*size as usize) < min_cells)
            .collect();
        let mut open = |index: usize| {
            if empty.labels[index] > 0 {
                fill[empty.labels[index] as usize - 1] = false;
            }
        };
        for x in 0..width {
            open(x);
            open((height - 1) * width + x);
        }
        for y in 0..height {
            open(y * width);
            open(y * width + width - 1);
        }
        for (cell, label) in self.cells.iter_mut().zip(&empty.labels) {
            if *label > 0 && fill[*label as usize - 1] {
                *cell = true;
            }
        }
        fill.iter().filter(|fill| **fill).count()
    }

    /// The outline of every component: its outer ring and the rings of its
    /// holes, along the cell edges. Regions come in the order of
    /// `components`. A region inside the hole of another is a region of its
    /// own.
    pub fn trace(&self, connectivity: Connectivity) -> Vec<CellRegion> {
        let (width, height) = (self.frame.width as i64, self.frame.height as i64);
        let components = self.components(connectivity);
        let mut regions: Vec<CellRegion> = (0..components.count())
            .map(|_| CellRegion {
                outer: Vec::new(),
                holes: Vec::new(),
            })
            .collect();
        // Per corner, one bit for each direction in which an edge that was
        // followed leaves it.
        let stride = width as usize + 1;
        let mut followed = vec![0u8; stride * (height as usize + 1)];
        for y in 0..height {
            for x in 0..width {
                if !self.get(x, y) {
                    continue;
                }
                // The four edges that have this cell on their left.
                for start in [(x, y, 0), (x + 1, y, 1), (x + 1, y + 1, 2), (x, y + 1, 3)] {
                    let (i, j, heading) = start;
                    let (_, right) = edge_sides(i, j, heading);
                    if self.get(right.0, right.1)
                        || followed[j as usize * stride + i as usize] & (1 << heading) != 0
                    {
                        continue;
                    }
                    let ring = self.follow(start, connectivity, &mut followed);
                    let region =
                        &mut regions[components.labels[(y * width + x) as usize] as usize - 1];
                    if twice_cell_area(&ring) > 0 {
                        region.outer = ring;
                    } else {
                        region.holes.push(ring);
                    }
                }
            }
        }
        regions
    }

    /// `trace` in plane coordinates.
    pub fn regions(&self, connectivity: Connectivity) -> Vec<Region> {
        self.trace(connectivity)
            .iter()
            .map(|region| region.to_plane(&self.frame))
            .collect()
    }

    /// Walk the boundary that starts with one edge, keeping the occupied
    /// cells on the left, until it closes. Only corners are kept.
    fn follow(
        &self,
        start: (i64, i64, u8),
        connectivity: Connectivity,
        followed: &mut [u8],
    ) -> Vec<[i32; 2]> {
        const STEP: [(i64, i64); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
        let stride = self.frame.width as usize + 1;
        let (mut i, mut j, mut heading) = start;
        let mut ring = Vec::new();
        loop {
            followed[j as usize * stride + i as usize] |= 1 << heading;
            i += STEP[heading as usize].0;
            j += STEP[heading as usize].1;
            let (ahead_left, ahead_right) = edge_sides(i, j, heading);
            let (left, right) = ((heading + 1) % 4, (heading + 3) % 4);
            let next = match (
                self.get(ahead_left.0, ahead_left.1),
                self.get(ahead_right.0, ahead_right.1),
            ) {
                (true, true) => right,
                (true, false) => heading,
                // Two occupied cells touch at this corner only. They are one
                // component when corners connect, so the walk crosses over
                // to the other cell; otherwise it stays with its own.
                (false, true) if connectivity == Connectivity::Eight => right,
                (false, _) => left,
            };
            if next != heading {
                ring.push([i as i32, j as i32]);
            }
            heading = next;
            if (i, j, heading) == start {
                break;
            }
        }
        // The walk ends where it began; a ring that begins at a corner lists
        // that corner first.
        if ring.last() == Some(&[start.0 as i32, start.1 as i32]) {
            ring.rotate_right(1);
        }
        ring
    }
}

/// The cells to the left and to the right of the edge that leaves corner
/// `(i, j)` in a direction: 0 is right, 1 up, 2 left and 3 down.
fn edge_sides(i: i64, j: i64, heading: u8) -> ((i64, i64), (i64, i64)) {
    match heading {
        0 => ((i, j), (i, j - 1)),
        1 => ((i - 1, j), (i, j)),
        2 => ((i - 1, j - 1), (i - 1, j)),
        _ => ((i, j - 1), (i - 1, j - 1)),
    }
}

/// One pass of a box filter along the rows or along the columns: a cell is
/// set when the `radius` cells to both sides of it and the cell itself hold
/// at least one set cell, or with `all` only set cells. Cells outside the
/// grid count as empty.
fn window_pass(
    cells: &[bool],
    width: usize,
    height: usize,
    radius: usize,
    along_rows: bool,
    all: bool,
) -> Vec<bool> {
    if radius == 0 {
        return cells.to_vec();
    }
    let (lines, length, line_step, cell_step) = if along_rows {
        (height, width, width, 1)
    } else {
        (width, height, 1, width)
    };
    let need = if all { 2 * radius + 1 } else { 1 };
    let mut result = vec![false; cells.len()];
    for line in 0..lines {
        let at = |position: usize| line * line_step + position * cell_step;
        let mut inside = (0..radius.min(length))
            .filter(|position| cells[at(*position)])
            .count();
        for position in 0..length {
            if position + radius < length && cells[at(position + radius)] {
                inside += 1;
            }
            if position > radius && cells[at(position - radius - 1)] {
                inside -= 1;
            }
            result[at(position)] = inside >= need;
        }
    }
    result
}

/// Label the connected groups of the cells that equal `value`.
fn label_cells(
    cells: &[bool],
    value: bool,
    frame: GridFrame,
    connectivity: Connectivity,
) -> Components {
    let (width, height) = (frame.width as i64, frame.height as i64);
    let mut labels = vec![0u32; cells.len()];
    let mut sizes = Vec::new();
    let mut pending = Vec::new();
    for start in 0..cells.len() {
        if cells[start] != value || labels[start] != 0 {
            continue;
        }
        let label = sizes.len() as u32 + 1;
        let mut size = 0u32;
        labels[start] = label;
        pending.push(start);
        while let Some(cell) = pending.pop() {
            size += 1;
            let (x, y) = (cell as i64 % width, cell as i64 / width);
            for (dx, dy) in connectivity.neighbours() {
                let (x, y) = (x + dx, y + dy);
                if x < 0 || y < 0 || x >= width || y >= height {
                    continue;
                }
                let neighbour = (y * width + x) as usize;
                if cells[neighbour] == value && labels[neighbour] == 0 {
                    labels[neighbour] = label;
                    pending.push(neighbour);
                }
            }
        }
        sizes.push(size);
    }
    Components { labels, sizes }
}

/// Twice the signed area of a ring of cell corners, in cells.
fn twice_cell_area(ring: &[[i32; 2]]) -> i64 {
    (0..ring.len())
        .map(|index| {
            let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
            a[0] as i64 * b[1] as i64 - b[0] as i64 * a[1] as i64
        })
        .sum()
}

/// The outline of one component in cell corners. A ring is closed: its last
/// corner joins its first. The outer ring runs counter-clockwise and the
/// hole rings clockwise, so the occupied cells are always on the left.
/// Where two cells of the component touch at a corner only, a ring passes
/// through that corner twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRegion {
    pub outer: Vec<[i32; 2]>,
    pub holes: Vec<Vec<[i32; 2]>>,
}

impl CellRegion {
    /// Cells of the component, which is the area inside its outer ring less
    /// its holes.
    pub fn cells(&self) -> u64 {
        let twice = twice_cell_area(&self.outer)
            + self
                .holes
                .iter()
                .map(|hole| twice_cell_area(hole))
                .sum::<i64>();
        (twice / 2).max(0) as u64
    }

    pub fn to_plane(&self, frame: &GridFrame) -> Region {
        let ring = |ring: &Vec<[i32; 2]>| ring.iter().map(|corner| frame.vertex(*corner)).collect();
        Region {
            outer: ring(&self.outer),
            holes: self.holes.iter().map(ring).collect(),
        }
    }
}

/// An area on the plane: a closed outer ring, counter-clockwise, and the
/// closed rings of its holes, clockwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub outer: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
}

impl Region {
    /// Area inside the outer ring less the holes.
    pub fn area(&self) -> f64 {
        ring_signed_area(&self.outer).abs()
            - self
                .holes
                .iter()
                .map(|hole| ring_signed_area(hole).abs())
                .sum::<f64>()
    }

    /// Length of the outer ring and of every hole ring together.
    pub fn perimeter(&self) -> f64 {
        ring_perimeter(&self.outer)
            + self
                .holes
                .iter()
                .map(|hole| ring_perimeter(hole))
                .sum::<f64>()
    }

    /// Whether a position lies in the area: inside the outer ring and in no
    /// hole.
    pub fn contains(&self, uv: [f64; 2]) -> bool {
        ring_contains(&self.outer, uv) && !self.holes.iter().any(|hole| ring_contains(hole, uv))
    }

    /// The area with every ring reduced to straight segments that stay
    /// within `tolerance` of it, see `simplify_ring_indices`. A hole that is
    /// left with fewer than three corners is dropped.
    ///
    /// The result bounds an area as this region does: no ring comes to cross
    /// itself or another, and every hole stays inside the outer ring. A part
    /// that is thinner than the tolerance, such as a wall of which one face
    /// was scanned, therefore keeps corners that the tolerance alone would
    /// have removed. Rings that touch at a corner may come apart there.
    ///
    /// Every region is reduced on its own: two regions of one mask that lie
    /// nearer to each other than the tolerance are not kept apart.
    pub fn simplified(&self, tolerance: f64) -> Self {
        let Ok(kept) = self.kept_corners(tolerance, &mut || Ok(())) else {
            return self.clone();
        };
        let ring = |ring: &[[f64; 2]], kept: &[usize]| -> Vec<[f64; 2]> {
            kept.iter().map(|index| ring[*index]).collect()
        };
        Self {
            outer: ring(&self.outer, &kept[0]),
            holes: self
                .holes
                .iter()
                .zip(&kept[1..])
                .map(|(hole, kept)| ring(hole, kept))
                .filter(|hole| hole.len() >= 3)
                .collect(),
        }
    }

    /// The positions of the corners that `simplified` keeps, ascending per
    /// ring: first those of the outer ring, then those of every hole in the
    /// order of `holes`. They let a caller find the stretch of a ring that
    /// each straight segment replaces.
    ///
    /// `proceed` is called before every stretch that is examined. An error
    /// from it, such as `LoadError::Cancelled`, stops the work and is
    /// returned: a ring of very many corners can take long.
    pub fn kept_corners(
        &self,
        tolerance: f64,
        proceed: &mut dyn FnMut() -> Result<(), LoadError>,
    ) -> Result<Vec<Vec<usize>>, LoadError> {
        let rings: Vec<&[[f64; 2]]> = std::iter::once(self.outer.as_slice())
            .chain(self.holes.iter().map(Vec::as_slice))
            .collect();
        let corners = Corners::new(&rings, tolerance);
        let mut offset = 0;
        rings
            .iter()
            .map(|ring| {
                let kept = reduce(&corners, offset, ring.len(), tolerance, proceed)?.0;
                offset += ring.len();
                Ok(kept)
            })
            .collect()
    }
}

/// Signed area of a closed ring: positive when it runs counter-clockwise.
pub fn ring_signed_area(ring: &[[f64; 2]]) -> f64 {
    let Some(first) = ring.first() else {
        return 0.0;
    };
    // Relative to the first corner, so that far coordinates keep their digits.
    let twice: f64 = (0..ring.len())
        .map(|index| {
            let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
            (a[0] - first[0]) * (b[1] - first[1]) - (b[0] - first[0]) * (a[1] - first[1])
        })
        .sum();
    twice * 0.5
}

/// Length of a closed ring, the edge from the last corner to the first
/// included.
pub fn ring_perimeter(ring: &[[f64; 2]]) -> f64 {
    (0..ring.len())
        .map(|index| {
            let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum()
}

/// Whether a position lies inside a closed ring, by counting crossings.
pub fn ring_contains(ring: &[[f64; 2]], uv: [f64; 2]) -> bool {
    let mut inside = false;
    for index in 0..ring.len() {
        let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
        if (a[1] > uv[1]) != (b[1] > uv[1])
            && uv[0] < a[0] + (uv[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            inside = !inside;
        }
    }
    inside
}

/// Reduce a closed ring to straight segments that stay within `tolerance`
/// of it, and return the positions of the corners that remain, ascending.
/// The positions let a caller find the stretch of the ring that each
/// segment replaces.
///
/// The winding is kept, and a ring keeps at least three corners where it has
/// them, so a thin strip does not fall to a line. The ring does not come to
/// cross itself: a corner is only left out where the segment that replaces
/// it passes over no other corner of the ring. The rings of one region are
/// kept clear of each other by `Region::simplified`.
pub fn simplify_ring_indices(ring: &[[f64; 2]], tolerance: f64) -> Vec<usize> {
    let corners = Corners::new(&[ring], tolerance);
    match reduce(&corners, 0, ring.len(), tolerance, &mut || Ok(())) {
        Ok((kept, _)) => kept,
        Err(_) => (0..ring.len()).collect(),
    }
}

/// A closed ring reduced to straight segments that stay within `tolerance`
/// of it, see `simplify_ring_indices`. A tolerance of zero only removes
/// corners that lie on a straight line between their neighbours.
pub fn simplify_ring(ring: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    simplify_ring_indices(ring, tolerance)
        .into_iter()
        .map(|index| ring[index])
        .collect()
}

/// The corners of all rings of a region, one ring after another, in square
/// cells for finding those near a segment. A segment that replaces a stretch
/// of a ring may not pass over any of them: an edge can only come to cross
/// that segment when one of its ends lies between the segment and the
/// stretch, so rings that did not cross before do not cross after.
struct Corners {
    points: Vec<[f64; 2]>,
    /// Where each ring starts in `points`, and one entry more.
    rings: Vec<usize>,
    min: [f64; 2],
    cell: f64,
    width: usize,
    height: usize,
    /// Per cell, where its corners start in `order`, and one entry more.
    starts: Vec<usize>,
    /// Positions in `points`, cell by cell.
    order: Vec<usize>,
    /// The distance below which a corner counts as lying on a segment.
    touch: f64,
}

impl Corners {
    /// `reach` is the largest distance a lookup will ask for.
    fn new(rings: &[&[[f64; 2]]], reach: f64) -> Self {
        let points: Vec<[f64; 2]> = rings.iter().flat_map(|ring| ring.iter().copied()).collect();
        let mut starts = vec![0];
        for ring in rings {
            starts.push(starts[starts.len() - 1] + ring.len());
        }
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut largest = 0.0f64;
        for point in &points {
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
                largest = largest.max(point[axis].abs());
            }
        }
        let extent = [max[0] - min[0], max[1] - min[1]];
        // About one corner per cell, no cell smaller than a lookup reaches,
        // and no more cells along a side than there are corners.
        let count = points.len().max(1) as f64;
        let cell = [
            reach,
            (extent[0] * extent[1] / count).sqrt(),
            extent[0].max(extent[1]) / count,
        ]
        .into_iter()
        .fold(0.0, f64::max);
        let (cell, width, height) =
            if cell > 0.0 && cell.is_finite() && extent.iter().all(|side| side.is_finite()) {
                (
                    cell,
                    (extent[0] / cell) as usize + 1,
                    (extent[1] / cell) as usize + 1,
                )
            } else {
                (1.0, 1, 1)
            };
        let mut corners = Self {
            points,
            rings: starts,
            min,
            cell,
            width,
            height,
            starts: vec![0; width * height + 1],
            order: Vec::new(),
            touch: largest * 1e-12,
        };
        for point in &corners.points {
            let cell = corners.row(point[1]) * width + corners.column(point[0]);
            corners.starts[cell + 1] += 1;
        }
        for cell in 0..width * height {
            corners.starts[cell + 1] += corners.starts[cell];
        }
        let mut next = corners.starts.clone();
        let mut order = vec![0; corners.points.len()];
        for (number, point) in corners.points.iter().enumerate() {
            let cell = corners.row(point[1]) * width + corners.column(point[0]);
            order[next[cell]] = number;
            next[cell] += 1;
        }
        corners.order = order;
        corners
    }

    fn column(&self, u: f64) -> usize {
        // A position outside the cells, or one that is no number, goes to
        // the nearest column: the conversion saturates.
        (((u - self.min[0]) / self.cell) as usize).min(self.width - 1)
    }

    fn row(&self, v: f64) -> usize {
        (((v - self.min[1]) / self.cell) as usize).min(self.height - 1)
    }

    /// Whether `found` says yes to any corner in the cells within `reach`
    /// of the segment from `a` to `b`. It gets the position of the corner in
    /// `points`, and each corner at most once.
    fn any_near(
        &self,
        a: [f64; 2],
        b: [f64; 2],
        reach: f64,
        mut found: impl FnMut(usize) -> bool,
    ) -> bool {
        // Rounding must not let a corner on the edge of the reach slip by.
        let reach = reach + self.touch;
        let (left, right) = if a[0] <= b[0] { (a, b) } else { (b, a) };
        let run = right[0] - left[0];
        for column in self.column(left[0] - reach)..=self.column(right[0] + reach) {
            // The stretch of the segment that comes within reach of this
            // column, and the rows that stretch comes within reach of.
            let (low, high) = if run > 0.0 {
                let edge = self.min[0] + column as f64 * self.cell;
                let height = |u: f64| {
                    let u = u.max(left[0]).min(right[0]);
                    left[1] + (right[1] - left[1]) * ((u - left[0]) / run)
                };
                let (from, to) = (height(edge - reach), height(edge + self.cell + reach));
                (from.min(to), from.max(to))
            } else {
                (a[1].min(b[1]), a[1].max(b[1]))
            };
            for row in self.row(low - reach)..=self.row(high + reach) {
                let cell = row * self.width + column;
                for corner in &self.order[self.starts[cell]..self.starts[cell + 1]] {
                    if found(*corner) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The corners before and after a corner in its ring.
    fn neighbours(&self, corner: usize) -> [usize; 2] {
        let ring = self.rings.partition_point(|start| *start <= corner) - 1;
        let (start, n) = (self.rings[ring], self.rings[ring + 1] - self.rings[ring]);
        let at = corner - start;
        [start + (at + n - 1) % n, start + (at + 1) % n]
    }

    /// The corner, if any, that stands in the way of replacing a stretch of
    /// a ring by the straight segment between its ends. The ring is the `n`
    /// corners from `offset`; the stretch runs from `from` to `to` steps
    /// after corner `start` of the ring and strays `deviation` from the
    /// segment at most.
    ///
    /// A corner between the segment and the stretch, or on the segment, is
    /// in the way. So is one at a place where rings touch, when the segment
    /// would cut into what the touching ring bounds there.
    fn obstacle(
        &self,
        (offset, n): (usize, usize),
        start: usize,
        (from, to): (usize, usize),
        deviation: f64,
        examined: &mut u64,
    ) -> Option<Obstacle> {
        let at = |step: usize| self.points[offset + (start + step) % n];
        let (a, b) = (at(from), at(to));
        let side = |point: [f64; 2]| cross(minus(b, a), minus(point, a));
        // Where another corner lies on the last corner of the stretch, rings
        // touch, and the segment may arrive there only along the edge that
        // was there. What lies between two rings from one touching corner
        // to the next would fall to a line if both cut across it; with this
        // rule each of them keeps its last edge.
        let arrives_aside = {
            let (edge, segment) = (minus(at(to - 1), b), minus(a, b));
            cross(edge, segment) != 0.0 || dot(edge, segment) < 0.0
        };
        // How far along the segment every corner of the stretch lies, when
        // the stretch moves ahead at each step; found when first asked for.
        // A line across the segment meets such a stretch once, so one edge
        // tells whether a corner lies between the two, and at each of its
        // corners the segment is on a known side of the stretch.
        let mut ahead: Option<Option<Vec<f64>>> = None;
        let mut find_ahead = || {
            let along: Vec<f64> = (from..=to)
                .map(|step| dot(minus(b, a), minus(at(step), a)))
                .collect();
            along
                .windows(2)
                .all(|pair| pair[0] < pair[1])
                .then_some(along)
        };
        // Every corner near the segment is looked at, and of the obstacles
        // found the first in the order of `Obstacle` is returned: which one
        // a lookup meets first depends on the corner the ring starts at,
        // and the result must not.
        let mut found: Option<Obstacle> = None;
        let mut find = |obstacle: Obstacle| {
            let first = found.map_or(obstacle, |other| other.min(obstacle));
            found = Some(first);
            // Nothing comes before this one, so the lookup can stop.
            first == Obstacle::Beside(false)
        };
        self.any_near(a, b, deviation, |corner| {
            // The corners of the stretch go with it, and its ends stay.
            if (offset..offset + n).contains(&corner)
                && (from..=to).contains(&((corner - offset + n - start) % n))
            {
                return false;
            }
            let point = self.points[corner];
            let distance = segment_distance(point, a, b);
            if distance > deviation + self.touch {
                return false;
            }
            // A ring that touches at the first corner is no obstacle by
            // itself: its edges cannot lie between the segment and the
            // stretch unless a corner at their far end lies there too.
            if point == a {
                return false;
            }
            if point == b {
                return arrives_aside && find(Obstacle::AtEnd);
            }
            if distance <= self.touch {
                return find(Obstacle::OnSegment);
            }
            let Some(along) = ahead.get_or_insert_with(&mut find_ahead) else {
                // A stretch that turns back. Its outline, closed by the
                // segment, holds the corner when a line from the corner
                // crosses it an odd number of times, as in `ring_contains`.
                // Where a ring touches such a stretch, the stretch stays.
                *examined += (to - from) as u64;
                let mut inside = false;
                for step in from..=to {
                    let (c, d) = (at(step), if step == to { a } else { at(step + 1) });
                    if c == point {
                        return find(Obstacle::OnSegment);
                    }
                    if (c[1] > point[1]) != (d[1] > point[1])
                        && point[0] < c[0] + (point[1] - c[1]) * (d[0] - c[0]) / (d[1] - c[1])
                    {
                        inside = !inside;
                    }
                }
                return inside && find(Obstacle::Beside(side(point) > 0.0));
            };
            *examined += 1;
            // The edge of the stretch across the segment from the corner.
            let position = dot(minus(b, a), minus(point, a));
            let edge = along.partition_point(|corner| *corner <= position);
            if edge == 0 || edge == along.len() {
                return false;
            }
            let step = from + edge - 1;
            let (c, d) = (at(step), at(step + 1));
            let turn = side(point);
            if c == point {
                // A ring touches the stretch at this corner. That is no
                // obstacle when the segment passes on the other side of
                // the stretch than the edges of that ring: the two then
                // come apart. On the same side they would cross. Left of
                // the segment, the segment is to the right of the stretch.
                let (back, out) = (minus(at(step - 1), c), minus(d, c));
                let (first, last) = if turn > 0.0 { (back, out) } else { (out, back) };
                return self
                    .neighbours(corner)
                    .into_iter()
                    .any(|other| in_sector(first, last, minus(self.points[other], c)))
                    && find(Obstacle::Beside(turn > 0.0));
            }
            let share = (position - along[edge - 1]) / (along[edge] - along[edge - 1]);
            let stretch = side(c) + (side(d) - side(c)) * share;
            turn * stretch > 0.0 && turn.abs() < stretch.abs() && find(Obstacle::Beside(turn > 0.0))
        });
        found
    }
}

/// What stands in the way of a segment, which tells where to split the
/// stretch that the segment was to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Obstacle {
    /// A corner to the left of the segment (true) or to its right: the
    /// stretch passes it on that side, and so must what replaces the
    /// stretch. Split at the corner farthest on that side.
    Beside(bool),
    /// The stretch strays too far from the segment, or a corner lies on the
    /// segment: split at the corner farthest from the segment.
    OnSegment,
    /// A ring touches at the last corner: the last edge of the stretch
    /// stays.
    AtEnd,
}

fn minus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Whether direction `d` points into the sector that runs counter-clockwise
/// from direction `from` to direction `to`, its two sides included.
fn in_sector(from: [f64; 2], to: [f64; 2], d: [f64; 2]) -> bool {
    let along = |side: [f64; 2]| cross(side, d) == 0.0 && dot(side, d) > 0.0;
    let turn = cross(from, to);
    if along(from) || along(to) {
        true
    } else if turn > 0.0 {
        cross(from, d) > 0.0 && cross(d, to) > 0.0
    } else if turn < 0.0 {
        // More than half a turn: all but the sector that runs back.
        !(cross(to, d) >= 0.0 && cross(d, from) >= 0.0)
    } else {
        // Half a turn, or sides that fall together: then nothing is sure.
        dot(from, to) > 0.0 || cross(from, d) > 0.0
    }
}

/// `simplify_ring_indices` for the ring of `n` corners from `offset` in
/// `corners`, kept clear of all corners there. Also returns how many corners
/// were looked at, as the measure of the work done.
fn reduce(
    corners: &Corners,
    offset: usize,
    n: usize,
    tolerance: f64,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<(Vec<usize>, u64), LoadError> {
    let ring = &corners.points[offset..offset + n];
    let mut examined = 0u64;
    if n <= 3 || tolerance.is_nan() || tolerance < 0.0 {
        return Ok(((0..n).collect(), examined));
    }
    // Two corners that lie on the hull: the lowest, leftmost one and the one
    // farthest from it. Ties go by position, so that the choice does not
    // depend on the corner the ring starts at; corners count as equally far
    // within the rounding of their coordinates.
    let by_position = |a: [f64; 2], b: [f64; 2]| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0]));
    let mut first = 0;
    for index in 1..n {
        if by_position(ring[index], ring[first]).is_lt() {
            first = index;
        }
    }
    let from_first =
        |index: usize| (ring[index][0] - ring[first][0]).hypot(ring[index][1] - ring[first][1]);
    let mut far = first;
    for index in 0..n {
        let further = from_first(index) - from_first(far);
        if further > corners.touch
            || (further >= -corners.touch && by_position(ring[index], ring[far]).is_gt())
        {
            far = index;
        }
    }
    if far == first {
        return Ok((vec![first], examined));
    }
    let mut keep = vec![false; n];
    keep[first] = true;
    keep[far] = true;
    let steps = (far + n - first) % n;
    let chains = [(first, steps), (far, n - steps)];
    // Stretches to examine: the corner they are counted from, their first
    // and last step, and whether they must be split in any case.
    let mut pending: Vec<(usize, usize, usize, bool)> = chains
        .iter()
        .map(|(start, steps)| (*start, 0, *steps, false))
        .collect();
    let mut widened = false;
    loop {
        while let Some((start, from, to, split_anyway)) = pending.pop() {
            proceed()?;
            if to - from < 2 {
                continue;
            }
            let (a, b) = (ring[(start + from) % n], ring[(start + to) % n]);
            let deviation = |step: usize| segment_distance(ring[(start + step) % n], a, b);
            examined += (to - from) as u64;
            let worst = (from + 1..to).map(deviation).fold(0.0, f64::max);
            // A stretch on the segment itself changes nothing when it goes.
            let obstacle = if worst <= corners.touch {
                None
            } else if worst > tolerance + corners.touch || split_anyway {
                Some(Obstacle::OnSegment)
            } else {
                corners.obstacle((offset, n), start, (from, to), worst, &mut examined)
            };
            let Some(obstacle) = obstacle else {
                continue;
            };
            examined += (to - from) as u64;
            // Of corners that are equally far, as the teeth of a comb are,
            // the one nearest the middle: taking the first would peel off
            // one tooth per pass and look at all the others again every
            // time.
            let farthest = |among: &dyn Fn(f64) -> bool| {
                let turn = |step: usize| cross(minus(b, a), minus(ring[(start + step) % n], a));
                let worst = (from + 1..to)
                    .filter(|step| among(turn(*step)))
                    .map(deviation)
                    .fold(0.0, f64::max);
                (from + 1..to)
                    .filter(|step| among(turn(*step)) && deviation(*step) >= worst - corners.touch)
                    .min_by_key(|step| (2 * step).abs_diff(from + to))
            };
            // Where to split follows from what was in the way.
            let split = match obstacle {
                Obstacle::OnSegment => None,
                Obstacle::Beside(true) => farthest(&|turn| turn > 0.0),
                Obstacle::Beside(false) => farthest(&|turn| turn < 0.0),
                Obstacle::AtEnd => Some(to - 1),
            }
            .or_else(|| farthest(&|_| true))
            .unwrap_or((from + to) / 2);
            keep[(start + split) % n] = true;
            pending.push((start, from, split, false));
            pending.push((start, split, to, false));
        }
        if widened || keep.iter().filter(|keep| **keep).count() >= 3 {
            break;
        }
        // Everything lies within the tolerance of one line: keep what is
        // farthest from it on either side, so that an area remains.
        widened = true;
        pending.extend(
            chains
                .iter()
                .map(|(start, steps)| (*start, 0, *steps, true)),
        );
    }
    Ok(((0..n).filter(|index| keep[*index]).collect(), examined))
}

/// Distance from a point to the segment between `a` and `b`.
fn segment_distance(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    let along = if length_sq > 0.0 {
        (((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point[0] - a[0] - along * dx).hypot(point[1] - a[1] - along * dy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::Rng;

    /// A mask of unit cells at the origin, drawn with `#` for an occupied
    /// cell. The first row is the top one.
    fn mask(rows: &[&str]) -> Mask {
        let frame =
            GridFrame::new([0.0, 0.0], 1.0, rows[0].len() as u32, rows.len() as u32).unwrap();
        Mask::from_fn(frame, |x, y| {
            rows[rows.len() - 1 - y as usize].as_bytes()[x as usize] == b'#'
        })
    }

    fn drawn(mask: &Mask) -> Vec<String> {
        let frame = mask.frame();
        (0..frame.height)
            .rev()
            .map(|y| {
                (0..frame.width)
                    .map(|x| {
                        if mask.get(x as i64, y as i64) {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn frame_maps_cells_and_plane_coordinates() {
        let frame = GridFrame::new([10.0, -4.0], 0.5, 6, 4).unwrap();
        assert_eq!(frame.cells(), 24);
        assert_eq!(frame.index(2, 3), 20);
        assert_eq!(frame.cell_of([10.0, -4.0]), Some([0, 0]));
        assert_eq!(frame.cell_of([11.26, -2.01]), Some([2, 3]));
        assert_eq!(frame.cell_of([12.99, -2.01]), Some([5, 3]));
        assert_eq!(frame.cell_of([13.0, -3.0]), None);
        assert_eq!(frame.cell_of([9.99, -3.0]), None);
        assert_eq!(frame.cell_of([11.0, -2.0]), None);
        assert_eq!(frame.cell_of([f64::NAN, -3.0]), None);
        assert_eq!(frame.cell_center(2, 3), [11.25, -2.25]);
        assert_eq!(frame.vertex([0, 0]), [10.0, -4.0]);
        assert_eq!(frame.vertex([6, 4]), [13.0, -2.0]);
        assert_eq!(frame.max(), [13.0, -2.0]);
        assert_eq!(frame.cell_area(), 0.25);
        assert_eq!(frame.cells_for_area(0.05), 1);
        assert_eq!(frame.cells_for_area(0.26), 2);
        assert_eq!(frame.cells_for_length(1.2), 2);
        assert_eq!(frame.cells_for_length(1.3), 3);
        assert!(GridFrame::new([0.0, 0.0], 0.0, 1, 1).is_err());
        assert!(GridFrame::new([0.0, f64::NAN], 1.0, 1, 1).is_err());
        assert!(GridFrame::new([0.0, 0.0], 1.0, 0, 1).is_err());
        assert!(GridFrame::new([0.0, 0.0], 1.0, 1 << 14, 1 << 13).is_err());
    }

    #[test]
    fn covering_frame_holds_both_corners_and_coarsens_to_fit() {
        let frame = GridFrame::covering([1.0, 2.0], [5.0, 3.0], 0.02, 1_000_000).unwrap();
        assert_eq!((frame.cell, frame.origin), (0.02, [1.0, 2.0]));
        assert!(frame.cell_of([1.0, 2.0]).is_some());
        assert!(frame.cell_of([5.0, 3.0]).is_some());
        assert!(frame.width <= 202 && frame.height <= 52);
        // 201 by 51 cells do not fit in 5,000; 101 by 26 do.
        let coarse = GridFrame::covering([1.0, 2.0], [5.0, 3.0], 0.02, 5_000).unwrap();
        assert_eq!(coarse.cell, 0.04);
        assert!(coarse.cells() <= 5_000);
        assert!(coarse.cell_of([5.0, 3.0]).is_some());
        let point = GridFrame::covering([1.0, 2.0], [1.0, 2.0], 0.02, 10).unwrap();
        assert_eq!((point.width, point.height), (1, 1));
        assert!(GridFrame::covering([1.0, 2.0], [0.0, 3.0], 0.02, 10).is_err());
        assert!(GridFrame::covering([1.0, 2.0], [5.0, 3.0], -1.0, 10).is_err());
        assert!(GridFrame::covering([1.0, 2.0], [5.0, 3.0], 0.02, 0).is_err());
    }

    #[test]
    fn count_grid_counts_thresholds_and_coarsens() {
        let frame = GridFrame::new([0.0, 0.0], 0.1, 5, 3).unwrap();
        let mut grid = CountGrid::new(frame);
        assert_eq!(grid.median_occupied(), 0);
        for _ in 0..3 {
            assert_eq!(grid.add([0.05, 0.05]), Some(0));
        }
        assert_eq!(grid.add([0.45, 0.25]), Some(14));
        assert_eq!(grid.add([0.45, 0.26]), Some(14));
        assert_eq!(grid.add([0.31, 0.11]), Some(8));
        assert_eq!(grid.add([0.55, 0.05]), None);
        assert_eq!(grid.add([-0.01, 0.05]), None);
        assert_eq!(
            (grid.count(0, 0), grid.count(4, 2), grid.count(3, 1)),
            (3, 2, 1)
        );
        assert_eq!(grid.occupied(), 3);
        assert_eq!(grid.median_occupied(), 2);
        assert_eq!(grid.counts().iter().sum::<u32>(), 6);
        assert_eq!(drawn(&grid.threshold(1)), ["....#", "...#.", "#...."]);
        assert_eq!(drawn(&grid.threshold(2)), ["....#", ".....", "#...."]);
        assert_eq!(drawn(&grid.threshold(3)), [".....", ".....", "#...."]);

        let coarse = grid.coarsened();
        assert_eq!(
            coarse.frame(),
            GridFrame::new([0.0, 0.0], 0.2, 3, 2).unwrap()
        );
        assert_eq!(coarse.counts(), [3, 1, 0, 0, 0, 2]);
    }

    #[test]
    fn rectangle_traces_to_one_counter_clockwise_ring() {
        let frame = GridFrame::new([10.0, 20.0], 0.5, 6, 5).unwrap();
        let block = Mask::from_fn(frame, |x, y| (1..4).contains(&x) && (2..4).contains(&y));
        for connectivity in [Connectivity::Four, Connectivity::Eight] {
            let cells = block.trace(connectivity);
            assert_eq!(cells.len(), 1);
            assert_eq!(cells[0].outer, [[1, 2], [4, 2], [4, 4], [1, 4]]);
            assert!(cells[0].holes.is_empty());
            assert_eq!(cells[0].cells(), 6);
            let regions = block.regions(connectivity);
            assert_eq!(
                regions[0].outer,
                [[10.5, 21.0], [12.0, 21.0], [12.0, 22.0], [10.5, 22.0]]
            );
            assert_eq!(ring_signed_area(&regions[0].outer), 1.5);
            assert_eq!(regions[0].area(), 1.5);
            assert_eq!(regions[0].perimeter(), 5.0);
        }
    }

    #[test]
    fn l_shape_has_six_corners() {
        let shape = mask(&["#...", "#...", "####"]);
        let regions = shape.trace(Connectivity::Eight);
        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions[0].outer,
            [[0, 0], [4, 0], [4, 1], [1, 1], [1, 3], [0, 3]]
        );
        assert_eq!(regions[0].cells(), 6);
        assert_eq!(regions[0].to_plane(&shape.frame()).perimeter(), 14.0);
    }

    #[test]
    fn hole_is_a_clockwise_ring_of_its_region() {
        let shape = mask(&["#####", "#..##", "#####", "#####"]);
        let regions = shape.regions(Connectivity::Eight);
        assert_eq!(regions.len(), 1);
        let region = &regions[0];
        assert_eq!(ring_signed_area(&region.outer), 20.0);
        assert_eq!(region.holes.len(), 1);
        assert_eq!(ring_signed_area(&region.holes[0]), -2.0);
        assert_eq!(region.holes[0].len(), 4);
        for corner in [[1.0, 2.0], [3.0, 2.0], [3.0, 3.0], [1.0, 3.0]] {
            assert!(region.holes[0].contains(&corner));
        }
        assert_eq!(region.area(), 18.0);
        assert_eq!(region.perimeter(), 18.0 + 6.0);
        assert_eq!(shape.trace(Connectivity::Eight)[0].cells(), 18);
        assert!(region.contains([0.5, 2.5]));
        assert!(!region.contains([2.0, 2.5]));
        assert!(!region.contains([5.5, 2.5]));
    }

    #[test]
    fn cells_that_touch_at_a_corner_join_only_with_corner_connectivity() {
        let shape = mask(&[".#", "#."]);
        let joined = shape.trace(Connectivity::Eight);
        assert_eq!(joined.len(), 1);
        // One ring that passes through the shared corner twice.
        assert_eq!(
            joined[0].outer,
            [
                [0, 0],
                [1, 0],
                [1, 1],
                [2, 1],
                [2, 2],
                [1, 2],
                [1, 1],
                [0, 1]
            ]
        );
        assert_eq!(joined[0].cells(), 2);
        let apart = shape.trace(Connectivity::Four);
        assert_eq!(apart.len(), 2);
        assert_eq!(apart[0].outer, [[0, 0], [1, 0], [1, 1], [0, 1]]);
        assert_eq!(apart[1].outer, [[1, 1], [2, 1], [2, 2], [1, 2]]);
    }

    #[test]
    fn holes_that_touch_at_a_corner_join_only_when_cells_do_not() {
        let shape = mask(&["####", "#.##", "##.#", "####"]);
        let corners = shape.trace(Connectivity::Eight);
        assert_eq!(corners.len(), 1);
        assert_eq!(corners[0].holes.len(), 2);
        assert!(corners[0].holes.iter().all(|hole| hole.len() == 4));
        assert!(corners[0]
            .holes
            .iter()
            .all(|hole| twice_cell_area(hole) == -2));
        let edges = shape.trace(Connectivity::Four);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].holes.len(), 1);
        assert_eq!(edges[0].holes[0].len(), 8);
        assert_eq!(twice_cell_area(&edges[0].holes[0]), -4);
        assert_eq!(edges[0].cells(), 14);
    }

    #[test]
    fn island_inside_a_hole_is_its_own_region() {
        let shape = mask(&[
            "#######", "#.....#", "#.###.#", "#.#.#.#", "#.###.#", "#.....#", "#######",
        ]);
        let regions = shape.trace(Connectivity::Eight);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].outer, [[0, 0], [7, 0], [7, 7], [0, 7]]);
        assert_eq!(regions[0].holes.len(), 1);
        assert_eq!(twice_cell_area(&regions[0].holes[0]), -50);
        assert_eq!(regions[1].outer, [[2, 2], [5, 2], [5, 5], [2, 5]]);
        assert_eq!(regions[1].holes.len(), 1);
        assert_eq!(twice_cell_area(&regions[1].holes[0]), -2);
        assert_eq!(
            regions[0].cells() + regions[1].cells(),
            shape.count() as u64
        );
    }

    #[test]
    fn every_ring_keeps_the_occupied_cells_on_its_left() {
        // A shape with holes, a notch, a diagonal contact and a cell on the
        // edge of the grid.
        let shape = mask(&[
            "..##....#",
            ".####..#.",
            "##..##...",
            "#.##.#.##",
            "##..##.##",
            ".####...#",
        ]);
        for connectivity in [Connectivity::Four, Connectivity::Eight] {
            let regions = shape.trace(connectivity);
            assert_eq!(regions.len(), shape.components(connectivity).count());
            let mut cells = 0;
            for region in &regions {
                assert!(twice_cell_area(&region.outer) > 0);
                assert!(region.holes.iter().all(|hole| twice_cell_area(hole) < 0));
                cells += region.cells();
                for ring in std::iter::once(&region.outer).chain(&region.holes) {
                    for index in 0..ring.len() {
                        let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
                        // Axis-aligned edges between different corners.
                        assert!((a[0] == b[0]) != (a[1] == b[1]));
                        let heading = match ((b[0] - a[0]).signum(), (b[1] - a[1]).signum()) {
                            (1, 0) => 0,
                            (0, 1) => 1,
                            (-1, 0) => 2,
                            _ => 3,
                        };
                        let (left, right) = edge_sides(a[0] as i64, a[1] as i64, heading);
                        assert!(shape.get(left.0, left.1));
                        assert!(!shape.get(right.0, right.1));
                    }
                }
            }
            assert_eq!(cells, shape.count() as u64);
        }
    }

    #[test]
    fn closing_bridges_a_gap_up_to_twice_the_radius_and_no_wider() {
        // Two wall faces four cells apart, and a doorway of five cells.
        let faces = mask(&[
            "..............",
            ".####.....###.",
            "..............",
            "..............",
            "..............",
            "..............",
            ".####.....###.",
            "..............",
        ]);
        let mut closed = faces.clone();
        closed.close(2);
        assert_eq!(
            drawn(&closed),
            [
                "..............",
                ".####.....###.",
                ".####.....###.",
                ".####.....###.",
                ".####.....###.",
                ".####.....###.",
                ".####.....###.",
                "..............",
            ]
        );
        // One cell more between the faces and nothing is bridged.
        let apart = mask(&[
            "..............",
            ".####.....###.",
            "..............",
            "..............",
            "..............",
            "..............",
            "..............",
            ".####.....###.",
            "..............",
        ]);
        let mut unchanged = apart.clone();
        unchanged.close(2);
        assert_eq!(unchanged, apart);
        // A radius of three bridges both the faces and the doorway.
        let mut wide = faces.clone();
        wide.close(3);
        assert!((1..13).all(|x| (1..7).all(|y| wide.get(x, y))));
        assert_eq!(wide.count(), 12 * 6);
    }

    #[test]
    fn closing_never_grows_a_shape_at_the_edge_of_the_grid() {
        for rows in [
            ["#.....", "......", "......", "......"],
            ["......", "......", "..#...", "......"],
            ["......", "......", "......", ".....#"],
            ["..#...", "......", "......", "......"],
            ["###...", "###...", "......", "......"],
        ] {
            let shape = mask(&rows);
            for radius in 0..5 {
                let mut closed = shape.clone();
                closed.close(radius);
                assert_eq!(closed, shape, "radius {radius}");
            }
        }
    }

    #[test]
    fn opening_drops_thin_parts_and_keeps_blocks() {
        let mut shape = mask(&[
            ".........",
            ".###.....",
            ".#######.",
            ".###.....",
            "......#..",
            ".........",
        ]);
        shape.open(1);
        assert_eq!(
            drawn(&shape),
            [
                ".........",
                ".###.....",
                ".###.....",
                ".###.....",
                ".........",
                ".........",
            ]
        );
        // The edge of the grid does not cut into a block that lies against it.
        let mut edge = mask(&["###..", "###..", "###.."]);
        edge.open(1);
        assert_eq!(drawn(&edge), ["###..", "###..", "###.."]);
    }

    #[test]
    fn dilation_and_erosion_take_a_radius_per_direction() {
        let mut shape = Mask::new(GridFrame::new([0.0, 0.0], 1.0, 5, 5).unwrap());
        assert_eq!(shape.count(), 0);
        shape.set(2, 2, true);
        shape.set(4, 4, true);
        shape.set(4, 4, false);
        assert_eq!(shape, mask(&[".....", ".....", "..#..", ".....", "....."]));
        assert_eq!(shape.cells().iter().position(|cell| *cell), Some(12));
        assert!(shape.get(2, 2) && !shape.get(2, 3) && !shape.get(-1, 2) && !shape.get(2, 5));
        shape.dilate(2, 1);
        assert_eq!(drawn(&shape), [".....", "#####", "#####", "#####", "....."]);
        shape.erode(1, 1);
        assert_eq!(drawn(&shape), [".....", ".....", ".###.", ".....", "....."]);
        shape.erode(1, 0);
        assert_eq!(drawn(&shape), [".....", ".....", "..#..", ".....", "....."]);
        shape.dilate(0, 9);
        assert_eq!(drawn(&shape), ["..#..", "..#..", "..#..", "..#..", "..#.."]);
        shape.erode(0, 1);
        assert_eq!(drawn(&shape), [".....", "..#..", "..#..", "..#..", "....."]);
    }

    #[test]
    fn components_are_numbered_in_reading_order_with_their_sizes() {
        let shape = mask(&["..#.#", "...#.", "##...", "##..#"]);
        let corners = shape.components(Connectivity::Eight);
        assert_eq!(corners.sizes, [4, 1, 3]);
        assert_eq!(corners.count(), 3);
        // Stored from the bottom row up.
        assert_eq!(corners.labels[..5], [1, 1, 0, 0, 2]);
        assert_eq!(corners.labels[15..], [0, 0, 3, 0, 3]);
        let edges = shape.components(Connectivity::Four);
        assert_eq!(edges.sizes, [4, 1, 1, 1, 1]);
        assert_eq!(
            mask(&["...", "..."])
                .components(Connectivity::Eight)
                .count(),
            0
        );
    }

    #[test]
    fn small_components_are_removed_and_large_ones_stay() {
        let speckled = mask(&["#....#.", ".....#.", ".###...", ".###..#", "....#.."]);
        let mut corners = speckled.clone();
        assert_eq!(corners.remove_small_components(3, Connectivity::Eight), 3);
        assert_eq!(
            drawn(&corners),
            [".......", ".......", ".###...", ".###...", "....#.."]
        );
        let mut edges = speckled.clone();
        assert_eq!(edges.remove_small_components(3, Connectivity::Four), 4);
        assert_eq!(
            drawn(&edges),
            [".......", ".......", ".###...", ".###...", "......."]
        );
        let mut all = speckled.clone();
        assert_eq!(all.remove_small_components(0, Connectivity::Eight), 0);
        assert_eq!(all, speckled);
        // A component of exactly the smallest size stays.
        let mut largest = speckled.clone();
        assert_eq!(largest.remove_small_components(7, Connectivity::Eight), 3);
        assert_eq!(largest.count(), 7);
        assert_eq!(largest.remove_small_components(8, Connectivity::Eight), 1);
        assert_eq!(largest, Mask::new(speckled.frame()));
    }

    #[test]
    fn holes_below_a_size_are_filled_and_larger_or_open_ones_stay() {
        let shape = mask(&[
            "########.",
            "#.##...#.",
            "####...#.",
            "#.######.",
            "..#####..",
        ]);
        // One hole of one cell, one of six, and two notches open to the edge.
        let mut small = shape.clone();
        assert_eq!(small.fill_small_holes(6, Connectivity::Eight), 1);
        assert_eq!(
            drawn(&small),
            [
                "########.",
                "####...#.",
                "####...#.",
                "#.######.",
                "..#####..",
            ]
        );
        let mut both = shape.clone();
        assert_eq!(both.fill_small_holes(7, Connectivity::Eight), 2);
        assert_eq!(both.count(), shape.count() + 7);
        assert!(!both.get(1, 1) && !both.get(0, 0) && !both.get(8, 2));

        // The empty cell at the lower left touches the outside at a corner
        // only: a hole when cells connect by corners, open when they do not.
        let pinched = mask(&["###", "#.#", ".##"]);
        let mut corners = pinched.clone();
        assert_eq!(corners.fill_small_holes(2, Connectivity::Eight), 1);
        assert!(corners.get(1, 1));
        let mut edges = pinched.clone();
        assert_eq!(edges.fill_small_holes(2, Connectivity::Four), 0);
        assert_eq!(edges, pinched);

        // Empty cells that reach any one of the four edges are not holes.
        for rows in [["#.#", "###", "###"], ["###", "###", "#.#"]] {
            for open in [mask(&rows), mask(&[".##", "###", "###"])] {
                let mut filled = open.clone();
                assert_eq!(filled.fill_small_holes(99, Connectivity::Eight), 0);
                assert_eq!(filled, open);
            }
        }
        for rows in [["###", ".##", "###"], ["###", "##.", "###"]] {
            let open = mask(&rows);
            let mut filled = open.clone();
            assert_eq!(filled.fill_small_holes(99, Connectivity::Eight), 0);
            assert_eq!(filled, open);
        }
    }

    #[test]
    fn area_and_perimeter_of_known_rings() {
        let square = [[2.0, 1.0], [5.0, 1.0], [5.0, 3.0], [2.0, 3.0]];
        assert_eq!(ring_signed_area(&square), 6.0);
        assert_eq!(ring_perimeter(&square), 10.0);
        let mut reversed = square;
        reversed.reverse();
        assert_eq!(ring_signed_area(&reversed), -6.0);
        assert_eq!(ring_perimeter(&reversed), 10.0);
        let triangle = [[0.0, 0.0], [4.0, 0.0], [0.0, 3.0]];
        assert_eq!(ring_signed_area(&triangle), 6.0);
        assert_eq!(ring_perimeter(&triangle), 12.0);
        assert_eq!(ring_signed_area(&[]), 0.0);
        assert_eq!(ring_perimeter(&[]), 0.0);
        // The same square at national grid coordinates, in millimetres.
        let far = square.map(|[u, v]| [u * 0.001 + 207_000.0, v * 0.001 + 474_000.0]);
        assert!((ring_signed_area(&far) - 6.0e-6).abs() < 1e-12);
        assert!(ring_contains(&square, [3.0, 2.0]));
        assert!(!ring_contains(&square, [1.0, 2.0]));
        assert!(!ring_contains(&square, [3.0, 3.5]));
    }

    #[test]
    fn simplification_turns_a_staircase_into_a_straight_edge() {
        // A right triangle whose slope is a staircase of unit steps.
        let mut ring = vec![[0.0, 0.0], [8.0, 0.0]];
        for step in 0..8 {
            ring.push([8.0 - step as f64, step as f64 + 1.0]);
            ring.push([7.0 - step as f64, step as f64 + 1.0]);
        }
        assert_eq!(ring.last(), Some(&[0.0, 8.0]));
        let area = ring_signed_area(&ring);
        let simple = simplify_ring(&ring, 0.8);
        // The riser at either end of the stairs is a real edge of the shape;
        // the one at the upper end is on the far corner and stays.
        assert_eq!(simple, [[0.0, 0.0], [8.0, 0.0], [1.0, 8.0], [0.0, 8.0]]);
        assert_eq!(ring_signed_area(&simple), area);
        // Below the size of a step every corner stays.
        assert_eq!(simplify_ring(&ring, 0.3), ring);
        // The result does not depend on the corner the ring starts with.
        for shift in 1..ring.len() {
            let mut turned = ring.clone();
            turned.rotate_left(shift);
            let mut corners = simplify_ring(&turned, 0.8);
            corners.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
            assert_eq!(corners, [[0.0, 0.0], [0.0, 8.0], [1.0, 8.0], [8.0, 0.0]]);
        }
    }

    #[test]
    fn simplification_keeps_what_exceeds_the_tolerance_and_the_winding() {
        // A rectangle with a notch 0.5 deep in its top edge.
        let ring = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 4.0],
            [6.0, 4.0],
            [6.0, 3.5],
            [4.0, 3.5],
            [4.0, 4.0],
            [0.0, 4.0],
        ];
        let kept = simplify_ring_indices(&ring, 0.1);
        assert_eq!(kept, [0, 1, 2, 3, 4, 5, 6, 7]);
        let dropped = simplify_ring_indices(&ring, 0.6);
        assert_eq!(dropped, [0, 1, 2, 7]);
        let mut clockwise = ring;
        clockwise.reverse();
        let simple = simplify_ring(&clockwise, 0.6);
        assert_eq!(simple.len(), 4);
        assert_eq!(ring_signed_area(&simple), -40.0);
    }

    #[test]
    fn simplification_removes_points_on_a_straight_line_at_zero_tolerance() {
        let ring = [
            [0.0, 0.0],
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [2.0, 2.0],
            [1.0, 2.0],
            [0.0, 2.0],
            [0.0, 1.0],
        ];
        assert_eq!(simplify_ring_indices(&ring, 0.0), [0, 2, 4, 6]);
        let triangle = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert_eq!(simplify_ring(&triangle, 100.0), triangle);
        assert_eq!(simplify_ring(&ring, f64::NAN), ring);
        assert_eq!(simplify_ring(&[[1.0, 1.0]; 5], 0.1), [[1.0, 1.0]]);
    }

    #[test]
    fn a_thin_strip_keeps_its_area_when_simplified() {
        // One cell thick and far within the tolerance of its own diagonal.
        let strip = mask(&["####################"]);
        let region = &strip.regions(Connectivity::Eight)[0];
        let simple = region.simplified(1.5);
        assert_eq!(simple.outer.len(), 4);
        assert_eq!(simple.area(), 20.0);
        assert!(ring_signed_area(&simple.outer) > 0.0);
    }

    #[test]
    fn a_traced_disc_has_its_area_and_simplifies_to_few_corners() {
        let frame = GridFrame::new([-1.0, -1.0], 0.02, 100, 100).unwrap();
        let disc = Mask::from_fn(frame, |x, y| {
            let [u, v] = frame.cell_center(x, y);
            u * u + v * v <= 0.8 * 0.8 && u * u + v * v >= 0.3 * 0.3
        });
        let regions = disc.regions(Connectivity::Eight);
        assert_eq!(regions.len(), 1);
        let region = &regions[0];
        assert_eq!(region.holes.len(), 1);
        let expected = std::f64::consts::PI * (0.64 - 0.09);
        assert!((region.area() - expected).abs() < 0.01 * expected);
        assert!((region.area() - disc.count() as f64 * frame.cell_area()).abs() < 1e-9);
        let simple = region.simplified(0.03);
        assert!(simple.outer.len() < region.outer.len() / 4);
        assert!(simple.outer.len() >= 12);
        assert_eq!(simple.holes.len(), 1);
        assert!((simple.area() - expected).abs() < 0.04 * expected);
        assert!(ring_signed_area(&simple.outer) > 0.0);
        assert!(ring_signed_area(&simple.holes[0]) < 0.0);
        assert!(simple.contains([0.5, 0.0]) && !simple.contains([0.0, 0.0]));
        // Every corner that is left lies on the ring it came from.
        assert!(simple
            .outer
            .iter()
            .all(|corner| region.outer.contains(corner)));
    }

    /// Whether direction `d` lies strictly inside the sector that runs
    /// counter-clockwise from direction `from` to direction `to`. The
    /// checker has its own test, apart from the one in the code it checks.
    fn strictly_in_sector(from: [f64; 2], to: [f64; 2], d: [f64; 2]) -> bool {
        let turn = cross(from, to);
        if turn > 0.0 {
            cross(from, d) > 0.0 && cross(d, to) > 0.0
        } else if turn < 0.0 {
            !(cross(to, d) >= 0.0 && cross(d, from) >= 0.0)
        } else {
            cross(from, d) > 0.0
        }
    }

    /// What is wrong with a region as the boundary of an area, found by
    /// comparing every edge with every other. Rings may touch at a corner
    /// they share, as traced rings do; anything else that brings two edges
    /// together is a fault. Exact for corners on whole numbers.
    fn fault(region: &Region) -> Option<String> {
        let rings: Vec<&Vec<[f64; 2]>> = std::iter::once(&region.outer)
            .chain(&region.holes)
            .collect();
        let mut edges = Vec::new();
        // Per corner: the ring, the corner before it and the one after.
        let mut visits = Vec::new();
        for (number, ring) in rings.iter().enumerate() {
            let n = ring.len();
            if n < 3 {
                return Some(format!("ring {number} has {n} corners"));
            }
            let area = ring_signed_area(ring);
            if (number == 0) != (area > 0.0) || area == 0.0 {
                return Some(format!("ring {number} has area {area}"));
            }
            for index in 0..n {
                let (a, b) = (ring[index], ring[(index + 1) % n]);
                if a == b {
                    return Some(format!("ring {number} repeats {a:?}"));
                }
                edges.push((a, b));
                visits.push((a, ring[(index + n - 1) % n], b));
            }
        }
        for (first, &(a, b)) in edges.iter().enumerate() {
            for &(c, d) in &edges[first + 1..] {
                let shared = [(a, b, c, d), (a, b, d, c), (b, a, c, d), (b, a, d, c)]
                    .into_iter()
                    .find(|(p, _, q, _)| p == q);
                if let Some((p, e, _, f)) = shared {
                    // Two edges from one corner may not run along each other.
                    let (e, f) = (minus(e, p), minus(f, p));
                    if cross(e, f) == 0.0 && dot(e, f) > 0.0 {
                        return Some(format!("{a:?}-{b:?} runs along {c:?}-{d:?}"));
                    }
                    continue;
                }
                let side = |p, q, r| cross(minus(q, p), minus(r, p));
                let (s1, s2, s3, s4) = (side(a, b, c), side(a, b, d), side(c, d, a), side(c, d, b));
                let between = |p, q, r| dot(minus(r, p), minus(r, q)) <= 0.0;
                if (s1 * s2 < 0.0 && s3 * s4 < 0.0)
                    || (s1 == 0.0 && between(a, b, c))
                    || (s2 == 0.0 && between(a, b, d))
                    || (s3 == 0.0 && between(c, d, a))
                    || (s4 == 0.0 && between(c, d, b))
                {
                    return Some(format!("{a:?}-{b:?} meets {c:?}-{d:?}"));
                }
            }
        }
        // Where rings share a corner, two passages through it must keep
        // either their empty sides (to the right) or their occupied sides
        // (to the left) clear of each other.
        for (first, &(at, before, after)) in visits.iter().enumerate() {
            for &(other, other_before, other_after) in &visits[first + 1..] {
                if at != other {
                    continue;
                }
                let (b1, o1) = (minus(before, at), minus(after, at));
                let (b2, o2) = (minus(other_before, at), minus(other_after, at));
                let empty_sides_meet = strictly_in_sector(b1, o1, b2)
                    || strictly_in_sector(b1, o1, o2)
                    || strictly_in_sector(b2, o2, b1)
                    || strictly_in_sector(b2, o2, o1);
                let occupied_sides_meet = strictly_in_sector(o1, b1, b2)
                    || strictly_in_sector(o1, b1, o2)
                    || strictly_in_sector(o2, b2, b1)
                    || strictly_in_sector(o2, b2, o1);
                if empty_sides_meet && occupied_sides_meet {
                    return Some(format!("rings cross at their shared corner {at:?}"));
                }
            }
        }
        for (number, hole) in region.holes.iter().enumerate() {
            for corner in hole {
                if !ring_contains(&region.outer, *corner) && !region.outer.contains(corner) {
                    return Some(format!("hole {number} lies outside the outer ring"));
                }
                for (other_number, other) in region.holes.iter().enumerate() {
                    if other_number != number
                        && ring_contains(other, *corner)
                        && !other.contains(corner)
                    {
                        return Some(format!("hole {number} lies in hole {other_number}"));
                    }
                }
            }
        }
        None
    }

    /// The cut through a room of 200 by 150 cells of which only the inside
    /// of the walls was scanned, turned about its centre: a point every
    /// eighth of a cell along the four faces, and the cells with three or
    /// more.
    fn single_face_room(degrees: f64, close: u32) -> Mask {
        let frame = GridFrame::new([0.0, 0.0], 1.0, 300, 300).unwrap();
        let mut grid = CountGrid::new(frame);
        let (sin, cos) = degrees.to_radians().sin_cos();
        let corners = [
            [-100.0, -75.0],
            [100.0, -75.0],
            [100.0, 75.0],
            [-100.0, 75.0],
        ];
        for side in 0..4 {
            let (a, b) = (corners[side], corners[(side + 1) % 4]);
            let steps = if side % 2 == 0 { 1_600 } else { 1_200 };
            for step in 0..steps {
                let along = step as f64 / steps as f64;
                let (u, v) = (a[0] + (b[0] - a[0]) * along, a[1] + (b[1] - a[1]) * along);
                grid.add([150.0 + u * cos - v * sin, 150.0 + u * sin + v * cos])
                    .unwrap();
            }
        }
        let mut mask = grid.threshold(3);
        mask.close(close);
        mask.remove_small_components(6, Connectivity::Eight);
        mask
    }

    #[test]
    fn the_checker_of_these_tests_tells_sound_regions_from_broken_ones() {
        let shape = mask(&["#####", "#..##", "###.#", "#####"]);
        let region = &shape.regions(Connectivity::Eight)[0];
        assert_eq!(region.holes.len(), 2);
        assert_eq!(fault(region), None);
        let pinched = &mask(&[".#", "#."]).regions(Connectivity::Eight)[0];
        assert_eq!(fault(pinched), None);
        let square = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let hole = |x: f64, y: f64| vec![[x, y], [x, y + 1.0], [x + 1.0, y + 1.0], [x + 1.0, y]];
        let region = |outer: &Vec<[f64; 2]>, holes: &[Vec<[f64; 2]>]| Region {
            outer: outer.clone(),
            holes: holes.to_vec(),
        };
        assert_eq!(fault(&region(&square, &[hole(1.0, 1.0)])), None);
        // A hole across the outer ring, outside it, in another hole, wound
        // the wrong way, and against an edge of the outer ring.
        assert!(fault(&region(&square, &[hole(3.5, 1.0)])).is_some());
        assert!(fault(&region(&square, &[hole(6.0, 1.0)])).is_some());
        let wide = vec![[0.5, 0.5], [0.5, 3.5], [3.5, 3.5], [3.5, 0.5]];
        assert!(fault(&region(&square, &[wide, hole(1.0, 1.0)])).is_some());
        let mut reversed = hole(1.0, 1.0);
        reversed.reverse();
        assert!(fault(&region(&square, &[reversed])).is_some());
        assert!(fault(&region(&square, &[hole(3.0, 1.0)])).is_some());
        // A ring that crosses itself, between corners and through one.
        let bow = vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0], [4.0, 6.0]];
        assert!(fault(&region(&bow, &[])).is_some());
        let through = vec![
            [0.0, -3.0],
            [2.0, 2.0],
            [4.0, 5.0],
            [4.0, 1.0],
            [2.0, 2.0],
            [0.0, 6.0],
        ];
        assert!(fault(&region(&through, &[])).is_some());
    }

    #[test]
    fn a_wall_one_cell_thick_around_a_room_stays_a_sound_region_when_simplified() {
        for close in [0, 12] {
            for step in 0..65 {
                let degrees = step as f64 * 0.7;
                let regions = single_face_room(degrees, close).regions(Connectivity::Eight);
                assert_eq!(regions.len(), 1, "{degrees} degrees");
                let region = &regions[0];
                assert_eq!(region.holes.len(), 1, "{degrees} degrees");
                assert_eq!(fault(region), None);
                let simple = region.simplified(1.5);
                assert_eq!(
                    fault(&simple),
                    None,
                    "{degrees} degrees, closing {close}: {simple:?}"
                );
                assert_eq!(simple.holes.len(), 1);
                // Either face moves by the tolerance at most, the wall does
                // not fall to a line, and its staircases go.
                let (before, after) = (region.area(), simple.area());
                assert!(
                    (after - before).abs() <= 1.5 * region.perimeter() && after > 0.5 * before,
                    "{degrees} degrees, closing {close}: {before} to {after}"
                );
                let corners = |region: &Region| region.outer.len() + region.holes[0].len();
                assert!(
                    corners(&simple) <= 40,
                    "{degrees} degrees, closing {close}: {} of {}",
                    corners(&simple),
                    corners(region)
                );
            }
        }
    }

    #[test]
    fn thin_parts_that_touch_at_corners_do_not_fold_over_when_simplified() {
        // A strip one cell thick with two cells that hang on by a corner:
        // all of it within the tolerance of one line.
        let ring = [
            [0.0, 4.0],
            [4.0, 4.0],
            [4.0, 5.0],
            [5.0, 5.0],
            [5.0, 6.0],
            [4.0, 6.0],
            [4.0, 7.0],
            [3.0, 7.0],
            [3.0, 6.0],
            [4.0, 6.0],
            [4.0, 5.0],
            [0.0, 5.0],
        ];
        let region = Region {
            outer: ring.to_vec(),
            holes: Vec::new(),
        };
        assert_eq!(fault(&region), None);
        let simple = region.simplified(1.5);
        assert_eq!(fault(&simple), None, "{simple:?}");
        assert!(simple.outer.len() < ring.len());
        assert!(simple.area() > 0.5 * region.area());
        assert_eq!(simple.outer, simplify_ring(&ring, 1.5));

        // Two cells that share a corner: the ring passes through it twice.
        let pinched = &mask(&[".#", "#."]).regions(Connectivity::Eight)[0];
        let simple = pinched.simplified(1.5);
        assert_eq!(fault(&simple), None, "{simple:?}");
        assert_eq!(simple.area(), 2.0);
        // An empty cell that is open to the outside through a corner only:
        // the ring around it must not close on itself.
        let bay = &mask(&["##.", "#.#", "###"]).regions(Connectivity::Four)[0];
        assert_eq!(bay.holes.len(), 0);
        assert_eq!(bay.outer.len(), 10);
        assert_eq!(bay.outer.iter().filter(|at| **at == [2.0, 2.0]).count(), 2);
        for tolerance in [0.5, 1.0, 1.5, 3.0] {
            let simple = bay.simplified(tolerance);
            assert_eq!(fault(&simple), None, "{simple:?}");
            assert!(simple.area() > 0.4 * bay.area(), "{simple:?}");
        }
    }

    /// A mask of one or more paths one cell wide that wander with straight
    /// and diagonal steps: thin parts, loops and cells that touch at corners.
    fn wandering_paths(rng: &mut Rng, size: u32, paths: usize) -> Mask {
        let mut mask = Mask::new(GridFrame::new([0.0, 0.0], 1.0, size, size).unwrap());
        let mut draw = |limit: u64| (rng.next_u64() % limit) as i64;
        for _ in 0..paths {
            let (mut x, mut y) = (draw(size as u64), draw(size as u64));
            let (mut dx, mut dy) = (1, 0);
            for _ in 0..size * 6 {
                mask.set(x as u32, y as u32, true);
                if draw(10) < 3 {
                    (dx, dy) = (draw(3) - 1, draw(3) - 1);
                }
                x = (x + dx).clamp(0, size as i64 - 1);
                y = (y + dy).clamp(0, size as i64 - 1);
            }
        }
        mask
    }

    #[test]
    fn speckle_and_thin_paths_simplify_to_sound_regions() {
        let mut rng = Rng::new(5);
        let frame = GridFrame::new([0.0, 0.0], 1.0, 36, 36).unwrap();
        let mut regions = 0;
        let mut removed = 0;
        for round in 0..90 {
            let mask = if round % 3 == 2 {
                wandering_paths(&mut rng, 24 + round as u32 % 16, 1 + round % 2)
            } else {
                let density = 0.35 + 0.3 * (round % 4) as f64 / 3.0;
                Mask::from_fn(frame, |_, _| rng.unit() < density)
            };
            for connectivity in [Connectivity::Four, Connectivity::Eight] {
                for region in mask.regions(connectivity) {
                    assert_eq!(fault(&region), None);
                    for tolerance in [0.5, 1.5, 4.0] {
                        let simple = region.simplified(tolerance);
                        assert_eq!(
                            fault(&simple),
                            None,
                            "tolerance {tolerance}: {region:?} to {simple:?}"
                        );
                        assert_eq!(simple.holes.len(), region.holes.len());
                        removed += region.outer.len() - simple.outer.len();
                        // A ring on its own stays sound as well, and keeps
                        // the same corners whichever one it starts at.
                        let alone = Region {
                            outer: simplify_ring(&region.outer, tolerance),
                            holes: Vec::new(),
                        };
                        assert_eq!(fault(&alone), None, "{:?} to {alone:?}", region.outer);
                        let mut turned = region.outer.clone();
                        turned.rotate_left(region.outer.len() / 3);
                        let mut turned = simplify_ring(&turned, tolerance);
                        let mut straight = alone.outer;
                        for ring in [&mut turned, &mut straight] {
                            ring.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
                        }
                        assert_eq!(turned, straight, "{:?}", region.outer);
                    }
                    regions += 1;
                }
            }
        }
        // Enough regions, and corners that went, for the test to mean something.
        assert!(regions > 2_000 && removed > 5_000, "{regions} {removed}");
    }

    #[test]
    fn the_same_corners_stay_at_national_grid_coordinates() {
        // Cells of 20 mm far from the origin: the rounding of coordinates
        // must not decide which of two equal corners stays.
        let mut rng = Rng::new(21);
        let mut rings = 0;
        for round in 0..40 {
            let size = 12 + round % 30;
            let near = GridFrame::new([0.0, 0.0], 1.0, size, size).unwrap();
            let far = GridFrame::new([207_000.13, 474_000.77], 0.02, size, size).unwrap();
            let density = 0.3 + 0.01 * round as f64;
            let cells: Vec<bool> = (0..near.cells()).map(|_| rng.unit() < density).collect();
            let occupied = |x, y| cells[near.index(x, y)];
            let near = Mask::from_fn(near, occupied).regions(Connectivity::Eight);
            let far = Mask::from_fn(far, occupied).regions(Connectivity::Eight);
            assert_eq!(near.len(), far.len());
            for (near, far) in near.iter().zip(&far) {
                for cells in [0.5, 1.5, 4.0] {
                    let near = near.kept_corners(cells, &mut || Ok(())).unwrap();
                    let far = far.kept_corners(cells * 0.02, &mut || Ok(())).unwrap();
                    assert_eq!(near, far);
                    rings += near.len();
                }
            }
        }
        assert!(rings > 3_000, "{rings}");
    }

    /// A strip with teeth one cell wide, five high and one apart.
    fn comb(teeth: usize) -> Vec<[f64; 2]> {
        let mut ring = vec![[0.0, 0.0], [(2 * teeth - 1) as f64, 0.0]];
        for tooth in (0..teeth).rev() {
            ring.push([(2 * tooth + 1) as f64, 6.0]);
            ring.push([(2 * tooth) as f64, 6.0]);
            if tooth > 0 {
                ring.push([(2 * tooth) as f64, 1.0]);
                ring.push([(2 * tooth - 1) as f64, 1.0]);
            }
        }
        ring
    }

    #[test]
    fn a_comb_of_equal_teeth_takes_work_in_proportion_to_its_length() {
        // Every tooth is as far from the line along the comb as the next.
        // Splitting at the first of them looks at all the others again for
        // every tooth: hundreds of millions of corners for this comb.
        let ring = comb(20_000);
        let corners = Corners::new(&[&ring], 1.5);
        let (kept, examined) = reduce(&corners, 0, ring.len(), 1.5, &mut || Ok(())).unwrap();
        assert_eq!(ring.len(), 80_000);
        assert!(examined < 100 * ring.len() as u64, "{examined}");
        // Every tooth is still there.
        assert!(kept.len() > 40_000 && kept.len() < ring.len());
        let simple: Vec<[f64; 2]> = kept.iter().map(|index| ring[*index]).collect();
        let area = ring_signed_area(&ring);
        assert!((ring_signed_area(&simple) - area).abs() < 0.2 * area);
        assert_eq!(kept, simplify_ring_indices(&ring, 1.5));
    }

    #[test]
    fn kept_corners_name_what_stays_and_stop_when_told() {
        let shape = mask(&[
            "..########..",
            ".##......##.",
            "##..####..##",
            "#..##..##..#",
            "##..####..##",
            ".##......##.",
            "..########..",
        ]);
        let regions = shape.regions(Connectivity::Eight);
        assert_eq!(regions.len(), 2);
        for region in &regions {
            assert_eq!(region.holes.len(), 1);
            let mut calls = 0;
            let kept = region
                .kept_corners(1.0, &mut || {
                    calls += 1;
                    Ok(())
                })
                .unwrap();
            assert!(calls > 4);
            assert_eq!(kept.len(), 2);
            assert!(kept
                .iter()
                .all(|ring| ring.windows(2).all(|pair| pair[0] < pair[1])));
            let simple = region.simplified(1.0);
            let pick = |ring: &[[f64; 2]], kept: &[usize]| -> Vec<[f64; 2]> {
                kept.iter().map(|index| ring[*index]).collect()
            };
            assert_eq!(simple.outer, pick(&region.outer, &kept[0]));
            assert_eq!(simple.holes[0], pick(&region.holes[0], &kept[1]));
            assert!(simple.outer.len() < region.outer.len());
            assert_eq!(fault(&simple), None);
            // The same work stops at the third question.
            let mut left = 3;
            let stopped = region.kept_corners(1.0, &mut || {
                left -= 1;
                if left == 0 {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(stopped, Err(LoadError::Cancelled)));
            assert_eq!(left, 0);
        }
    }
}
