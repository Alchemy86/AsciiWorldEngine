//! Frames to disk. An `.svg` (colour, every glyph in place, readable by a
//! browser or by ffmpeg built with librsvg) and a `.txt` of the same glyphs.

use crate::output::{grid_to_svg, grid_to_text};
use crate::Grid;

/// Write `dir/name.svg` and `dir/name.txt`, creating `dir` if needed. Returns
/// the path stem, `dir/name`.
pub fn write_frame(g: &Grid, dir: &str, name: &str, title: &str) -> String {
    let _ = std::fs::create_dir_all(dir);
    let stem = format!("{dir}/{name}");
    let _ = std::fs::write(format!("{stem}.svg"), grid_to_svg(g, title));
    let _ = std::fs::write(format!("{stem}.txt"), grid_to_text(g));
    stem
}
