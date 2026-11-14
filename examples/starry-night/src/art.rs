//! The living painting: what each cell shows at time `t`.
//!
//! Three things move, and the painting is only magical with all three:
//!
//!   1. **It pixels in.** Every cell has its own moment to appear, drawn from a
//!      hash of its position, so the picture arrives a scatter at a time. A
//!      cell arrives as a hot, flickering character and cools to its colour.
//!   2. **It flows.** Each cell samples the painting not at its own position
//!      but through a slowly drifting sum of sine waves, so the sky's swirls
//!      ripple like wet paint. The waves are gentle while the picture forms
//!      and swell once it has.
//!   3. **It shimmers.** The glyph is chosen from the brightness the cell sees
//!      THROUGH that flow, so as the paint slides under a cell its character
//!      changes with it — and where the paint has a strong edge, the glyph
//!      follows the direction of the brush stroke instead.

use ascii_engine::{ramp, rng, Grid};

use crate::image::{luma, Image};

/// When the first cell appears, and how long until the last one has.
const REVEAL_START: f32 = 0.3;
const REVEAL_SPAN: f32 = 3.6;
/// How long an arriving cell flickers before it settles into the painting.
const ARRIVE: f32 = 0.45;
/// The flow's reach, as a fraction of the picture's width, once it is fully
/// up; and how much of that is already there while the picture is forming.
const FLOW: f32 = 0.016;
const FLOW_EARLY: f32 = 0.25;
/// Glyph jitter in ramp steps: the texture's shimmer even where the paint is
/// still.
const SHIMMER: f32 = 0.42;
/// How strong an edge must be before the cell draws the stroke's direction
/// rather than its density: the brightness range crossed per picture width,
/// measured in screen space (a cell is two widths tall).
const STROKE_EDGE: f32 = 20.0;
/// The characters the picture is made of, densest first once sorted by how
/// much of the cell each one inks.
const DENSITY: &[u8] = b"0XZUVO*+=:.";
/// How far a character's colour is brightened, at most, and pushed away from
/// grey, to make up for a glyph inking only part of its cell.
const INK_GAIN: f32 = 1.9;
const INK_SATURATE: f32 = 1.6;
/// Brightness is raised to this before it picks a glyph, so the mid-tones of
/// the sky draw in light strokes and only the stars and moon go dense.
const GLYPH_GAMMA: f32 = 1.35;
/// How much a cell's difference from its neighbourhood adds to the brightness
/// that picks its glyph, on top of its own brightness.
const LOCAL_CONTRAST: f32 = 1.2;

pub struct Scene {
    /// The painting at working resolution: everything is measured off this.
    img: Image,
    /// The painting averaged down to the grid it is drawn on, twice over:
    /// `fine` at about two samples a cell, `broad` at a quarter of the cells.
    /// Sampling the big picture straight onto cells would alias the brush
    /// strokes into noise; `broad` is the neighbourhood a cell's brightness is
    /// judged against, which is what makes a swirl's light and dark bands
    /// come out as different characters.
    fitted: Option<(Frame, Image, Image)>,
    /// Brightness at the 2nd and 98th percentile, which become 0 and 1.
    lo: f32,
    hi: f32,
    ramp: Vec<u8>,
    seed: i32,
    /// How many times taller a cell is than it is wide where this is shown.
    pub cell_aspect: f32,
}

/// Where the picture sits on a grid: its top-left cell and its size in cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The largest frame of the picture's own shape that fits `cols` x `rows`,
/// centred, for cells `cell_aspect` times taller than they are wide.
pub fn fit(img_w: usize, img_h: usize, cols: usize, rows: usize, cell_aspect: f32) -> Frame {
    let a = img_w as f32 / img_h as f32 * cell_aspect;
    let (mut w, mut h) = (cols as f32, cols as f32 / a);
    if h > rows as f32 {
        h = rows as f32;
        w = h * a;
    }
    let (w, h) = ((w.round() as i32).max(1), (h.round() as i32).max(1));
    Frame { x: (cols as i32 - w) / 2, y: (rows as i32 - h) / 2, w, h }
}

/// A well-mixed value in 0..1 for a cell. `rng::hash3` alone leaves runs
/// down a column at small coordinates — a whole column arriving together is
/// a visible stripe in the scatter — so its output goes through a full
/// avalanche (murmur3's finaliser) first.
fn scatter(x: i32, y: i32, seed: i32) -> f32 {
    let mut h = rng::hash3(x, y, seed);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A character's colour, lifted: a glyph covers a third of its cell at best,
/// so the painting's own colour drawn as ink reads far darker than it is. The
/// lift is a gain on all three channels — brighter without going greyer — up
/// to whatever takes the strongest channel to full, then a push away from grey
/// so the blues stay deep and the stars stay yellow.
#[inline]
fn ink(c: [f32; 3]) -> [f32; 3] {
    let top = c[0].max(c[1]).max(c[2]).max(1e-3);
    let gain = INK_GAIN.min(1.0 / top);
    let c = [c[0] * gain, c[1] * gain, c[2] * gain];
    let l = luma(c);
    let sat = |v: f32| (l + (v - l) * INK_SATURATE).clamp(0.0, 1.0);
    [sat(c[0]), sat(c[1]), sat(c[2])]
}

#[inline]
fn rgb8(c: [f32; 3]) -> [u8; 3] {
    [(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8]
}

impl Scene {
    pub fn new(painting: &Image, seed: i32) -> Scene {
        let img = painting.shrink(480, 380);
        let mut l: Vec<f32> = img.px.iter().map(|&p| luma(p)).collect();
        l.sort_by(|a, b| a.total_cmp(b));
        let lo = l[l.len() * 2 / 100];
        let hi = l[l.len() * 98 / 100];
        Scene { img, fitted: None, lo, hi: hi.max(lo + 0.05), ramp: ramp::by_coverage(DENSITY), seed, cell_aspect: 2.0 }
    }

    pub fn frame_for(&self, cols: usize, rows: usize) -> Frame {
        fit(self.img.w, self.img.h, cols, rows, self.cell_aspect)
    }

    /// The moment the whole picture has arrived.
    pub fn formed_at() -> f32 {
        REVEAL_START + REVEAL_SPAN + ARRIVE
    }

    /// Draw the painting as it is `t` seconds after it started appearing.
    pub fn draw(&mut self, g: &mut Grid, t: f32) {
        let f = self.frame_for(g.cols, g.rows);
        if self.fitted.as_ref().map(|x| x.0) != Some(f) {
            let fine = self.img.shrink(f.w as usize * 2, (f.h as f32 * self.cell_aspect) as usize);
            let broad = self.img.shrink((f.w as usize / 4).max(1), (f.h as f32 * self.cell_aspect / 8.0) as usize + 1);
            self.fitted = Some((f, fine, broad));
        }
        let (_, fine, broad) = self.fitted.as_ref().expect("fitted above");
        let flow = FLOW * (FLOW_EARLY + (1.0 - FLOW_EARLY) * smoothstep(Self::formed_at() - 1.5, Self::formed_at() + 2.5, t));
        let aspect = self.img.w as f32 / self.img.h as f32;
        let (eu, ev) = (1.0 / f.w as f32, 1.0 / f.h as f32);
        let tau = std::f32::consts::TAU;
        for cy in 0..f.h {
            let v = (cy as f32 + 0.5) / f.h as f32;
            for cx in 0..f.w {
                let u = (cx as f32 + 0.5) / f.w as f32;
                let h = scatter(cx, cy, self.seed);
                let arrive = REVEAL_START + REVEAL_SPAN * h.powf(0.8);
                let age = t - arrive;
                if age < 0.0 {
                    continue;
                }

                let du = flow
                    * (0.62 * (tau * (1.6 * v + 0.9 * u) + 1.25 * t).sin()
                        + 0.38 * (tau * (3.3 * v - 0.4 * u) - 0.83 * t).sin());
                let dv = flow
                    * aspect
                    * (0.6 * (tau * (1.25 * u - 0.7 * v) + 1.05 * t).cos()
                        + 0.4 * (tau * (2.7 * u + 1.1 * v) + 0.61 * t).sin());
                let (su, sv) = (u + du, v + dv);
                let c = fine.sample(su, sv);
                let l = luma(c);
                let local = l - luma(broad.sample(su, sv));
                let b = ((l - self.lo + LOCAL_CONTRAST * local) / (self.hi - self.lo)).clamp(0.0, 1.0);

                let gx = (luma(fine.sample(su + eu, sv)) - luma(fine.sample(su - eu, sv))) / 2.0;
                let gy = (luma(fine.sample(su, sv + ev)) - luma(fine.sample(su, sv - ev))) / 4.0;
                let edge = (gx * gx + gy * gy).sqrt() * f.w as f32 / (self.hi - self.lo);

                let shimmer = SHIMMER * (2.1 * t + tau * scatter(cy, cx, self.seed ^ 0x5eed)).sin();
                let mut glyph = if edge > STROKE_EDGE && (0.12..0.9).contains(&b) {
                    stroke(gx, gy)
                } else {
                    self.ramp[ramp::index(self.ramp.len(), b.powf(GLYPH_GAMMA), shimmer)]
                };
                let mut colour = ink(c);

                if age < ARRIVE {
                    let k = age / ARRIVE;
                    let flick = (scatter(cx, cy, (t * 24.0) as i32) * 4.0) as usize;
                    glyph = self.ramp[flick.min(3)];
                    let hot = [1.0, 0.93, 0.62];
                    let w = (1.0 - k) * (1.0 - k);
                    for i in 0..3 {
                        colour[i] = colour[i] + (hot[i] - colour[i]) * w;
                    }
                }
                g.put(f.x + cx, f.y + cy, glyph, rgb8(colour), 0.0);
            }
        }
    }
}

/// The character that runs along a brush stroke, given the brightness
/// gradient across it (screen axes, y down). A stroke runs at right angles to
/// its gradient.
fn stroke(gx: f32, gy: f32) -> u8 {
    let (sx, sy) = (-gy, gx);
    let mut deg = (-sy).atan2(sx).to_degrees();
    if deg < 0.0 {
        deg += 180.0;
    }
    if deg >= 180.0 {
        deg -= 180.0;
    }
    match deg {
        d if !(22.5..157.5).contains(&d) => b'-',
        d if d < 67.5 => b'/',
        d if d < 112.5 => b'|',
        _ => b'\\',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_runs_across_its_gradient() {
        assert_eq!(stroke(1.0, 0.0), b'|', "brightness changing left to right is a vertical edge");
        assert_eq!(stroke(0.0, 1.0), b'-', "changing top to bottom is a horizontal edge");
        assert_eq!(stroke(1.0, 1.0), b'/', "brighter down and right: the edge rises to the right");
        assert_eq!(stroke(1.0, -1.0), b'\\');
    }

    #[test]
    fn the_picture_keeps_its_shape_on_tall_cells() {
        let f = fit(960, 760, 200, 60, 2.0);
        assert_eq!(f.h, 60);
        assert!((f.w as f32 / (f.h as f32 * 2.0) - 960.0 / 760.0).abs() < 0.03, "{f:?}");
        assert_eq!(f.x, (200 - f.w) / 2);
    }

    #[test]
    fn it_pixels_in_and_then_it_moves() {
        let img = Image::from_rgb8(
            4,
            4,
            &(0..48).map(|i| (i * 5) as u8).collect::<Vec<u8>>(),
        );
        let mut s = Scene::new(&img, 7);
        let mut lit = |t: f32| {
            let mut g = Grid::new(40, 20);
            s.draw(&mut g, t);
            (g.ch.iter().filter(|&&c| c != b' ').count(), g.ch, g.rgb)
        };
        let (none, _, _) = lit(0.0);
        let (some, _, _) = lit(REVEAL_START + REVEAL_SPAN * 0.4);
        let (all, a, ac) = lit(Scene::formed_at() + 4.0);
        assert_eq!(none, 0, "nothing before the reveal starts");
        assert!(some > 0 && some < all, "a scatter part way: {some} of {all}");
        let (_, b, bc) = lit(Scene::formed_at() + 5.0);
        assert!(a != b || ac != bc, "a formed painting must still be moving");
    }
}
