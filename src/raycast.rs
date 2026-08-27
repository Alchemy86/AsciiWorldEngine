//! One DDA per screen column, with an occlusion cull that is the difference
//! between a playable frame and a slideshow.
//!
//! The ray is `forward + right * tan(bearing)` with `forward` a unit vector, so
//! `ray . forward == 1` and the DDA's own `t` parameter **is** the
//! perpendicular (fisheye-corrected) distance. No cosine correction needed.
//!
//! The cull: march near to far keeping `best_ratio`, the highest
//! `(top - eye)/distance` any accepted cell reached. A cell behind that is
//! completely hidden — a nearer wall's silhouette strictly contains it, because
//! a farther wall's *bottom* edge is always higher on screen than a nearer
//! one's. Once no cell at the current distance could beat `best_ratio` even at
//! the world's maximum height, the whole column is finished. At street level
//! that ends most columns inside 60 units instead of 400.

use crate::camera::Camera;
use crate::project::Projection;
use crate::world::World;

/// Near geometry reaches its brightness floor here, so nothing beyond it is
/// visible on the near path.
pub const NEAR_MAX: f32 = 150.0;
/// The far skyline runs out to here.
pub const FAR_MAX: f32 = 400.0;
/// Hard cap on visible cells per column. The cull normally stops far short.
const MAX_HITS_PER_COL: usize = 48;

#[derive(Clone, Copy, Default)]
pub struct Hit {
    /// Perpendicular distance to the cell's near face.
    pub dist: f32,
    /// Perpendicular distance to the cell's far face — the roof band's far edge.
    pub exit: f32,
    /// 0 = an X-facing wall was hit, 1 = a Z-facing wall.
    pub side: u8,
    pub cell_x: i32,
    pub cell_z: i32,
    pub height: u8,
    /// Hit coordinate along the wall: world Z when `side == 0`, world X when 1.
    pub along: f32,
}

/// Per-column hit lists, kept in one flat vector and reused between frames.
pub struct Rays {
    pub hits: Vec<Hit>,
    /// `[start, end)` into `hits` for each column.
    pub spans: Vec<(u32, u32)>,
}

impl Rays {
    pub fn new(cols: usize) -> Self {
        Rays { hits: Vec::with_capacity(cols * 8), spans: vec![(0, 0); cols] }
    }

    #[inline]
    pub fn column(&self, x: usize) -> &[Hit] {
        let (a, b) = self.spans[x];
        &self.hits[a as usize..b as usize]
    }

    /// Cast every column against wherever we are.
    ///
    /// **The mode is resolved ONCE, here, and never again inside the march.**
    /// `World::cell` has a branch in it — city or room — and a branch is
    /// nothing until you take it thirty thousand times a frame, which is what
    /// a DDA over a hundred and eighty columns does. Handing `cast_with` a
    /// concrete cell source instead monomorphises the loop and the street pays
    /// exactly what it paid before rooms existed.
    pub fn cast(&mut self, world: &World, cam: &Camera, proj: &Projection) {
        let head = world.max_height();
        match &world.place {
            crate::world::Place::Outdoors => {
                self.cast_with(cam, proj, head, |x, z| world.city_cell(x, z))
            }
            crate::world::Place::Indoors(room) => self.cast_with(cam, proj, head, |x, z| {
                match room.at(x, z) {
                    Some(c) => c,
                    None => world.city_cell(x, z),
                }
            }),
        }
    }

    fn cast_with<F: Fn(i32, i32) -> crate::world::Cell>(
        &mut self,
        cam: &Camera,
        proj: &Projection,
        head_max: f32,
        cell_at: F,
    ) {
        self.hits.clear();
        if self.spans.len() != proj.cols {
            self.spans.resize(proj.cols, (0, 0));
        }
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let eye = proj.eye;
        let top_ratio = proj.top_ratio();
        // Only meaningful while something in the world can still rise above the
        // eye; from an elevated vista nothing can, and the cull falls back to
        // the distance cap.
        let head_room = head_max - eye;

        for x in 0..proj.cols {
            let start = self.hits.len() as u32;
            let t = proj.col_tan[x];
            let dx = fx + rx * t;
            let dz = fz + rz * t;

            let mut map_x = cam.x.floor() as i32;
            let mut map_z = cam.z.floor() as i32;
            let ddx = if dx == 0.0 { 1e30 } else { (1.0 / dx).abs() };
            let ddz = if dz == 0.0 { 1e30 } else { (1.0 / dz).abs() };
            let (step_x, mut side_x) = if dx < 0.0 {
                (-1, (cam.x - map_x as f32) * ddx)
            } else {
                (1, (map_x as f32 + 1.0 - cam.x) * ddx)
            };
            let (step_z, mut side_z) = if dz < 0.0 {
                (-1, (cam.z - map_z as f32) * ddz)
            } else {
                (1, (map_z as f32 + 1.0 - cam.z) * ddz)
            };

            let mut best_ratio = f32::NEG_INFINITY;
            let mut n = 0usize;
            loop {
                let (d, side) = if side_x < side_z {
                    let d = side_x;
                    side_x += ddx;
                    map_x += step_x;
                    (d, 0u8)
                } else {
                    let d = side_z;
                    side_z += ddz;
                    map_z += step_z;
                    (d, 1u8)
                };
                if d >= FAR_MAX || n >= MAX_HITS_PER_COL {
                    break;
                }
                // Nothing further can beat what we already have.
                if head_room > 0.0 && best_ratio > 0.0 && head_room / d <= best_ratio {
                    break;
                }
                if best_ratio >= top_ratio {
                    break;
                }
                if d < 1e-3 {
                    continue;
                }

                let cell = cell_at(map_x, map_z);
                if cell.height == 0 {
                    continue;
                }
                let exit = side_x.min(side_z);
                let h = cell.height as f32;
                // The cell's highest point on screen: its near roof edge when
                // we are below it, its far roof edge when we are above.
                let ratio = if h > eye { (h - eye) / d } else { (h - eye) / exit };
                if ratio <= best_ratio {
                    continue;
                }
                best_ratio = ratio;
                self.hits.push(Hit {
                    dist: d,
                    exit,
                    side,
                    cell_x: map_x,
                    cell_z: map_z,
                    height: cell.height,
                    along: if side == 0 { cam.z + d * dz } else { cam.x + d * dx },
                });
                n += 1;
            }
            self.spans[x] = (start, self.hits.len() as u32);
        }
    }
}
