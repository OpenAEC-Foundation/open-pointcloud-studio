//! The colours of a mesh per triangle, reduced to a small palette.
//!
//! A triangle takes the mean of the colours of its corners. The colours of
//! all triangles are counted in a grid of 5 bits per channel, and the grid
//! cells are split by median cut into at most [`MAX_COLOURS`] boxes: the
//! box that holds the most triangles over the widest range of colour is cut
//! in two at its median along its widest channel, until there are enough
//! boxes or none can be cut. A palette colour is the mean of the triangles
//! in its box. A mesh of few colours keeps them as they are; a scan, whose
//! colours gather in a few tones, gets its palette where its colours are
//! rather than spread over a fixed grid.
//!
//! The CAD writer turns every palette colour into entities of their own,
//! and the IFC writer into an entry of a colour list, so the bound keeps
//! both the number of entities and the number of materials a program makes
//! of them small.

/// The most colours a mesh is written in.
pub(crate) const MAX_COLOURS: usize = 256;

/// Bits per channel of the grid the colours are counted in.
const BITS: u32 = 5;
const CELLS: usize = 1 << (3 * BITS);

/// The palette of a mesh and the palette entry of every triangle.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceColours {
    pub palette: Vec<[u8; 3]>,
    pub of_triangle: Vec<u8>,
}

impl FaceColours {
    /// The triangles of every palette entry, in their order in the mesh.
    pub fn groups(&self) -> Vec<Vec<u32>> {
        let mut groups = vec![Vec::new(); self.palette.len()];
        for (triangle, &entry) in self.of_triangle.iter().enumerate() {
            groups[usize::from(entry)].push(triangle as u32);
        }
        groups
    }
}

fn cell(rgb: [u8; 3]) -> usize {
    let [r, g, b] = rgb.map(|value| usize::from(value >> (8 - BITS)));
    (r << (2 * BITS)) | (g << BITS) | b
}

fn channels(cell: usize) -> [usize; 3] {
    let mask = (1 << BITS) - 1;
    [cell >> (2 * BITS), (cell >> BITS) & mask, cell & mask]
}

/// The colours of the triangles of a mesh with a colour per vertex, or
/// nothing for a mesh without colours.
pub(crate) fn face_colours(
    colors: Option<&[[u8; 3]]>,
    vertex_count: usize,
    triangles: &[[u32; 3]],
) -> Option<FaceColours> {
    let colors = colors.filter(|colors| colors.len() == vertex_count && !triangles.is_empty())?;
    let mut count = vec![0u64; CELLS];
    let mut sum = vec![[0u64; 3]; CELLS];
    let cells: Vec<u16> = triangles
        .iter()
        .map(|triangle| {
            let corners = triangle.map(|index| colors[index as usize]);
            let mean: [u8; 3] = std::array::from_fn(|channel| {
                let total: u32 = corners.iter().map(|rgb| u32::from(rgb[channel])).sum();
                ((total + 1) / 3) as u8
            });
            let at = cell(mean);
            count[at] += 1;
            for channel in 0..3 {
                sum[at][channel] += u64::from(mean[channel]);
            }
            at as u16
        })
        .collect();

    // Median cut over the occupied cells.
    let mut boxes: Vec<Vec<usize>> = vec![(0..CELLS).filter(|&at| count[at] > 0).collect()];
    while boxes.len() < MAX_COLOURS {
        let widest = |members: &[usize]| -> (usize, usize) {
            (0..3)
                .map(|channel| {
                    let values = members.iter().map(|&at| channels(at)[channel]);
                    let range = values.clone().max().unwrap_or(0) - values.min().unwrap_or(0);
                    (range, channel)
                })
                .max()
                .unwrap_or((0, 0))
        };
        let Some((chosen, _)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, members)| members.len() > 1)
            .map(|(index, members)| {
                let population: u64 = members.iter().map(|&at| count[at]).sum();
                (index, population * widest(members).0 as u64)
            })
            .max_by_key(|&(index, priority)| (priority, std::cmp::Reverse(index)))
        else {
            break;
        };
        let mut members = std::mem::take(&mut boxes[chosen]);
        let (_, channel) = widest(&members);
        members.sort_unstable_by_key(|&at| (channels(at)[channel], at));
        let population: u64 = members.iter().map(|&at| count[at]).sum();
        // The first cell past half of the triangles, but never the first or
        // past the last, so both halves hold a cell.
        let mut running = 0;
        let mut split = members.len() - 1;
        for (position, &at) in members.iter().enumerate() {
            running += count[at];
            if 2 * running >= population {
                split = position + 1;
                break;
            }
        }
        let split = split.clamp(1, members.len() - 1);
        let upper = members.split_off(split);
        boxes[chosen] = members;
        boxes.push(upper);
    }

    let mut entry_of_cell = vec![0u8; CELLS];
    let palette = boxes
        .iter()
        .enumerate()
        .map(|(entry, members)| {
            let mut total = [0u64; 3];
            let mut population = 0;
            for &at in members {
                entry_of_cell[at] = entry as u8;
                population += count[at];
                for channel in 0..3 {
                    total[channel] += sum[at][channel];
                }
            }
            total.map(|value| ((value + population / 2) / population) as u8)
        })
        .collect();
    Some(FaceColours {
        palette,
        of_triangle: cells
            .into_iter()
            .map(|at| entry_of_cell[usize::from(at)])
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn few_colours_are_kept_and_many_are_reduced_to_the_palette() {
        let none = face_colours(None, 3, &[[0, 1, 2]]);
        assert!(none.is_none());
        // A triangle takes the mean of its corners.
        let colors = [
            [255, 0, 0],
            [255, 0, 0],
            [255, 0, 0],
            [0, 0, 255],
            [30, 60, 90],
        ];
        let triangles = [[0, 1, 2], [3, 3, 3], [0, 1, 2], [4, 4, 4]];
        let found = face_colours(Some(&colors), 5, &triangles).unwrap();
        assert_eq!(found.palette.len(), 3);
        let of = |triangle: usize| found.palette[usize::from(found.of_triangle[triangle])];
        assert_eq!(of(0), [255, 0, 0]);
        assert_eq!(of(1), [0, 0, 255]);
        assert_eq!(of(2), [255, 0, 0]);
        assert_eq!(of(3), [30, 60, 90]);
        let groups = found.groups();
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 4);

        // Every colour of the cube: no more than the palette holds, and
        // every triangle near its own colour.
        let mut colors = Vec::new();
        for r in (0..256).step_by(8) {
            for g in (0..256).step_by(8) {
                for b in (0..256).step_by(8) {
                    colors.push([r as u8, g as u8, b as u8]);
                }
            }
        }
        let triangles: Vec<[u32; 3]> = (0..colors.len() as u32).map(|at| [at; 3]).collect();
        let found = face_colours(Some(&colors), colors.len(), &triangles).unwrap();
        assert_eq!(found.palette.len(), MAX_COLOURS);
        for (triangle, rgb) in colors.iter().enumerate() {
            let entry = found.palette[usize::from(found.of_triangle[triangle])];
            let distance: i32 = (0..3)
                .map(|channel| (i32::from(entry[channel]) - i32::from(rgb[channel])).abs())
                .max()
                .unwrap();
            assert!(distance <= 64, "{rgb:?} as {entry:?}");
        }
    }
}
