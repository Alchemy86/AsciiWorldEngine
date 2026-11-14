//! The cell grid: the frame buffer every ASCII program draws into.
//!
//! A cell is a glyph, the colour it is drawn in, and optionally the colour of
//! the rest of the cell behind it. Next to those planes the grid keeps a depth
//! buffer, so a renderer that draws in any order still gets the nearest thing
//! in each cell, and a one-byte mark plane a program can use to find cells it
//! drew a particular way, such as text that must not be drawn over.
//!
//! Every plane is a flat row-major `Vec`, so getting a frame out is a walk over
//! bytes, never an object graph.

/// One frame of cells. See the module docs.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// The glyph in each cell, one byte, `b' '` for none.
    pub ch: Vec<u8>,
    /// The glyph's colour: 3 bytes a cell, `[r, g, b]`.
    pub rgb: Vec<u8>,
    /// **The background colour plane**: 3 bytes a cell, `[r, g, b]`, the colour
    /// of the part of the cell the glyph does not cover.
    ///
    /// While shading is off nothing writes it, `has_bg` stays false, and every
    /// output path skips the plane entirely — which is why the plain look costs
    /// exactly what it would cost with no background plane at all.
    pub bg: Vec<u8>,
    /// Whether anything on this frame actually wrote a background. While it is
    /// false the output paths skip the background plane entirely and `put`
    /// does not bother clearing it.
    pub has_bg: bool,
    /// **Shaded cells on or off** — the mode, held on the grid because the
    /// grid is the thing that has a background plane.
    ///
    /// Off, `put` writes the glyph, its colour and its depth and nothing else.
    /// On, the same glyphs in the same colours are drawn over a background of
    /// the surface they belong to (`color::shade`). Shading never changes WHICH
    /// character is drawn anywhere.
    pub shade: bool,
    /// **Which cells were drawn with `put_marked`.** One byte a cell, and only
    /// touched on a frame that marked something. Bookkeeping, not paint: no
    /// output path reads it. It exists so a program can find what it drew
    /// later without guessing at colours.
    pub mark: Vec<u8>,
    pub has_marks: bool,
    depth: Vec<f32>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Grid {
            cols,
            rows,
            ch: vec![b' '; cols * rows],
            rgb: vec![0; cols * rows * 3],
            bg: vec![0; cols * rows * 3],
            has_bg: false,
            shade: false,
            mark: vec![0; cols * rows],
            has_marks: false,
            depth: vec![f32::INFINITY; cols * rows],
        }
    }

    /// A new size, keeping the shading mode. Contents are lost.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let shade = self.shade;
        *self = Grid::new(cols, rows);
        self.shade = shade;
    }

    /// Blank every cell and reset the depth buffer, ready for a new frame.
    pub fn clear(&mut self) {
        self.ch.fill(b' ');
        self.rgb.fill(0);
        if self.has_bg {
            self.bg.fill(0);
            self.has_bg = false;
        }
        if self.has_marks {
            self.mark.fill(0);
            self.has_marks = false;
        }
        self.depth.fill(f32::INFINITY);
    }

    /// Draw `ch` in `rgb` at depth `d`, unless something nearer is already in
    /// the cell. Off the grid is ignored, so a caller can draw past the edges.
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, ch: u8, rgb: [u8; 3], d: f32) {
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return;
        }
        let i = y as usize * self.cols + x as usize;
        if d > self.depth[i] {
            return;
        }
        self.ch[i] = ch;
        self.depth[i] = d;
        self.rgb[i * 3] = rgb[0];
        self.rgb[i * 3 + 1] = rgb[1];
        self.rgb[i * 3 + 2] = rgb[2];
        if self.shade {
            // The gaps around this glyph are the surface it is drawn on, a
            // third as bright. One branch, predicted the same way on every cell
            // of a frame, and nothing at all while the mode is off.
            let s = crate::color::shade(rgb, ch);
            self.bg[i * 3] = s[0];
            self.bg[i * 3 + 1] = s[1];
            self.bg[i * 3 + 2] = s[2];
            self.has_bg = true;
        }
        if self.has_marks {
            self.mark[i] = 0;
        }
    }

    /// **The background alone, where no glyph is drawn.**
    ///
    /// A surface whose texture comes out blank in a cell still has a colour,
    /// and with shading on that colour belongs in the gap. This writes the
    /// shaded surface colour into the background plane and touches nothing
    /// else: not the glyph, not the colour plane and not the depth buffer,
    /// because no glyph was drawn and a cell with no glyph in it has never
    /// occluded anything.
    ///
    /// **A cell that has a character on it keeps that character's own
    /// background, and this will not take it.** A blank means "see through to
    /// whatever is behind", so the surface a cell is painted in has to be the
    /// surface the character on it came from, or the character floats over a
    /// colour that is not its own. The depth test is the same one `put` makes.
    #[inline]
    pub fn put_bg(&mut self, x: i32, y: i32, rgb: [u8; 3], d: f32) {
        if !self.shade || x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return;
        }
        let i = y as usize * self.cols + x as usize;
        if self.ch[i] != b' ' || d > self.depth[i] {
            return;
        }
        let s = crate::color::shade(rgb, b' ');
        self.bg[i * 3] = s[0];
        self.bg[i * 3 + 1] = s[1];
        self.bg[i * 3 + 2] = s[2];
        self.has_bg = true;
    }

    /// One whole row of background, shaded once and written straight in, with
    /// no depth test. For a surface that is one colour all the way across and
    /// drawn before anything else, such as a sky.
    pub fn bg_row(&mut self, y: i32, rgb: [u8; 3]) {
        if !self.shade || y < 0 || y >= self.rows as i32 {
            return;
        }
        let s = crate::color::shade(rgb, b' ');
        let row = y as usize * self.cols;
        for i in row..row + self.cols {
            self.bg[i * 3] = s[0];
            self.bg[i * 3 + 1] = s[1];
            self.bg[i * 3 + 2] = s[2];
        }
        self.has_bg = true;
    }

    /// Was this cell drawn with `put_marked`, and not drawn over since?
    #[inline]
    pub fn is_marked(&self, x: i32, y: i32) -> bool {
        if !self.has_marks {
            return false;
        }
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return false;
        }
        self.mark[y as usize * self.cols + x as usize] != 0
    }

    /// What the depth buffer holds at this cell; -1 off the grid. Lets a pass
    /// find out whether something it just drew actually won its cell.
    #[inline]
    pub fn depth_at(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return -1.0;
        }
        self.depth[y as usize * self.cols + x as usize]
    }

    /// Is every cell of this horizontal run on screen and in front of whatever
    /// is already drawn there? For things that must be drawn whole or not at
    /// all, such as a word that would read as a different word if clipped.
    #[inline]
    pub fn run_is_clear(&self, x: i32, y: i32, n: i32, d: f32) -> bool {
        if y < 0 || y >= self.rows as i32 || x < 0 || x + n > self.cols as i32 {
            return false;
        }
        let row = y as usize * self.cols;
        (0..n as usize).all(|i| d <= self.depth[row + x as usize + i])
    }

    /// The same as `put`, and it marks the cell (`is_marked`). An ordinary
    /// glyph in an ordinary colour; it does not shade its background.
    #[inline]
    pub fn put_marked(&mut self, x: i32, y: i32, ch: u8, rgb: [u8; 3], d: f32) {
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return;
        }
        let i = y as usize * self.cols + x as usize;
        if d > self.depth[i] {
            return;
        }
        self.ch[i] = ch;
        self.depth[i] = d;
        self.rgb[i * 3] = rgb[0];
        self.rgb[i * 3 + 1] = rgb[1];
        self.rgb[i * 3 + 2] = rgb[2];
        self.has_marks = true;
        self.mark[i] = 1;
    }

    /// Pack into one flat frame buffer: 4 bytes per cell, `[glyph, r, g, b]`,
    /// row-major. One buffer, one copy, no object graph.
    pub fn pack_into(&self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.cols * self.rows * 4);
        for i in 0..self.cols * self.rows {
            out.push(self.ch[i]);
            out.push(self.rgb[i * 3]);
            out.push(self.rgb[i * 3 + 1]);
            out.push(self.rgb[i * 3 + 2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_glyph_wins_whatever_order_it_is_drawn_in() {
        let mut g = Grid::new(4, 2);
        g.put(1, 1, b'f', [1, 2, 3], 9.0);
        g.put(1, 1, b'n', [4, 5, 6], 2.0);
        g.put(1, 1, b'x', [7, 8, 9], 5.0);
        assert_eq!(g.ch[5], b'n');
        assert_eq!(&g.rgb[15..18], &[4, 5, 6]);
        g.put(-1, 0, b'o', [0; 3], 0.0);
        g.put(4, 0, b'o', [0; 3], 0.0);
        assert!(g.ch.iter().filter(|&&c| c == b'o').count() == 0);
    }

    #[test]
    fn shading_fills_only_the_background_plane() {
        let mut plain = Grid::new(3, 1);
        let mut shaded = Grid::new(3, 1);
        shaded.shade = true;
        for g in [&mut plain, &mut shaded] {
            g.put(0, 0, b'.', [200, 100, 50], 1.0);
            g.put_bg(1, 0, [200, 100, 50], 1.0);
        }
        assert_eq!(plain.ch, shaded.ch);
        assert_eq!(plain.rgb, shaded.rgb);
        assert!(!plain.has_bg && shaded.has_bg);
        assert_ne!(&shaded.bg[0..3], &[0, 0, 0]);
        assert_ne!(&shaded.bg[3..6], &[0, 0, 0]);
    }

    #[test]
    fn a_mark_lasts_until_something_draws_over_it() {
        let mut g = Grid::new(2, 1);
        g.put_marked(0, 0, b'A', [255; 3], 1.0);
        assert!(g.is_marked(0, 0) && !g.is_marked(1, 0));
        g.put(0, 0, b'B', [255; 3], 0.5);
        assert!(!g.is_marked(0, 0));
    }
}
