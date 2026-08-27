//! The picture. Every glyph and every colour on screen is decided here.
//!
//! A wall column is NOT one shaded ramp. It is three different pictures
//! stacked, and they do not share a model:
//!
//!   * above ~2.6 world units — the **tower body**: a window lattice over a
//!     brightness ramp.
//!   * below ~2.6 world units — the **storefront**: floor ledges, lit panes and
//!     a signage row. This band is where the bright multi-coloured street line
//!     comes from.
//!   * beyond 150 units — the **far skyline**, with its own falloff
//!     (`(400-d)/252`, not the near path's `1 - d/150`) and its own fixed-pitch
//!     lattice, leaving roughly a third of a distant facade blank.
//!
//! Getting those three right, and keeping the un-lit parts of a facade *blank*,
//! is what makes a building read as a building instead of as speckle.
//!
//! There is a fourth picture only the elevated vista needs: with the camera
//! above the rooftops, cells shorter than the eye contribute a **roof band**
//! between their far and near edges.

use crate::camera::Camera;
use crate::entities::{Population, Sky};
use crate::interior::{fit, floor, Fitting, Interior};
use crate::palette::*;
use crate::project::Projection;
use crate::raycast::{Rays, NEAR_MAX};
use crate::rng::noise;
use crate::world::{surface, Cell, Place, Placed, Prop, World, BLOCK, BLOCK_BUILT};

/// Near geometry falls off linearly to here.
const FALLOFF: f32 = 150.0;
/// The far skyline's own curve. Apply the near falloff to distant towers and
/// the entire skyline renders black — the most consequential constant here.
const FAR_ZERO: f32 = 400.0;
const FAR_SPAN: f32 = 252.0;
/// Ground rows past this are not drawn: past it the ground is one flat tone
/// and every glyph costs the same as a lit one.
const MAX_GROUND: f32 = 143.25;
/// World height at which the storefront band ends and the tower body begins.
const STOREFRONT_TOP: f32 = 2.6;
/// How tall a street doorway is drawn. Under the storefront band's own top, so
/// an entrance sits inside the shopfront rather than cutting across it.
const DOOR_TOP: f32 = 2.35;
/// Nothing indoors is further off than this, and most rooms are half it. It is
/// a cap on the ceiling and floor passes, not a falloff — how fast a room goes
/// dark is `Interior::fall`, and that is per room.
const ROOM_MAX: f32 = 72.0;

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub ch: Vec<u8>,
    pub rgb: Vec<u8>,
    /// Background colour, 3 bytes a cell, `[0,0,0]` everywhere the frame is
    /// simply drawn on black — which is almost all of it.
    ///
    /// It exists for exactly one thing: a registration plate is black
    /// characters ON yellow, and that is not something a foreground colour can
    /// say. Both output paths skip it while it is black, so a frame with no
    /// plates in it costs what it always did.
    pub bg: Vec<u8>,
    /// Whether anything on this frame actually painted one. While it is false
    /// both output paths skip the background entirely and `put` does not
    /// bother clearing it, so a run with `--no-plates` costs exactly what it
    /// cost before plates existed.
    ///
    /// **Nothing sets it any more.** A plate used to be the one thing on the
    /// frame that painted a background; it is drawn out of characters now, so
    /// this is false on every frame and the plane is skipped everywhere. It is
    /// left in place — and costs nothing while it is black — rather than being
    /// torn out of the ANSI, SVG and wasm paths in the same breath as a change
    /// to how a plate looks.
    pub has_panels: bool,
    /// **Which cells are part of a registration plate.** One byte a cell, and
    /// only touched on a frame that drew one.
    ///
    /// A plate is now made of the same coloured-glyph-on-black everything else
    /// is made of, which is the point — but it still has to be findable, and by
    /// something better than guessing at colours. Three things read it and all
    /// three have to be right: the rain pass, which must not knock a character
    /// out of a registration; `--plate-shot`, which scores frames on the plates
    /// in them; and the test that says a plate on screen is never a
    /// registration other than its own.
    pub plate: Vec<u8>,
    pub has_plates: bool,
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
            has_panels: false,
            plate: vec![0; cols * rows],
            has_plates: false,
            depth: vec![f32::INFINITY; cols * rows],
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        *self = Grid::new(cols, rows);
    }

    fn clear(&mut self) {
        self.ch.fill(b' ');
        self.rgb.fill(0);
        if self.has_panels {
            self.bg.fill(0);
            self.has_panels = false;
        }
        if self.has_plates {
            self.plate.fill(0);
            self.has_plates = false;
        }
        self.depth.fill(f32::INFINITY);
    }

    #[inline]
    fn put(&mut self, x: i32, y: i32, ch: u8, rgb: [u8; 3], d: f32) {
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
        if self.has_panels {
            self.bg[i * 3] = 0;
            self.bg[i * 3 + 1] = 0;
            self.bg[i * 3 + 2] = 0;
        }
        if self.has_plates {
            self.plate[i] = 0;
        }
    }

    /// Is this cell part of a plate?
    #[inline]
    pub fn is_plate(&self, x: i32, y: i32) -> bool {
        if !self.has_plates {
            return false;
        }
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return false;
        }
        self.plate[y as usize * self.cols + x as usize] != 0
    }

    /// Is every cell of this horizontal run on screen and in front of whatever
    /// is already drawn there?
    ///
    /// A registration plate is drawn all or not at all. Half a plate — clipped
    /// by a building corner, or by a car passing in front of it — reads as a
    /// SHORTER REGISTRATION, which is worse than no plate: `1 RG` behind
    /// another car came out as `1 R`. When the whole run cannot be drawn the
    /// caller falls back to the blank panel, which can be occluded as much as
    /// it likes and still says only "a plate".
    #[inline]
    fn run_is_clear(&self, x: i32, y: i32, n: i32, d: f32) -> bool {
        if y < 0 || y >= self.rows as i32 || x < 0 || x + n > self.cols as i32 {
            return false;
        }
        let row = y as usize * self.cols;
        (0..n as usize).all(|i| d <= self.depth[row + x as usize + i])
    }

    /// The same as `put`, and it marks the cell as belonging to a plate. An
    /// ordinary glyph in an ordinary colour — the mark is bookkeeping, not
    /// paint, and nothing in any of the three output paths reads it.
    #[inline]
    fn put_plate(&mut self, x: i32, y: i32, ch: u8, rgb: [u8; 3], d: f32) {
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
        self.has_plates = true;
        self.plate[i] = 1;
    }

    /// Pack into the flat frame buffer the frontends read: 4 bytes per cell,
    /// `[glyph, r, g, b]`, row-major. One buffer, one copy, no object graph.
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

pub struct Renderer {
    /// Nearest wall distance per column, for depth-testing the population.
    nearest: Vec<f32>,
    /// Street furniture in range this frame. Kept here so the per-frame
    /// gathering allocates once, at startup, and never again.
    props: Vec<Placed>,
    /// Plates waiting to be drawn, held over until every vehicle BODY is in the
    /// depth buffer. Drawn in the same pass as the cars, a plate can only be
    /// tested against the cars drawn before it, and a car drawn afterwards
    /// clips it — which turns `1 RG` into `1 R`. Allocated once.
    plate_q: Vec<PlateDraw>,
    /// Distance to the ceiling plane at each screen row. The ground has a
    /// standing table on `Projection` because its height never changes; a
    /// ceiling's does, per room, so it is filled in here at the top of the
    /// pass and is the same handful of divides `set_view` already does.
    ceil_d: Vec<f32>,
}

/// One plate, held over to the second pass.
#[derive(Clone, Copy)]
struct PlateDraw {
    dist: f32,
    row: i32,
    span: i32,
    c0: i32,
    c1: i32,
    key: u16,
}

impl Renderer {
    pub fn new(cols: usize) -> Self {
        Renderer {
            nearest: vec![f32::INFINITY; cols],
            props: Vec::with_capacity(512),
            plate_q: Vec::with_capacity(crate::entities::VEH_COUNT),
            ceil_d: Vec::new(),
        }
    }

    pub fn render(
        &mut self,
        grid: &mut Grid,
        world: &World,
        cam: &Camera,
        proj: &Projection,
        rays: &Rays,
        pop: &Population,
        sky_fx: &Sky,
        time: f32,
    ) {
        grid.clear();
        if self.nearest.len() != proj.cols {
            self.nearest.resize(proj.cols, f32::INFINITY);
        }
        // **Which of the two pictures this is** — decided once, here, off the
        // mode the engine is in. Not a test inside the wall pass, and not a
        // flag the glyph tables consult: a street and a room are two different
        // pictures made of different things, and the only thing they share is
        // the raycaster that found the geometry for both.
        match &world.place {
            Place::Outdoors => {
                self.sky(grid, cam, proj, time);
                self.ground(grid, world, cam, proj);
                self.walls(grid, world, proj, rays);
                self.props(grid, world, cam, proj);
                self.population(grid, cam, proj, pop);
                self.rain(grid, cam, proj, sky_fx);
            }
            Place::Indoors(room) => {
                // The same sky as outside, because it IS the same sky and you
                // can see it through the glazing. The ceiling paints over it
                // wherever there is a ceiling, which is most of the frame.
                self.sky(grid, cam, proj, time);
                self.room_ceiling(grid, room, cam, proj);
                self.room_floor(grid, world, room, cam, proj);
                self.room_walls(grid, world, room, proj, rays);
                self.room_fixtures(grid, room, cam, proj);
            }
        }
    }

    // ---- sky ------------------------------------------------------------
    /// A night sky: a haze band in the seven rows above the horizon, where the
    /// city's own light washes it out, and above that a **star field in three
    /// magnitudes** that slowly twinkles.
    ///
    /// The base layer is sparse: green `.` above `hash > 0.994`, `*` above
    /// `0.9985`, in `hsl(135, 100%, …)`. That alone gives a sky you have to
    /// look for. Two things on top are what make it a sky you look AT:
    ///
    ///   * **Magnitude.** A handful of stars per screen are drawn bright and
    ///     near-white with a `+` cross, the way the eye actually sorts a night
    ///     sky — a few obvious ones and a wash of faint ones behind them.
    ///   * **A twinkle.** Each star's brightness rides a slow sine on its own
    ///     phase. It costs one multiply on cells that already passed the
    ///     threshold, and it is the difference between a starfield and a
    ///     texture.
    ///
    /// Stars are keyed to world BEARING and elevation, not to screen position,
    /// so they hold still while you turn under them.
    fn sky(&mut self, g: &mut Grid, cam: &Camera, proj: &Projection, t: f32) {
        let sky_rows = (proj.horizon.ceil() as i32).clamp(0, proj.rows as i32);
        for x in 0..proj.cols {
            let dir_angle = cam.yaw + proj.col_tan[x].atan();
            let key_x = (480.0 * dir_angle) as i32;
            for y in 0..sky_rows {
                let elev = ((proj.horizon - y as f32) / proj.proj_y).atan() + proj.pitch;
                let n = noise(key_x, (480.0 * elev) as i32 + 8000);
                let to_horizon = proj.horizon - y as f32;
                if to_horizon < 7.0 {
                    let t = (7.0 - to_horizon) / 7.0;
                    if n < 0.12 * t {
                        g.put(x as i32, y, b'.', hsl(135.0, 100.0, 13.0 + 8.0 * t), f32::INFINITY);
                    }
                    continue;
                }
                if n <= 0.988 {
                    continue;
                }
                // Its own phase, so neighbours do not pulse in unison.
                let ph = noise(key_x + 501, (480.0 * elev) as i32 + 131) * 6.2832;
                let twinkle = 0.78 + 0.22 * (t * 1.7 + ph).sin();
                // Higher in the sky is darker: the city's glow only reaches so
                // far up, and that gradient is most of what gives the sky depth.
                let lift = ((to_horizon - 7.0) / 24.0).min(1.0);
                let (ch, hue, sat, base) = if n > 0.9992 {
                    (b'+', 52.0, 26.0, 62.0)          // first magnitude, near-white
                } else if n > 0.9975 {
                    (b'*', 96.0, 62.0, 34.0)
                } else if n > 0.9955 {
                    (b'*', 135.0, 100.0, 22.0)        // the base layer, bright
                } else {
                    (b'.', 135.0, 100.0, 12.0)        // and faint
                };
                g.put(
                    x as i32,
                    y,
                    ch,
                    hsl(hue, sat, base * twinkle * (1.0 - 0.28 * lift)),
                    f32::INFINITY,
                );
            }
        }
    }

    // ---- ground ---------------------------------------------------------
    /// Textured off the world's SURFACE class, and keyed to the 32-cell block:
    /// kerbs at block offsets 4 and 11, centre lines at 7 and 8, lane dashes at
    /// 6 and 9, pavement slab lines every 4 cells. That even split across
    /// surfaces is what fills the lower half of the frame with striation
    /// instead of leaving it dark.
    fn ground(&mut self, g: &mut Grid, world: &World, cam: &Camera, proj: &Projection) {
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let y0 = (proj.horizon.ceil() as i32).max(0);
        for x in 0..proj.cols {
            let t = proj.col_tan[x];
            let dx = fx + rx * t;
            let dz = fz + rz * t;
            for y in y0..proj.rows as i32 {
                let d = proj.ground_depth[y as usize];
                if !(d > 0.0) || d > MAX_GROUND {
                    continue;
                }
                let wx = cam.x + dx * d;
                let wz = cam.z + dz * d;
                let cx = wx.floor() as i32;
                let cz = wz.floor() as i32;
                // Outdoors by construction — `render` chose this pass off the
                // mode — so ask the city directly rather than going through the
                // dispatch on every one of several thousand ground cells.
                let c = world.city_cell(cx, cz);
                if c.height != 0 {
                    continue;
                }
                // World units one character row covers here.
                let dn = proj.ground_depth[((y + 1) as usize).min(proj.rows - 1)];
                let dp = proj.ground_depth[(y - 1).max(0) as usize];
                let scale = (d / proj.proj_x).max((0.5 * (dn - dp).abs()).min(8.0));
                let (glyph, colour) = ground_glyph(&c, wx, wz, cx, cz, d, scale);
                if glyph != b' ' {
                    g.put(x as i32, y, glyph, colour, d);
                }
            }
        }
    }

    // ---- walls ----------------------------------------------------------
    fn walls(
        &mut self,
        g: &mut Grid,
        world: &World,
        proj: &Projection,
        rays: &Rays,
    ) {
        let rows = proj.rows as i32;
        // A building's bright vertical corner is drawn where the wall SEGMENT
        // changes between screen columns — NOT at every sixth sub-column.
        // Drawing it per sub-column shreds the facade into vertical hash.
        let mut prev: Option<crate::raycast::Hit> = None;
        for x in 0..proj.cols {
            let hits = rays.column(x);
            self.nearest[x] = hits.first().map(|h| h.dist).unwrap_or(f32::INFINITY);
            let lead = hits.first().copied();
            let edge_v = match (lead, prev) {
                (Some(l), Some(p)) => {
                    l.side != p.side || (l.cell_x != p.cell_x && l.cell_z != p.cell_z)
                }
                (Some(_), None) => true,
                _ => false,
            };
            prev = lead;
            let col_vignette = 0.8 + 0.2 * (1.0 - (2.0 * (x as f32 / proj.cols as f32 - 0.5)).abs());

            // NEAR TO FAR, with a running horizon. `ybuf` is the topmost row
            // anything nearer has already covered; a farther wall is only ever
            // visible ABOVE it, because a farther wall's bottom edge is always
            // higher on screen than a nearer one's. Clipping to it turns the
            // pass from O(hits x span) into O(cols x rows): from an elevated
            // vista, where a few thousand cells are in frame at once, that is
            // most of the frame cost.
            let mut ybuf = rows;
            for hit in hits.iter() {
                if ybuf <= 0 {
                    break;
                }
                if hit.dist <= 0.02 {
                    continue;
                }
                let c = world.city_cell(hit.cell_x, hit.cell_z);
                let h = hit.height as f32;

                // The roof, when we are above it. Only an elevated vista ever
                // sees this; at street level the band is empty.
                if proj.eye > h {
                    let t = self.roof(g, proj, hit, &c, h, x as i32, ybuf);
                    ybuf = ybuf.min(t);
                }

                if hit.dist > NEAR_MAX {
                    let t = self.far_wall(g, proj, hit, &c, h, x as i32, ybuf);
                    ybuf = ybuf.min(t);
                    continue;
                }

                let t = self.facade(
                    g, world.grain, proj, hit, &c, h, x as i32, ybuf, edge_v, col_vignette, 1.0,
                );
                ybuf = ybuf.min(t);
            }
        }
    }

    /// **One near facade, drawn.** Everything a wall of the city looks like:
    /// the tower body's window lattice, the storefront band under it, a lit
    /// billboard where a block carries one, the bright corner where the wall
    /// segment turns, and — where the wall carries an entrance — the doorway.
    ///
    /// It is its own method for one reason: it is called twice. Once from
    /// `walls`, for the street. Once from `room_walls`, for the city seen
    /// through a window, where `dim` is under one so what is beyond the glass
    /// sits back from the room you are standing in. At `dim == 1.0` it is the
    /// street pass exactly as it always was, to the byte.
    ///
    /// Returns the topmost row it covered, for the caller's running horizon,
    /// or `i32::MAX` when it covered nothing.
    ///
    /// `inline` is load-bearing, not decoration. This runs once per visible
    /// wall cell — a few hundred at street level and well over a thousand from
    /// an elevated vista — and out of line it costs a call with eleven
    /// arguments where the body used to keep its whole working set in
    /// registers. Measured at +0.05 ms a frame from the vista without it, which
    /// is more than the entire feature this file was opened to add. It is
    /// emitted twice, for the street and for the view out of a window; that is
    /// the same trade `grid_to_ansi` makes with its two loop bodies.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn facade(
        &mut self,
        g: &mut Grid,
        grain: i32,
        proj: &Projection,
        hit: &crate::raycast::Hit,
        c: &crate::world::Cell,
        h: f32,
        x: i32,
        ybuf: i32,
        edge_v: bool,
        col_vignette: f32,
        dim: f32,
    ) -> i32 {
        let rows = proj.rows as i32;
        let r0 = proj.row_of(h, hit.dist).ceil().max(0.0) as i32;
        let r1raw = proj.row_of(0.0, hit.dist);
        if r1raw < 0.0 {
            return i32::MAX;
        }
        let r1 = (r1raw.floor() as i32).min(rows - 1);
        if r0 > r1 {
            return i32::MAX;
        }
        // Shade over the wall's WHOLE span even where it is clipped, so
        // the vertical bulge down a facade does not change as something
        // moves in front of it.
        let span = r1 - r0;
        let r1 = r1.min(ybuf - 1);
        if r0 > r1 {
            return i32::MAX;
        }
        let hue = c.hue as f32;
        let sat = c.sat as f32;
        let lit_p = c.lit as f32 / 100.0;
        let arch = c.arch;
        // Architecture overrides the cell's own lattice on sculpted forms.
        let style = match arch {
            1 => 3u8,
            2 => 0,
            3 => 2,
            _ => c.win,
        };
        let sign_hue = (hue + 88.0 + 41.0 * style as f32) % 360.0;
        let wall_pos = hit.along;
        let col_n = (6.0 * (hit.along - hit.along.floor())) as i32;
        let base_b = (1.0 - (hit.dist / FALLOFF).min(1.0)).max(0.0)
            * (0.9 + 0.1 * noise(7 * hit.cell_x + 131 * hit.side as i32, 7 * hit.cell_z + 3))
            * (if hit.side == 0 { 0.96 } else { 0.82 })
            * col_vignette
            * dim;
        let sf_row = proj.row_of(STOREFRONT_TOP, hit.dist).ceil() as i32;
        let sf_tmax = r1 - (r0 + 1).max(sf_row);
        // **The doorway.** `Cell::door >= 5` is the wall at the back of an
        // entrance bay — the generator put it there, and a ray only ever
        // reaches it through the cell the generator took out of the facade in
        // front of it. Drawing it lit is what makes a way in something you can
        // SEE from across the street rather than something you find by walking
        // into walls.
        let door_row = if c.door >= 5 {
            (proj.row_of(DOOR_TOP, hit.dist).ceil().max(0.0) as i32).max(r0 + 1)
        } else {
            i32::MAX
        };
        // Hoisted out of the row loop: almost no wall in the city carries a
        // doorway, and the ones that do carry it over a known band.
        let doorway = c.door >= 5 && door_row <= r1;
        let bld = if c.plan != 0 && sf_row <= r1 { Some(building_of(hit.cell_x, hit.cell_z, c.plan, BLOCK, grain)) } else { None };
        let sign = billboard_on(proj, hit, h, hue, wall_pos);

        for y in r0..=r1 {
            let b = base_b * v_profile(y - r0, span);
            if b < 0.05 {
                continue;
            }
            let wy = proj.height_at(y as usize, hit.dist);
            let scale = (hit.dist / proj.proj_x)
                .max(hit.dist * proj.row_span[(y as usize).min(proj.rows - 1)]);
            let q = quant(scale);
            let wq = (2.0 * wall_pos / q).floor() as i32;
            let hq = (2.0 * wy / q).floor() as i32;
            let sn = surf_tex(wall_pos, wy, scale, 3 * hit.cell_x, 5 * hit.cell_z);
            let glyph;
            let colour;

            if doorway && y >= door_row {
                // ---- a lit doorway ------------------------------------
                let u = 6.0 * (wall_pos - wall_pos.floor());
                let dr = r1 - y;
                let n = (r1 - door_row).max(1);
                if y == door_row {
                    glyph = b'=';
                    colour = hsl(44.0, 92.0, 62.0 + 26.0 * b); // the lintel
                } else if dr == 0 {
                    glyph = b'=';
                    colour = hsl(44.0, 58.0, 48.0 + 30.0 * b); // the step
                } else if !(0.9..5.1).contains(&u) {
                    glyph = b'|';
                    colour = hsl(44.0, 78.0, 52.0 + 30.0 * b); // the jambs
                } else {
                    // The light coming out of it, brightest at the head.
                    let k = 1.0 - dr as f32 / n as f32;
                    glyph = if sn > 0.62 { b'#' } else if sn > 0.3 { b'%' } else { b'@' };
                    colour = hsl(42.0, 74.0, 30.0 + 26.0 * k + 22.0 * b);
                }
            } else if let Some(s) = sign.as_ref().filter(|s| y >= s.top && y <= s.bottom) {
                // ---- lit billboard panel ------------------------
                let n = s.bottom - s.top + 1;
                let gy = ((7 * (y - s.top) / n) as i32).min(6);
                let gx = ((17.0 * s.u) as i32).min(16);
                let mut on = false;
                let gl = if gy == 0 || gy == 6 {
                    if gx == 0 || gx == 16 { b'+' } else { b'=' }
                } else if gx == 0 || gx == 16 {
                    b'|'
                } else {
                    on = s.grid[(gy - 1) as usize].as_bytes()
                        .get((gx - 1) as usize)
                        .copied()
                        .unwrap_or(b'.') == b'#';
                    if on { if sn > 0.5 { b'#' } else { b'@' } } else { b'.' }
                };
                let l = if on {
                    58.0 + 25.0 * b
                } else if gl == b'.' {
                    12.0 + 14.0 * b
                } else {
                    35.0 + 22.0 * b
                };
                glyph = gl;
                colour = hsl(s.hue, if on { 100.0 } else { 65.0 }, l);
            } else if let Some(bld) = bld.as_ref().filter(|_| y > r0 && y >= sf_row) {
                // ---- storefront of a laid-out building ----------
                let p = bld.style.pattern;
                let dr = r1 - y;
                if dr == 0 {
                    glyph = if p == 4 { b'-' } else { b'_' };
                    colour = hsl(bld.style.frame_hue, 38.0, 30.0 + 22.0 * b);
                } else if dr == sf_tmax && sf_tmax >= 3 {
                    let i = ((2.0 * wall_pos).floor() as i32).unsigned_abs() as usize
                        % bld.label_len.max(1);
                    glyph = bld.label[i];
                    colour = hsl(bld.style.accent_hue, 90.0, 54.0 + 22.0 * b);
                } else if col_n == 0 || col_n == 5 {
                    glyph = EDGE_CH[p];
                    colour = hsl(bld.style.frame_hue, 62.0, 38.0 + 30.0 * b);
                } else if dr % FLOOR_PITCH[p] == 0 {
                    glyph = LEDGE_CH[p];
                    colour = hsl(bld.style.frame_hue, 62.0, 38.0 + 30.0 * b);
                } else {
                    let on = sn < PANE_LIT[p];
                    let run = if on { PANE_ON[p] } else { PANE_OFF[p] };
                    glyph = run[(hq + col_n).unsigned_abs() as usize % run.len()];
                    colour = if on {
                        hsl(bld.style.light_hue, 82.0, 46.0 + 25.0 * b)
                    } else {
                        hsl(bld.style.glass_hue, 58.0, 15.0 + 22.0 * b)
                    };
                }
            } else if y > r0 && y >= sf_row {
                // ---- storefront of an unplanned wall ------------
                let dr = r1 - y;
                if dr == 0 {
                    glyph = b'_';
                    colour = hsl(hue, 25.0, 18.0 + 12.0 * b);
                } else if edge_v && dr < 3 {
                    glyph = b'|';
                    colour = hsl(hue, 100.0, 46.0 + 38.0 * b);
                } else if dr == sf_tmax && sf_tmax >= 3 {
                    glyph = b"$@%&"[((4.0 * sn) as usize).min(3)];
                    colour = hsl(sign_hue, 95.0, 56.0 + 16.0 * b);
                } else if sn < 0.12 + 0.5 * lit_p {
                    glyph = b'0';
                    colour = hsl(hue, 100.0, 56.0 + 16.0 * b);
                } else {
                    glyph = b':';
                    colour = hsl(hue + 6.0, 60.0, 30.0 + 18.0 * b);
                }
            } else {
                // ---- tower body --------------------------------
                let v4 = hq.rem_euclid(4);
                let u6 = wq.rem_euclid(6);
                let pane = match style {
                    0 => u6 % 3 == 1 && v4 == 1,
                    1 => u6 % 3 == 1 && v4 == 1 && sn < 0.5,
                    2 => (v4 == 1 || v4 == 2) && u6 % 2 == 0,
                    _ => u6 % 2 == 0 && sn < 0.7,
                };
                if edge_v && y < r1 {
                    glyph = if arch == 1 {
                        if wall_pos.rem_euclid(2.0) < 1.0 { b'/' } else { b'\\' }
                    } else {
                        b'|'
                    };
                    colour = hsl(hue, 100.0, 42.0 + 44.0 * b);
                } else if h >= 4.0 && pane {
                    if sn < lit_p {
                        glyph = b'0';
                        colour = hsl(hue, 100.0, 62.0 + 10.0 * b);
                    } else {
                        glyph = b':';
                        colour = hsl(hue, 45.0, 24.0 + 16.0 * b);
                    }
                } else if y == r0 && r0 > 0 && h >= 3.0 {
                    glyph = match arch {
                        1 => b'~',
                        2 => b'^',
                        3 => b'*',
                        _ => b'=',
                    };
                    colour = hsl(if arch == 2 { 48.0 } else { hue }, 100.0, 70.0);
                    if arch == 3 && h >= 20.0 && r0 >= 2 {
                        g.put(x, r0 - 2, if sn > 0.5 { b'*' } else { b'^' },
                              hsl(hue, 100.0, 82.0), hit.dist);
                        g.put(x, r0 - 1, b'|', hsl(hue, 100.0, 55.0), hit.dist);
                    } else if arch == 1 && h >= 25.0 && sn < 0.5 && r0 >= 1 {
                        g.put(x, r0 - 1, b'H',
                              hsl(hue, 70.0, 34.0 + 14.0 * b), hit.dist);
                    }
                } else if u6 % 3 == 1 {
                    glyph = b':';
                    colour = hsl(hue, 35.0, 16.0 + 14.0 * b);
                } else {
                    let idx = ((WALL_RAMP.len() - 1) as f32 * (1.0 - b)
                        + 0.55 * (sn - 0.5))
                        .round()
                        .clamp(0.0, (WALL_RAMP.len() - 1) as f32) as usize;
                    glyph = WALL_RAMP[idx];
                    colour = hsl(hue, sat, 38.0 + 26.0 * b);
                }
            }
            if glyph != b' ' {
                g.put(x, y, glyph, colour, hit.dist);
            }
        }
        r0
    }

    // ---- indoors --------------------------------------------------------
    // Four passes, and they are NOT the outdoor four with different constants.
    // A street is a ground plane, an open sky and facades; a room is a floor
    // slab, a CEILING you are under, and surfaces close enough to touch. The
    // one thing the two share is the raycaster, which found the geometry for
    // both without being told which it was looking at.

    /// **The ceiling.** The single strongest cue that you are indoors, and the
    /// only one of these four passes the street has no counterpart for at all:
    /// something solid over your head, lit, that gets closer as you look up.
    ///
    /// Lit strips on a pitch, structural beams across them, and dark plenum
    /// between. All three are keyed to WORLD cells, not to screen position, so
    /// the ceiling holds still while you walk under it.
    ///
    /// Where the sample falls outside the room there is nothing to draw: that
    /// is out of the window, and the sky pass and the wall pass have it.
    fn room_ceiling(&mut self, g: &mut Grid, room: &Interior, cam: &Camera, proj: &Projection) {
        if self.ceil_d.len() != proj.rows {
            self.ceil_d = vec![f32::INFINITY; proj.rows];
        }
        let cy = room.ceiling_y();
        for y in 0..proj.rows {
            self.ceil_d[y] = proj.plane_depth(y, cy);
        }
        // A car's ceiling stops at its own glass; over the WELL there is open
        // shaft, and what closes it off is the soffit at the top of the shaft,
        // tens of units up. Without this second plane the sky pass shows
        // through the roof of the building on the top floor.
        let head = if room.lift.is_some() { room.shaft_head() } else { f32::NAN };
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let inv_fall = 1.0 / room.fall;
        for x in 0..proj.cols {
            let t = proj.col_tan[x];
            let dx = fx + rx * t;
            let dz = fz + rz * t;
            for y in 0..proj.rows {
                let d = self.ceil_d[y];
                if !(d > 0.0) || d > ROOM_MAX {
                    continue;
                }
                let wx = cam.x + dx * d;
                let wz = cam.z + dz * d;
                let (cx, cz) = (wx.floor() as i32, wz.floor() as i32);
                let Some(c) = room.at(cx, cz) else { continue };
                if c.win == fit::WELL {
                    // Open shaft. The soffit that closes it is `head` up, at a
                    // different distance, so it takes its own sample — and only
                    // counts where THAT sample is over the well too.
                    if head.is_nan() {
                        continue;
                    }
                    let hd = proj.plane_depth(y, head);
                    if !(hd > 0.0) || hd > ROOM_MAX {
                        continue;
                    }
                    let (hx, hz) = (cam.x + dx * hd, cam.z + dz * hd);
                    let (kx, kz) = (hx.floor() as i32, hz.floor() as i32);
                    if !room.at(kx, kz).is_some_and(|k| k.win == fit::WELL) {
                        continue;
                    }
                    let n = noise(13 * kx + 5, 17 * kz + 3);
                    g.put(
                        x as i32,
                        y as i32,
                        if n > 0.7 { b'#' } else { b'=' },
                        hsl(CEIL_HUE, 8.0, 5.0 + 7.0 * n),
                        hd,
                    );
                    continue;
                }
                // A wall or a column reaches the ceiling, so the wall pass owns
                // that column of screen and this would only fight it.
                if c.height as f32 >= cy - 0.01 {
                    continue;
                }
                let b = room_light(room.ambient, d, inv_fall);
                // Which way the strips run, and which way they repeat.
                let (run, across) = if room.light_along_x { (cx, cz) } else { (cz, cx) };
                let lit = (across - room.light_phase).rem_euclid(room.light_pitch) == 0;
                let n = noise(11 * cx + 3, 7 * cz + 29);
                let (glyph, colour) = if lit {
                    // A fitting, not a painted line: the run is broken every
                    // few cells by its own housing, so it reads as a row of
                    // luminaires rather than as a stripe.
                    if run.rem_euclid(6) != 5 {
                        (b'=', hsl(room.light_hue, room.light_sat, 44.0 + 50.0 * b))
                    } else {
                        (b'#', hsl(room.light_hue, room.light_sat * 0.6, 18.0 + 26.0 * b))
                    }
                } else if run.rem_euclid(room.beam_pitch) == 0 {
                    (
                        if room.light_along_x { b'|' } else { b'-' },
                        hsl(CEIL_HUE, 10.0, 5.0 + 38.0 * b),
                    )
                } else if n > 0.74 {
                    (b':', hsl(CEIL_HUE, 8.0, 6.0 + 42.0 * b))
                } else {
                    (b'.', hsl(CEIL_HUE, 6.0, 4.0 + 44.0 * b))
                };
                // **A ceiling is opaque.** Leaving the darkest cells blank the
                // way the sky pass does lets the city show through the roof:
                // the plenum is dark, not absent.
                g.put(x as i32, y as i32, glyph, colour, d);
            }
        }
    }

    /// **The floor slab, and the street beyond it.** Inside the room this is
    /// the room's own material, with the pools the ceiling strips throw on it
    /// — that pairing is most of why the light reads as coming from above
    /// rather than from nowhere. Outside the room it is the actual pavement,
    /// through `ground_glyph`, because what you see out of the window is the
    /// city and not a picture of one.
    fn room_floor(
        &mut self,
        g: &mut Grid,
        world: &World,
        room: &Interior,
        cam: &Camera,
        proj: &Projection,
    ) {
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let y0 = (proj.horizon.ceil() as i32).max(0);
        let inv_fall = 1.0 / room.fall;
        for x in 0..proj.cols {
            let t = proj.col_tan[x];
            let dx = fx + rx * t;
            let dz = fz + rz * t;
            for y in y0..proj.rows as i32 {
                // The room's own slab first: on any floor but the ground one
                // it is nearer than the street, and on the ground one the two
                // are the same plane.
                let d = proj.plane_depth(y as usize, room.base);
                let mut drawn = false;
                if d > 0.0 && d <= ROOM_MAX {
                    let wx = cam.x + dx * d;
                    let wz = cam.z + dz * d;
                    let (cx, cz) = (wx.floor() as i32, wz.floor() as i32);
                    if let Some(c) = room.at(cx, cz) {
                        drawn = true;
                        if c.win == fit::WELL {
                            // The car's slab stops at its own glass. Down the
                            // shaft is the pit, on the ground plane — the same
                            // one the street stands on, because the shaft is
                            // cut all the way down to it.
                            let pd = proj.ground_depth[y as usize];
                            if pd > 0.0 && pd.is_finite() {
                                let (px, pz) = (cam.x + dx * pd, cam.z + dz * pd);
                                let (kx, kz) = (px.floor() as i32, pz.floor() as i32);
                                if room.at(kx, kz).is_some_and(|k| k.win == fit::WELL) {
                                    let n = noise(7 * kx + 11, 5 * kz + 19);
                                    g.put(
                                        x as i32,
                                        y,
                                        if n > 0.62 { b'#' } else { b'=' },
                                        hsl(206.0, 10.0, 3.0 + 8.0 * n),
                                        pd,
                                    );
                                }
                            }
                        } else if c.height == 0 {
                            let b = room_light(room.ambient, d, inv_fall);
                            let across = if room.light_along_x { cz } else { cx };
                            // Under a strip. The pool is what makes the light
                            // come from somewhere.
                            let pool = if (across - room.light_phase)
                                .rem_euclid(room.light_pitch)
                                == 0
                            {
                                0.30
                            } else {
                                0.0
                            };
                            let dn = proj.plane_depth((y as usize + 1).min(proj.rows - 1), room.base);
                            let dp = proj.plane_depth((y - 1).max(0) as usize, room.base);
                            let scale =
                                (d / proj.proj_x).max((0.5 * (dn - dp).abs()).min(8.0));
                            let (glyph, colour) =
                                floor_glyph(room, &c, wx, wz, cx, cz, scale, b + pool);
                            if glyph != b' ' {
                                g.put(x as i32, y, glyph, colour, d);
                            }
                        }
                    }
                }
                if drawn {
                    continue;
                }
                // Out of the window, or out through the door: the street.
                let d = proj.ground_depth[y as usize];
                if !(d > 0.0) || d > MAX_GROUND {
                    continue;
                }
                let wx = cam.x + dx * d;
                let wz = cam.z + dz * d;
                let (cx, cz) = (wx.floor() as i32, wz.floor() as i32);
                if room.contains(cx, cz) {
                    continue;
                }
                let c = world.city_cell(cx, cz);
                if c.height != 0 {
                    continue;
                }
                let dn = proj.ground_depth[((y + 1) as usize).min(proj.rows - 1)];
                let dp = proj.ground_depth[(y - 1).max(0) as usize];
                let scale = (d / proj.proj_x).max((0.5 * (dn - dp).abs()).min(8.0));
                let (glyph, colour) = ground_glyph(&c, wx, wz, cx, cz, d, scale);
                if glyph != b' ' {
                    g.put(x as i32, y, glyph, dim_rgb(colour, OUTSIDE_DIM), d);
                }
            }
        }
    }

    /// **The room's own surfaces, and the city past its glazing.**
    ///
    /// The same near-to-far walk with a running horizon the street uses, over
    /// the same hits from the same raycaster. The only question asked per hit
    /// is `Interior::contains` — a bounds check — and it decides whether this
    /// is a surface an arm's length away or a building across the road.
    fn room_walls(
        &mut self,
        g: &mut Grid,
        world: &World,
        room: &Interior,
        proj: &Projection,
        rays: &Rays,
    ) {
        let rows = proj.rows as i32;
        let cy = room.ceiling_y();
        let inv_fall = 1.0 / room.fall;
        let mut prev: Option<crate::raycast::Hit> = None;
        for x in 0..proj.cols {
            let hits = rays.column(x);
            self.nearest[x] = hits.first().map(|h| h.dist).unwrap_or(f32::INFINITY);
            let lead = hits.first().copied();
            let edge_v = match (lead, prev) {
                (Some(l), Some(p)) => {
                    l.side != p.side || (l.cell_x != p.cell_x && l.cell_z != p.cell_z)
                }
                (Some(_), None) => true,
                _ => false,
            };
            prev = lead;
            let col_vignette =
                0.8 + 0.2 * (1.0 - (2.0 * (x as f32 / proj.cols as f32 - 0.5)).abs());

            let mut ybuf = rows;
            for hit in hits.iter() {
                if ybuf <= 0 {
                    break;
                }
                if hit.dist <= 0.02 {
                    continue;
                }

                // --- the city, through the glass --------------------------
                let Some(c) = room.at(hit.cell_x, hit.cell_z) else {
                    let c = world.city_cell(hit.cell_x, hit.cell_z);
                    let h = hit.height as f32;
                    if proj.eye > h {
                        ybuf = ybuf.min(self.roof(g, proj, hit, &c, h, x as i32, ybuf));
                    }
                    if hit.dist > NEAR_MAX {
                        ybuf = ybuf.min(self.far_wall(g, proj, hit, &c, h, x as i32, ybuf));
                        continue;
                    }
                    // Its OWN falloff — the street's, over 150 units, not the
                    // room's over twenty — and then held back a further notch,
                    // so what is beyond the glass sits behind the surfaces of
                    // the room whatever the two distances happen to be.
                    ybuf = ybuf.min(self.facade(
                        g,
                        world.grain,
                        proj,
                        hit,
                        &c,
                        h,
                        x as i32,
                        ybuf,
                        edge_v,
                        col_vignette,
                        OUTSIDE_DIM,
                    ));
                    continue;
                };

                // --- a surface of the room --------------------------------
                let solid = c.win == fit::WALL || c.win == fit::COLUMN || c.win == fit::LIFT;
                // A wall, a column and the lift core meet the ceiling exactly.
                // Their cell height is that rounded up, and using it would
                // leave a seam. A window's sill is read off the ROOM, not off
                // the cell: a cell height is a whole unit and a car's slab is
                // not, so quantising it would make the glass jump a unit at a
                // time as the car rose.
                let top = if solid {
                    cy
                } else if c.win == fit::WINDOW {
                    room.base + room.sill
                } else {
                    c.height as f32
                };
                if top <= room.base + 0.01 && c.win != fit::SHAFT {
                    continue;
                }
                // The walls of the shaft run the whole height of the building,
                // and you are looking DOWN one of them as well as up it — so
                // they are the one surface indoors that is not floored by the
                // slab you are standing on.
                let foot = if c.win == fit::SHAFT { 0.0 } else { room.base };
                // The top face of anything you are looking down on: a counter,
                // a window sill, a crate. Same band the rooftops use.
                if proj.eye > top {
                    ybuf = ybuf.min(self.fitting_top(g, room, proj, hit, &c, top, x as i32, ybuf));
                }
                let r0 = proj.row_of(top, hit.dist).ceil().max(0.0) as i32;
                let r1raw = proj.row_of(foot, hit.dist);
                if r1raw < 0.0 {
                    continue;
                }
                let r1 = (r1raw.floor() as i32).min(rows - 1);
                if r0 > r1 {
                    continue;
                }
                let span = r1 - r0;
                let r1 = r1.min(ybuf - 1);
                if r0 > r1 {
                    continue;
                }
                ybuf = ybuf.min(r0);

                let hue = c.hue as f32;
                let sat = c.sat as f32;
                let glow = c.lit as f32 / 100.0;
                // Indoors both of these are gentler than they are on a street.
                // A room is lit from its own ceiling, so its two wall faces are
                // nearly as bright as each other, and a vignette that reads as
                // depth down an avenue reads as a smudge on a wall you could
                // touch.
                let base_b = room_light(room.ambient, hit.dist, inv_fall)
                    * (if hit.side == 0 { 1.0 } else { 0.93 })
                    * (0.90 + 0.10 * (col_vignette - 0.8) * 5.0);
                let along = hit.along;
                for y in r0..=r1 {
                    let b = base_b * (0.84 + 0.16 * v_profile(y - r0, span));
                    let wy = proj.height_at(y as usize, hit.dist) - room.base;
                    let scale = (hit.dist / proj.proj_x)
                        .max(hit.dist * proj.row_span[(y as usize).min(proj.rows - 1)]);
                    // **World units to ONE character, here** — across the
                    // surface and up it. Every band and joint indoors is
                    // measured in these, so a joint is a joint at any distance.
                    // Counting them in fractions of a CELL instead is what
                    // turned a panel line into a sixty-column stripe: a sixth
                    // of a cell is one column on a facade across the street and
                    // half the screen on a wall you could lean on.
                    let cw = (hit.dist / proj.proj_x).max(1e-4);
                    let ch = (hit.dist * proj.row_span[(y as usize).min(proj.rows - 1)])
                        .max(1e-4);
                    let sn = surf_tex(along, wy, quant_fine(scale), 3 * hit.cell_x, 5 * hit.cell_z);
                    let (glyph, colour) =
                        surface_of(room, &c, y, r0, r1, along, cw, ch, wy, sn, b, hue, sat, glow);
                    if glyph != b' ' {
                        g.put(x as i32, y, glyph, colour, hit.dist);
                    }
                }
            }
        }
    }

    /// The top of something in the room you are taller than — a counter, a
    /// sill, a crate. Between the cell's far edge and its near edge, exactly
    /// the way `roof` handles a rooftop seen from above, because it is the
    /// same band: the difference is that indoors you are over this furniture
    /// every time you walk past it rather than only from a vista.
    #[allow(clippy::too_many_arguments)]
    fn fitting_top(
        &mut self,
        g: &mut Grid,
        room: &Interior,
        proj: &Projection,
        hit: &crate::raycast::Hit,
        c: &crate::world::Cell,
        top: f32,
        x: i32,
        ybuf: i32,
    ) -> i32 {
        let t0 = proj.row_of(top, hit.exit).ceil().max(0.0) as i32;
        let t1 = (proj.row_of(top, hit.dist).floor().min(proj.rows as f32 - 1.0) as i32)
            .min(ybuf - 1);
        if t0 > t1 {
            return i32::MAX;
        }
        let b = room_light(room.ambient, hit.dist, 1.0 / room.fall);
        let hue = c.hue as f32;
        for y in t0..=t1 {
            let k = if t1 > t0 { (t1 - y) as f32 / (t1 - t0) as f32 } else { 0.0 };
            let d = hit.dist + (hit.exit - hit.dist) * k;
            let n = noise(hit.cell_x * 17 + y, hit.cell_z * 31);
            let (glyph, colour) = match c.win {
                fit::WINDOW => (b'=', hsl(196.0, 26.0, 30.0 + 40.0 * b)),
                fit::COUNTER | fit::DESK => {
                    if y == t1 {
                        (b'=', hsl(hue, 40.0, 34.0 + 38.0 * b))
                    } else if n > 0.8 {
                        (b'o', hsl(hue, 70.0, 40.0 + 32.0 * b))
                    } else {
                        (b'-', hsl(hue, 26.0, 22.0 + 30.0 * b))
                    }
                }
                fit::PLANTER => (
                    if n > 0.55 { b'*' } else { b'&' },
                    hsl(112.0, 52.0, 18.0 + 34.0 * b),
                ),
                _ => {
                    if y == t1 {
                        (b'_', hsl(hue, 30.0, 26.0 + 30.0 * b))
                    } else if n > 0.62 {
                        (b':', hsl(hue, 22.0, 16.0 + 24.0 * b))
                    } else {
                        (b'.', hsl(hue, 18.0, 12.0 + 20.0 * b))
                    }
                }
            };
            g.put(x, y, glyph, colour, d);
        }
        t0
    }

    /// The fixtures: the exit sign over the door, terminals, notices, standing
    /// lamps, planting, air handlers. Billboards through `nearest`, the same
    /// per-column wall buffer the street furniture and the population go
    /// through, so a lamp behind a rack is behind it.
    ///
    /// They are not decoration the renderer invented — `Interior::props` placed
    /// them and each one carries a label, a verb and a reach that
    /// `Interior::interaction_near` answers with.
    fn room_fixtures(&mut self, g: &mut Grid, room: &Interior, cam: &Camera, proj: &Projection) {
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let cols = proj.cols as i32;
        let rows = proj.rows as i32;
        for p in &room.props {
            let tx = p.x - cam.x;
            let tz = p.z - cam.z;
            let fd = tx * fx + tz * fz;
            if fd < 0.25 || fd > ROOM_MAX {
                continue;
            }
            let off = tx * rx + tz * rz;
            let x0 = (proj.cols as f32 / 2.0 + (proj.proj_x / fd) * off).round() as i32;
            if x0 < -12 || x0 >= cols + 12 {
                continue;
            }
            let b = room_light(room.ambient, fd, 1.0 / room.fall).max(0.15);
            let top = proj.row_of(room.base + p.top, fd).ceil() as i32;
            let bot = proj.row_of(room.base + p.bottom, fd).floor() as i32;
            if bot < 0 || top >= rows || bot < top {
                continue;
            }
            let cols_per_unit = proj.proj_x / fd;
            // Capped harder than the street's furniture is. Indoors you walk
            // right up to these, and a half-metre air handler two paces off
            // genuinely does subtend half the screen — which is true, and reads
            // as a wall rather than as a fitting. Past the cap it is drawn at
            // the cap rather than dropped, so nothing pops.
            let wide = |r: f32| ((r * cols_per_unit).round() as i32).clamp(0, cols / 9);
            match p.kind {
                Fitting::ExitSign | Fitting::LiftSign => {
                    // The two things in a room allowed to shout, and they earn
                    // it: from the back of a room the way out — and the way up
                    // — is a lit word.
                    let half = wide(0.85).max(2);
                    let word = p.kind.word();
                    let bg = hsl(p.hue, 88.0, 22.0 + 14.0 * b);
                    let ink = hsl(p.hue, 100.0, 62.0 + 24.0 * b);
                    let mid = (top + bot) / 2;
                    let n = 2 * half + 1;
                    for y in top..=bot {
                        for i in 0..n {
                            let cx = x0 - half + i;
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let edge = y == top || y == bot || i == 0 || i == n - 1;
                            if edge {
                                g.put(cx, y, if y == top || y == bot { b'=' } else { b'|' }, ink, fd);
                            } else if y == mid && n >= 6 {
                                // Space the four letters across the panel.
                                let k = ((i - 1) * 4) / (n - 2);
                                let want = ((k * (n - 2)) / 4) + 1;
                                let ch = if i == want { word[k.min(3) as usize] } else { b' ' };
                                if ch != b' ' {
                                    g.put(cx, y, ch, ink, fd);
                                } else {
                                    g.put(cx, y, b' ', bg, fd);
                                }
                            } else {
                                g.put(cx, y, b'.', bg, fd);
                            }
                        }
                    }
                }
                Fitting::Terminal => {
                    let half = wide(0.34).max(0);
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let n = noise(5 * cx + p.seed as i32, 3 * y);
                            let ch = if y == top { b'=' } else if n > 0.6 { b'#' } else { b':' };
                            g.put(cx, y, ch, hsl(p.hue, 74.0, 30.0 + 44.0 * b), fd);
                        }
                    }
                }
                Fitting::Notice => {
                    let half = wide(0.6).max(1);
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let frame = y == top || y == bot || cx == x0 - half || cx == x0 + half;
                            let n = noise(7 * cx + 13 * p.seed as i32, 11 * y);
                            let ch = if frame { b'+' } else if n > 0.55 { b'-' } else { b' ' };
                            if ch != b' ' {
                                g.put(cx, y, ch, hsl(p.hue, 52.0, 26.0 + 40.0 * b), fd);
                            }
                        }
                    }
                }
                Fitting::Lamp => {
                    let half = wide(0.16).max(0);
                    let headr = proj.row_of(room.base + p.top - 0.35, fd).round() as i32;
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            if y <= headr {
                                g.put(cx, y, b'@', hsl(p.hue, 84.0, 56.0 + 34.0 * b), fd);
                            } else {
                                g.put(cx, y, b'|', hsl(p.hue, 20.0, 16.0 + 20.0 * b), fd);
                            }
                        }
                    }
                }
                Fitting::Plant => {
                    let half = wide(0.5).max(1);
                    let cyr = (top + bot) / 2;
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let u = (cx - x0) as f32 / half.max(1) as f32;
                            let v = (y - cyr) as f32 / ((bot - top).max(1) as f32 * 0.5);
                            if u * u + v * v > 1.0 {
                                continue;
                            }
                            let n = noise(3 * cx + 29 * p.seed as i32, 5 * y + 7);
                            if n < 0.2 {
                                continue;
                            }
                            let ch = if n > 0.72 { b'*' } else if n > 0.45 { b'&' } else { b'%' };
                            g.put(cx, y, ch, hsl(p.hue, 48.0, 14.0 + 34.0 * b), fd);
                        }
                    }
                }
                Fitting::CallUp | Fitting::CallDown => {
                    // A call button, at hand height on the wall beside you: a
                    // lit arrow on a small plate. Which of the two is under
                    // your hand is what the act key does, and the HUD says
                    // which before you press. Kept SMALL on purpose — the first
                    // cut used the terminal's radius and a lift panel two paces
                    // off then filled a fifth of the screen with a dark slab.
                    let half = wide(0.11).max(1);
                    let up = p.kind == Fitting::CallUp;
                    let mid = (top + bot) / 2;
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let edge = y == top || y == bot || cx == x0 - half || cx == x0 + half;
                            let arrow = (y - mid).abs() <= (bot - top) / 6 && (cx - x0).abs() <= half / 3;
                            let (ch, l, sat) = if arrow {
                                (if up { b'^' } else { b'v' }, 62.0, 100.0)
                            } else if edge {
                                (b'+', 40.0, 34.0)
                            } else {
                                (b':', 26.0, 22.0)
                            };
                            g.put(cx, y, ch, hsl(p.hue, sat, l * (0.55 + 0.45 * b)), fd);
                        }
                    }
                }
                Fitting::Vent => {
                    let half = wide(0.42).max(1);
                    for y in top..=bot {
                        for cx in x0 - half..=x0 + half {
                            if self.hidden(cx, fd) {
                                continue;
                            }
                            let ch = if (cx - x0).rem_euclid(2) == 0 { b'#' } else { b'=' };
                            g.put(cx, y, ch, hsl(p.hue, 16.0, 12.0 + 22.0 * b), fd);
                        }
                    }
                }
            }
        }
    }


    /// The rooftop plane, seen only from above. Between the cell's far edge and
    /// its near edge; dark, with plant and the odd roof light, and a bright
    /// parapet on the near lip so the block still reads as a solid volume.
    /// Returns the topmost row it covered, for the caller's running horizon.
    #[allow(clippy::too_many_arguments)]
    fn roof(
        &mut self,
        g: &mut Grid,
        proj: &Projection,
        hit: &crate::raycast::Hit,
        c: &crate::world::Cell,
        h: f32,
        x: i32,
        ybuf: i32,
    ) -> i32 {
        let top = proj.row_of(h, hit.exit).ceil().max(0.0) as i32;
        let bot = proj
            .row_of(h, hit.dist)
            .floor()
            .min(proj.rows as f32 - 1.0) as i32;
        let bot = bot.min(ybuf - 1);
        if top > bot {
            return i32::MAX;
        }
        let hue = c.hue as f32;
        let b = (1.0 - (hit.dist / FALLOFF).min(1.0)).max(0.0);
        for y in top..=bot {
            // Depth across the band, so the far edge of a roof sits behind the
            // near edge of the roof beyond it.
            let t = if bot > top { (bot - y) as f32 / (bot - top) as f32 } else { 0.0 };
            let d = hit.dist + (hit.exit - hit.dist) * t;
            let n = noise(hit.cell_x * 13 + y, hit.cell_z * 29);
            let (glyph, colour) = if y == bot {
                (b'=', hsl(hue, 70.0, 30.0 + 30.0 * b)) // parapet
            } else if n > 0.93 {
                (b'#', hsl(hue, 90.0, 48.0 + 22.0 * b)) // roof plant / light
            } else if n > 0.62 {
                (b':', hsl(hue, 30.0, 12.0 + 16.0 * b))
            } else if n > 0.3 {
                (b'.', hsl(hue, 20.0, 8.0 + 12.0 * b))
            } else {
                (b' ', [0, 0, 0])
            };
            if glyph != b' ' {
                g.put(x, y, glyph, colour, d);
            }
        }
        top
    }

    /// The far skyline: its own falloff, its own fixed-pitch lattice, and about
    /// a third of every facade left blank so distant towers read as lit grids
    /// on black rather than as solid slabs.
    #[allow(clippy::too_many_arguments)]
    fn far_wall(
        &mut self,
        g: &mut Grid,
        proj: &Projection,
        hit: &crate::raycast::Hit,
        c: &crate::world::Cell,
        h: f32,
        x: i32,
        ybuf: i32,
    ) -> i32 {
        let fb = ((FAR_ZERO - hit.dist) / FAR_SPAN).clamp(0.0, 1.0);
        if fb <= 0.0 {
            return i32::MAX;
        }
        let r0 = proj.row_of(h, hit.dist).ceil().max(0.0) as i32;
        let r1 = proj
            .row_of(0.0, hit.dist)
            .floor()
            .min(proj.rows as f32 - 1.0) as i32;
        let r1 = r1.min(ybuf - 1);
        if r0 > r1 {
            return i32::MAX;
        }
        let hue = c.hue as f32;
        let lit_p = c.lit as f32 / 100.0;
        let dim = 0.07 + 0.27 * fb;
        let style = match c.arch { 1 => 3u8, 2 => 0, 3 => 2, _ => c.win };
        for y in r0..=r1 {
            let wy = proj.height_at(y as usize, hit.dist);
            let uq = (0.55 * hit.along).floor() as i32;
            let vq = (0.55 * wy).floor() as i32;
            let v4 = vq.rem_euclid(4);
            let u6 = uq.rem_euclid(6);
            let n = noise(13 * hit.cell_x + 29 * uq, 11 * hit.cell_z + 17 * vq);
            let pane = match style {
                0 => u6 % 3 == 1 && v4 == 1,
                1 => u6 % 3 == 1 && v4 == 1 && n < 0.48,
                2 => (v4 == 1 || v4 == 2) && u6 % 2 == 0,
                _ => u6 % 2 == 0 && n < 0.68,
            };
            if y == r0 && r0 > 0 {
                let a = c.arch;
                let gl = match a { 1 => b'~', 2 => b'^', 3 => b'*', _ => b'=' };
                g.put(x, y, gl, hsl(hue, 55.0, 15.0 + 28.0 * fb), hit.dist);
            } else if pane && n < 0.72 * lit_p {
                g.put(x, y, if hit.dist > 300.0 { b'.' } else { b'0' },
                      hsl(hue, 70.0, 19.0 + 31.0 * fb), hit.dist);
            } else if pane {
                g.put(x, y, b':', hsl(hue, 28.0, 9.0 + 18.0 * fb), hit.dist);
            } else if n > 0.34 {
                g.put(x, y, if n > 0.72 { b':' } else { b'.' },
                      hsl(hue, (0.42 * c.sat as f32).max(18.0), 7.0 + 24.0 * dim), hit.dist);
            }
        }
        r0
    }

    // ---- population -----------------------------------------------------
    /// Figures, not filled blocks: a head, a torso with shoulders and legs for
    /// a person; a banded body on wheels for a vehicle. Filling the sprite's
    /// bounding box turns anything that walks close to the camera into a solid
    /// slab of colour.
    fn population(&mut self, g: &mut Grid, cam: &Camera, proj: &Projection, pop: &Population) {
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let cols = proj.cols as i32;
        let rows = proj.rows as i32;
        let sprite_max_half = (proj.cols as i32 / 12).max(2);

        let place = |px: f32, pz: f32, max_d: f32, nearest: &[f32]| -> Option<(f32, i32, f32)> {
            let tx = px - cam.x;
            let tz = pz - cam.z;
            let fd = tx * fx + tz * fz;
            if fd < 0.35 || fd > max_d {
                return None;
            }
            let off = tx * rx + tz * rz;
            let x0 = (proj.cols as f32 / 2.0 + (proj.proj_x / fd) * off).round() as i32;
            if x0 < 0 || x0 >= cols || nearest[x0 as usize] + 0.08 < fd {
                return None;
            }
            Some((fd, x0, (1.0 - fd / FALLOFF).max(0.0)))
        };

        for p in &pop.peds {
            let Some((fd, x0, b)) = place(p.x, p.z, 60.0, &self.nearest) else { continue };
            let top = proj.row_of(1.8, fd).ceil() as i32;
            let bot = proj.row_of(0.0, fd).floor() as i32;
            let span = bot - top;
            let colour = hsl(p.hue as f32, 80.0, 32.0 + 22.0 * b);
            if span < 2 {
                g.put(x0, bot, b'@', colour, fd);
                continue;
            }
            let half = ((0.28 / fd * proj.proj_x).round() as i32).clamp(0, sprite_max_half);
            let (c0, c1) = (x0 - half, x0 + half);
            let (i0, i1) = (x0 - (half >> 1), x0 + (half >> 1));
            let head = top + (0.3 * span as f32).floor().max(0.0) as i32;
            let torso = top + (0.68 * span as f32).floor().max(1.0) as i32;
            let flip = p.hx >= 0.0;
            for y in top..=bot {
                if y < 0 || y >= rows { continue; }
                for x in c0..=c1 {
                    let ch = if y <= head {
                        if x >= i0 && x <= i1 { b'@' } else { b' ' }
                    } else if y <= torso {
                        if x == c0 { if flip { b'/' } else { b'\\' } }
                        else if x == c1 { if flip { b'\\' } else { b'/' } }
                        else if x >= i0 && x <= i1 { b'#' } else { b' ' }
                    } else if x == i0 { if flip { b'/' } else { b'\\' } }
                    else if x == i1 { if flip { b'\\' } else { b'/' } }
                    else { b' ' };
                    if ch != b' ' {
                        g.put(x, y, ch, colour, fd);
                    }
                }
            }
        }

        for v in &pop.vehs {
            let Some((fd, x0, b)) = place(v.x, v.z, 90.0, &self.nearest) else { continue };
            let top = proj.row_of(1.05, fd).ceil() as i32;
            let bot = proj.row_of(0.0, fd).floor() as i32;
            let span = bot - top;
            let colour = hsl(v.hue as f32, 82.0, 42.0 + 25.0 * b);
            let glass = hsl(205.0, 45.0, 24.0 + 18.0 * b);
            if span < 1 {
                g.put(x0, bot, b'=', colour, fd);
                continue;
            }
            let half = ((0.9 / fd * proj.proj_x).round() as i32).clamp(1, sprite_max_half);
            let (c0, c1) = (x0 - half, x0 + half);
            let w = c1 - c0 + 1;
            let roof = top + (0.28 * span as f32).floor().max(0.0) as i32;
            let belt = top + (0.55 * span as f32).floor().max(1.0) as i32;
            for y in top..=bot {
                if y < 0 || y >= rows { continue; }
                for x in c0..=c1 {
                    let side = x == c0 || x == c1;
                    let wheel = y == bot && (x <= c0 + (w >> 3).max(0) || x >= c1 - (w >> 3).max(0));
                    let ch = if wheel { b'o' }
                        else if y <= roof { b'-' }
                        else if y <= belt { if side { b'o' } else { b':' } }
                        else { b'=' };
                    let col = if y > roof && y <= belt && !side { glass } else { colour };
                    g.put(x, y, ch, col, fd);
                }
            }
            if pop.plates_on && span >= 2 {
                // Which end of the car we are looking at used to decide the
                // colour — yellow at the back, white at the front. A plate is
                // drawn out of characters now and both ends are the same
                // yellow, so there is nothing left to ask. See `PLATE_BODY`.
                self.plate_q.push(PlateDraw {
                    dist: fd,
                    row: bot,
                    span,
                    c0,
                    c1,
                    key: v.plate,
                });
            }
        }

        // Second pass, now that every car body is in the depth buffer: a plate
        // can be tested against the whole frame rather than against the part of
        // it that happened to be drawn first.
        let readable = pop.plates.readable_width() as i32;
        for q in self.plate_q.drain(..) {
            let p = pop.plates.get(q.key);
            plate_on(g, p, q.dist, q.row, q.span, q.c0, q.c1, readable);
        }
    }
    // ---- street furniture -----------------------------------------------
    /// Lampposts, street trees and planters.
    ///
    /// They go through the same two gates everything else in the frame goes
    /// through — the distance falloff that lights them, and `nearest`, the
    /// per-column wall distance, that hides them behind anything closer — so a
    /// lamp down the far end of a street is dim and a lamp round the corner is
    /// simply not there. They are placed by the world generator
    /// (`World::props_near`), not scattered here.
    fn props(&mut self, g: &mut Grid, world: &World, cam: &Camera, proj: &Projection) {
        // Beyond this a lamppost is thinner than a character and reads as
        // speckle on the pavement, which is worse than nothing.
        const RANGE: f32 = 58.0;
        world.props_near(cam.x, cam.z, RANGE, &mut self.props);
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let cols = proj.cols as i32;
        let rows = proj.rows as i32;

        for p in &self.props {
            let tx = p.x - cam.x;
            let tz = p.z - cam.z;
            let fd = tx * fx + tz * fz;
            if fd < 0.3 || fd > RANGE {
                continue;
            }
            let off = tx * rx + tz * rz;
            let x0 = (proj.cols as f32 / 2.0 + (proj.proj_x / fd) * off).round() as i32;
            if x0 < -8 || x0 >= cols + 8 {
                continue;
            }
            let b = (1.0 - fd / FALLOFF).max(0.0);
            let bot = proj.row_of(0.0, fd).floor() as i32;
            let top = proj.row_of(p.height, fd).ceil() as i32;
            if bot < 0 || top >= rows || bot < top {
                continue;
            }
            // World units to screen columns at this distance.
            let cols_per_unit = proj.proj_x / fd;
            let wide = |r: f32| ((r * cols_per_unit).round() as i32).clamp(0, cols / 6);

            match p.kind {
                Prop::Lamp => {
                    // A post, a bracket, a head, and the pool it throws on the
                    // pavement. The pool matters more than the post: a lamp
                    // that does not light anything is a pole, and a street of
                    // poles is not a lit street.
                    let head = proj.row_of(p.height - 0.45, fd).round() as i32;
                    let post = hsl(212.0, 14.0, 18.0 + 18.0 * b);
                    let lamp = hsl(46.0, 100.0, 64.0 + 26.0 * b);
                    let halo = hsl(40.0, 88.0, 34.0 + 26.0 * b);
                    let shaft = wide(0.07);
                    for y in top..=bot {
                        if y < 0 || y >= rows { continue; }
                        for x in x0 - shaft..=x0 + shaft {
                            if self.hidden(x, fd) { continue; }
                            g.put(x, y, b'|', post, fd);
                        }
                    }
                    let arm = wide(0.30).max(1);
                    for x in x0 - arm..=x0 + arm {
                        if self.hidden(x, fd) { continue; }
                        let ch = if x == x0 { b'T' } else { b'-' };
                        g.put(x, head.max(top), ch, halo, fd);
                        if head + 1 <= bot {
                            g.put(x, head + 1, if x == x0 { b'@' } else { b'o' }, lamp, fd);
                        }
                    }
                    // The pool. Elliptical on screen because the pavement is
                    // seen at a glancing angle, and drawn just in FRONT of the
                    // lamp's own distance so it lies on the ground rather than
                    // fighting it for the same cells.
                    let pool_w = wide(1.5).max(1);
                    let pool_h = ((bot - head) / 6).clamp(1, 3);
                    for dy in 0..=pool_h {
                        let y = bot - dy;
                        if y < 0 || y >= rows { continue; }
                        let v = dy as f32 / (pool_h + 1) as f32;
                        let w = (pool_w as f32 * (1.0 - v * v).max(0.0).sqrt()) as i32;
                        for x in x0 - w..=x0 + w {
                            if self.hidden(x, fd) { continue; }
                            let u = (x - x0) as f32 / pool_w.max(1) as f32;
                            let fall = (1.0 - (u * u + v * v)).max(0.0);
                            if fall < 0.12 {
                                continue;
                            }
                            g.put(
                                x,
                                y,
                                if fall > 0.6 { b':' } else { b'.' },
                                hsl(44.0, 70.0, 22.0 + 34.0 * fall * (0.35 + 0.65 * b)),
                                fd - 0.02,
                            );
                        }
                    }
                }
                Prop::Tree => {
                    // Trunk to about a third of the way up, then a canopy that
                    // is an ellipse of leaf glyphs rather than a filled blob —
                    // a filled blob at close range is a wall of green.
                    let trunk_h = p.height * 0.34;
                    let trunk_row = proj.row_of(trunk_h, fd).round() as i32;
                    let bark = hsl(26.0, 38.0, 18.0 + 20.0 * b);
                    let shaft = wide(0.09);
                    for y in trunk_row.max(top)..=bot {
                        if y < 0 || y >= rows { continue; }
                        for x in x0 - shaft..=x0 + shaft {
                            if self.hidden(x, fd) { continue; }
                            g.put(x, y, b'|', bark, fd);
                        }
                    }
                    let rad = 0.55 + 0.22 * ((p.seed >> 20) % 4) as f32;
                    let half_w = wide(rad).max(1);
                    let cy = 0.5 * (top + trunk_row) as f32;
                    let half_h = (0.5 * (trunk_row - top) as f32).max(0.8);
                    // **Foliage green, not olive.** The old canopy sat at hue
                    // 104-137 with one flat saturation, which is a yellow-green
                    // — and a yellow-green at middling saturation next to a
                    // facade running full-chroma neon reads as grey. This is a
                    // cooler, deeper green, and the depth comes from the
                    // SATURATION rather than from the hue: the leaves the light
                    // catches are vivid and lean a shade warm, the ones in
                    // shadow drop to almost no chroma at all. That is what
                    // planting does under a street lamp, and it is what stops a
                    // tree competing with the building behind it.
                    //
                    // It holds under the falloff for the same reason every
                    // other colour here does: `hsl` is a floor plus a range on
                    // LIGHTNESS only, so a tree at seventy units is the same
                    // green as one at seven, darker.
                    let hue = 132.0 + ((p.seed >> 24) % 15) as f32;
                    for y in top..=trunk_row {
                        if y < 0 || y >= rows { continue; }
                        let v = (y as f32 - cy) / half_h;
                        for x in x0 - half_w..=x0 + half_w {
                            let u = (x - x0) as f32 / half_w as f32;
                            if u * u + v * v > 1.0 {
                                continue;
                            }
                            if self.hidden(x, fd) { continue; }
                            let n = noise(3 * x + 71 * p.seed as i32, 5 * y + 37);
                            if n < 0.18 {
                                continue; // let some sky through the crown
                            }
                            let (ch, colour) = if n > 0.72 {
                                (b'*', hsl(hue - 9.0, 66.0, 26.0 + 30.0 * b + 6.0 * n))
                            } else if n > 0.45 {
                                (b'&', hsl(hue, 42.0, 15.0 + 22.0 * b + 5.0 * n))
                            } else {
                                (b'%', hsl(hue + 7.0, 24.0, 9.0 + 15.0 * b + 4.0 * n))
                            };
                            g.put(x, y, ch, colour, fd);
                        }
                    }
                }
                Prop::Planter => {
                    let half_w = wide(0.42).max(1);
                    let rim = hsl(34.0, 22.0, 30.0 + 24.0 * b);
                    let leaf = hsl(118.0, 52.0, 18.0 + 26.0 * b);
                    for y in top..=bot {
                        if y < 0 || y >= rows { continue; }
                        for x in x0 - half_w..=x0 + half_w {
                            if self.hidden(x, fd) { continue; }
                            let edge = x == x0 - half_w || x == x0 + half_w;
                            if y == top {
                                let n = noise(7 * x + 13 * p.seed as i32, 3);
                                g.put(x, y, if n > 0.55 { b'*' } else { b'"' }, leaf, fd);
                            } else {
                                g.put(x, y, if edge { b'|' } else { b'=' }, rim, fd);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Is this column already filled by something nearer? The same test the
    /// population goes through, so props sit in the street rather than on top
    /// of the picture.
    #[inline]
    fn hidden(&self, x: i32, fd: f32) -> bool {
        x < 0
            || x as usize >= self.nearest.len()
            || self.nearest[x as usize] + 0.08 < fd
    }

    // ---- weather --------------------------------------------------------
    /// Rain, drawn where it belongs: **in the world**, at each drop's own
    /// distance, through the same per-column wall buffer the population goes
    /// through. That is the whole difference between rain and speckle laid
    /// over the picture — a drop behind a facade is hidden by it, a drop in
    /// front of one is drawn over it, and both get the same distance falloff
    /// as everything else, so the far side of the street rains faintly and the
    /// near side rains hard.
    ///
    /// It reads as *falling* because it is: the drop's world height is what
    /// picks its screen row, so it descends frame to frame, and the near ones
    /// carry a second cell of streak below them.
    fn rain(&mut self, g: &mut Grid, cam: &Camera, proj: &Projection, sky: &Sky) {
        if sky.drops.is_empty() {
            return;
        }
        let (fx, fz) = cam.forward();
        let (rx, rz) = cam.right();
        let cols = proj.cols as i32;
        let rows = proj.rows as i32;
        // Which way the fall leans on screen — and it leans because YOU are
        // moving, not because of a constant. Rain seen from a standstill falls
        // straight down; rain seen from a run slants back past you, and the
        // faster you go the harder it slants. The camera carries a real
        // velocity now, so this can be the honest quantity rather than a
        // stand-in for one.
        let drift = -(cam.vx * rx + cam.vz * rz);
        for d in &sky.drops {
            let tx = d.x - cam.x;
            let tz = d.z - cam.z;
            let fd = tx * fx + tz * fz;
            if fd < 0.35 || fd > 60.0 {
                continue;
            }
            let off = tx * rx + tz * rz;
            let x = (proj.cols as f32 / 2.0 + (proj.proj_x / fd) * off).round() as i32;
            if x < 0 || x >= cols || self.nearest[x as usize] + 0.08 < fd {
                continue;
            }
            let y = proj.row_of(d.y, fd).round() as i32;
            if y < 0 || y >= rows {
                continue;
            }
            let near = fd < 10.0;
            let mid = fd < 22.0;
            // Fall against drift, both as apparent motion at this distance.
            let ch = if drift.abs() > 0.45 * d.speed {
                if drift > 0.0 { b'\\' } else { b'/' }
            } else if near || mid {
                b'|'
            } else {
                b'\''
            };
            let colour = if near {
                hsl(188.0, 58.0, 52.0)
            } else if mid {
                hsl(188.0, 46.0, 38.0)
            } else {
                hsl(188.0, 34.0, 25.0)
            };
            // A drop that lands on a registration plate knocks a character out
            // of it, and a plate with a character missing reads as some other
            // registration. The plate wins; one drop is not worth that.
            if g.is_plate(x, y) {
                continue;
            }
            g.put(x, y, ch, colour, fd);
            // A drop crosses several rows in the time one frame lasts, so it is
            // a streak, not a dot — and the nearer and faster it is falling,
            // the longer that streak. This is what makes it read as FALLING in
            // a still frame as well as a moving one.
            let tail = if near { 2 } else if mid { 1 } else { 0 };
            for k in 1..=tail {
                if y + k >= rows {
                    break;
                }
                if g.is_plate(x, y + k) {
                    break;
                }
                g.put(x, y + k, ch, hsl(188.0, 44.0, 38.0 - 8.0 * k as f32), fd);
            }
        }
    }
}

/// One cell of the OUTDOOR ground: which glyph and which colour, given the
/// cell, where on it the ray landed, and how much of the world one character
/// row covers there.
///
/// Lifted out of `Renderer::ground` unchanged so that `Renderer::room_floor`
/// can call it too: standing in a room you see the pavement out of the window,
/// and the pavement out of the window has to be the same pavement. A second,
/// indoor-only rendering of a street would drift from the real one the first
/// time either was touched.
///
/// `inline` is load-bearing rather than decorative: this runs for every ground
/// cell of every frame — thousands — and out of line it costs a call and a
/// returned tuple where the old inline body kept everything in registers.
/// Measured at +0.027 ms a frame on a 0.57 ms frame without it.
#[inline(always)]
fn ground_glyph(c: &Cell, wx: f32, wz: f32, cx: i32, cz: i32, d: f32, scale: f32) -> (u8, [u8; 3]) {
    let b = (1.0 - d / FALLOFF).max(0.0);
    let fine = scale < 1.15;
    let sn = surf_tex(wx, wz, scale, 3 * c.surface as i32, 5 * c.surface as i32);
    // Which axis this street runs along, and the coordinate along it.
    let along_x = cz.rem_euclid(BLOCK) >= BLOCK_BUILT;
    let lane = if along_x { wx } else { wz };
    let par = if along_x { b'=' } else { b'|' }; // parallel to the kerb
    let m = c.cross as i32;

    let mut glyph = b' ';
    let mut colour = [0u8; 3];
    match c.surface {
        surface::ROADWAY => {
            glyph = if b > 0.5 { b'-' } else if b > 0.28 { b'_' } else { b'.' };
            // Asphalt reads as neutral grey under sodium and neon,
            // not as blue: keep the hue, drop the saturation.
            colour = hsl(210.0, 18.0, 30.0 + 28.0 * b);
            if fine && sn > 0.94 {
                glyph = if b > 0.48 { b':' } else { b'.' };
                colour = hsl(135.0, 70.0, 42.0 + 28.0 * b);
            }
            if m == 4 || m == 11 {
                glyph = par;
                colour = hsl(195.0, 34.0, 58.0 + 22.0 * b);
            } else if fine && d > 14.0 && noise(3 * cx + 1, 3 * cz) < 0.04 {
                // A drain cover — but only once it is far enough
                // out to be smaller than a character. One world
                // cell at your feet spans dozens of screen cells,
                // so a per-cell feature down there paints a blob.
                glyph = b'o';
                colour = hsl(200.0, 16.0, 34.0 + 16.0 * b);
            } else if (m == 7 || m == 8)
                && (!fine || ((2.0 * lane) as i32).rem_euclid(3) < 2)
            {
                glyph = if fine { par } else { b':' };
                colour = hsl(205.0, 42.0, 55.0 + 17.0 * b);
            } else if fine && (m == 6 || m == 9)
                && ((2.0 * lane) as i32).rem_euclid(5) < 3
            {
                glyph = b':';
                colour = hsl(205.0, 22.0, 38.0 + 16.0 * b);
            }
        }
        surface::PAVEMENT => {
            if m == 3 || m == 12 {
                glyph = par;
                colour = hsl(38.0, 30.0, 66.0 + 22.0 * b);
            } else {
                let slab = if along_x { cz % 4 == 0 } else { cx % 4 == 0 };
                glyph = if slab {
                    if along_x { b'-' } else { b'|' }
                } else if sn < 0.48 { b',' } else if sn < 0.82 { b'.' } else { b':' };
                colour = if slab {
                    hsl(38.0, 20.0, 52.0 + 24.0 * b)
                } else {
                    hsl(38.0, 24.0, 46.0 + 30.0 * b)
                };
            }
        }
        surface::PAINTED => {
            let on = if fine {
                ((2.0 * lane) as i32).rem_euclid(2) == 0
            } else {
                sn < 0.55
            };
            if on {
                glyph = if fine { b'=' } else { b'-' };
                colour = hsl(45.0, 30.0, 72.0 + 14.0 * b);
            }
        }
        surface::PLAZA => {
            let ridge = sn > 0.84;
            let soft = fine && sn < 0.1;
            glyph = if ridge {
                if cx & 1 != 0 { b'~' } else { b'=' }
            } else if soft { b'.' } else { b'_' };
            colour = hsl(
                if ridge { 190.0 } else { 205.0 },
                if ridge { 72.0 } else { 54.0 },
                22.0 + 31.0 * b + if soft { 8.0 } else { 0.0 },
            );
        }
        surface::SERVICE => {
            let seam = cx % 3 == 0 || cz % 3 == 0;
            glyph = if seam {
                if cx % 3 == 0 { b'|' } else { b'-' }
            } else if sn > 0.78 { b':' } else { b'.' };
            colour = if seam {
                hsl(42.0, 22.0, 48.0 + 26.0 * b)
            } else {
                hsl(38.0, 18.0, 38.0 + 26.0 * b)
            };
        }
        surface::THRESHOLD => {
            // The floor of an entrance bay. Lit, because the doorway behind it
            // is: it is the one piece of ground in the city that belongs to a
            // building rather than to the street, and it should look it.
            let step = if along_x { cz } else { cx };
            glyph = if step.rem_euclid(2) == 0 { b'=' } else { b'-' };
            colour = hsl(44.0, 46.0, 40.0 + 34.0 * b);
        }
        _ => {
            glyph = if sn < 0.5 { b',' } else if sn < 0.8 { b'.' } else { b'*' };
            colour = hsl(112.0, 42.0, 13.0 + 17.0 * b);
        }
    }
    (glyph, colour)
}

/// How far back the city sits when it is seen from inside a room.
///
/// It keeps its OWN falloff — the street's, over 150 units — because that is
/// what makes a tower across the road read as being across the road. This is
/// the notch on top: a room is lit and the outside is not lit by the room, so
/// what is past the glass has to sit behind the surfaces of the room even where
/// the two happen to be the same distance off.
const OUTSIDE_DIM: f32 = 0.62;

/// Scale all three channels at once. Hue and saturation are untouched, so the
/// city out of the window is the same city a shade further away, not a browner
/// one — the same reason `PLATE_FLOOR` scales rather than blends.
#[inline]
fn dim_rgb(c: [u8; 3], k: f32) -> [u8; 3] {
    [(c[0] as f32 * k) as u8, (c[1] as f32 * k) as u8, (c[2] as f32 * k) as u8]
}

/// How bright a surface of a room is at distance `d`.
///
/// **NOT the street's falloff with a smaller number in it.** Outdoors,
/// brightness runs from one at your feet to nothing at 150 units, because the
/// far end of a street genuinely is dark. A room is *lit*: its far wall is
/// twenty units away and you can see it perfectly well. So the fall is over the
/// top HALF of the range and the bottom half is the room's own ambient — which
/// is what makes a bar dim everywhere and a market bright everywhere, rather
/// than making every room a tunnel with a torch in it.
#[inline]
fn room_light(ambient: f32, d: f32, inv_fall: f32) -> f32 {
    (ambient * (0.56 + 0.44 * (1.0 - d * inv_fall).max(0.0))).clamp(0.0, 1.0)
}

/// The same fixed-world-spacing line, offset along the run — the far end of a
/// crate to `line_at`'s near end.
#[inline]
fn line_at_shift(along: f32, cw: f32, w: f32, shift: f32) -> bool {
    let a = along + w * shift;
    w > cw * 1.7 && (a / w).floor() != ((a - cw) / w).floor()
}

/// A muted brightness ramp for surfaces you are standing next to. `WALL_RAMP`
/// runs from `@` to blank because a facade across the street has a hundred and
/// fifty units to fade over; a room has twenty, and the same ramp inside it
/// makes every wall look like a lit facade.
const ROOM_RAMP: &[u8; 8] = b"#8Zx*:. ";

/// The ceiling's own hue, fixed and neutral rather than `room.wall_hue`. A
/// plenum is a slab or acoustic tile up there, not a fifth coat of the wall
/// paint, and giving it a family's wall colour was the other half of why a
/// room read as one hue floor to ceiling — the LIT strips still carry the
/// room's own `light_hue`, which is where a ceiling's colour should come from.
const CEIL_HUE: f32 = 205.0;

/// One cell of a room's FLOOR: which glyph and which colour for the material
/// this room is floored in. `b` already carries the pool the ceiling strips
/// throw, which is why a floor lights up in bands under the lights.
fn floor_glyph(
    room: &Interior,
    c: &crate::world::Cell,
    wx: f32,
    wz: f32,
    cx: i32,
    cz: i32,
    scale: f32,
    b: f32,
) -> (u8, [u8; 3]) {
    let b = b.clamp(0.0, 1.15);
    let sn = surf_tex(wx, wz, quant(scale).max(0.25), 3 * c.surface as i32, 7 * c.surface as i32);
    // The floor's OWN hue, not the wall's — see `Interior::floor_hue`. Using
    // `room.wall_hue` here used to make a room's floor and its walls the same
    // colour at nearly the same lightness, which is why a room read as one
    // dark haze instead of as a floor, a wall and a ceiling.
    let hue = room.floor_hue;
    match c.surface {
        floor::TILE => {
            let jx = cx.rem_euclid(2) == 0;
            let jz = cz.rem_euclid(2) == 0;
            if jx && jz {
                (b'+', hsl(hue, 28.0, 28.0 + 66.0 * b))
            } else if jx {
                (b'|', hsl(hue, 22.0, 22.0 + 60.0 * b))
            } else if jz {
                (b'-', hsl(hue, 22.0, 22.0 + 60.0 * b))
            } else {
                (if sn > 0.6 { b':' } else { b'.' }, hsl(hue, 16.0, 13.0 + 58.0 * b))
            }
        }
        floor::BOARD => {
            let run = if room.light_along_x { cz } else { cx };
            if run.rem_euclid(2) == 0 {
                (
                    if room.light_along_x { b'-' } else { b'|' },
                    hsl(hue, 38.0, 16.0 + 58.0 * b),
                )
            } else {
                (if sn > 0.55 { b'=' } else { b'_' }, hsl(hue + 2.0, 34.0, 12.0 + 56.0 * b))
            }
        }
        floor::POURED => {
            let seam = cx.rem_euclid(4) == 0 || cz.rem_euclid(4) == 0;
            if seam {
                (b'_', hsl(hue, 14.0, 21.0 + 56.0 * b))
            } else {
                (if sn > 0.72 { b':' } else { b'.' }, hsl(hue, 12.0, 12.0 + 54.0 * b))
            }
        }
        floor::CARPET => (
            if sn < 0.34 { b',' } else if sn < 0.72 { b'.' } else { b':' },
            hsl(hue, 32.0, 10.0 + 56.0 * b),
        ),
        floor::GRATE => (
            if (cx + cz).rem_euclid(2) == 0 { b'#' } else { b'=' },
            hsl(hue, 18.0, 12.0 + 60.0 * b),
        ),
        floor::TERRAZZO => {
            if sn > 0.92 {
                (b'*', hsl(hue, 36.0, 36.0 + 64.0 * b))
            } else if sn > 0.5 {
                (b':', hsl(hue, 18.0, 23.0 + 64.0 * b))
            } else {
                (b'.', hsl(hue, 12.0, 18.0 + 60.0 * b))
            }
        }
        _ => {
            // The mat in the doorway, lit from the street side. Kept warm and
            // fixed regardless of the room's own floor hue — it belongs to the
            // THRESHOLD, the same as the lit doorway outside does.
            let step = cx.rem_euclid(2) == 0;
            (if step { b'=' } else { b'-' }, hsl(44.0, 44.0, 32.0 + 46.0 * b))
        }
    }
}

/// One cell of a room's own SURFACES: a wall, a window sill, a column, or a
/// piece of furniture.
///
/// The bands are what make it read as a room rather than as a small street:
/// a **lit cove** where the wall meets the ceiling, a **dado** at hand height,
/// a **skirting** at the floor, and panel joints between them. All of them are
/// at fixed WORLD heights and world spacings, so they hold level and hold still
/// as you cross the room.
///
/// `q` is world units to one character at this distance, and every lattice here
/// is counted in it. That is the whole difference between a joint and a stripe:
/// a fixed fraction of a cell is one column on a facade a hundred units off and
/// thirty columns on a wall you could lean on.
#[allow(clippy::too_many_arguments)]
fn surface_of(
    room: &Interior,
    c: &crate::world::Cell,
    y: i32,
    r0: i32,
    r1: i32,
    along: f32,
    // World units one character covers across the surface, and up it.
    cw: f32,
    ch: f32,
    wy: f32,
    sn: f32,
    b: f32,
    hue: f32,
    sat: f32,
    glow: f32,
) -> (u8, [u8; 3]) {
    let dt = y - r0; // rows down from the top of this thing
    let dr = r1 - y; // rows up from the floor
    let tall = r1 - r0 > 4;
    // **A line at a fixed WORLD spacing, exactly one character wide.** A
    // character covers `cw` of the surface across and `ch` up it, so the line is
    // wherever those two disagree about which interval they are in — which is
    // true for one character and no more, however near or far the surface is.
    // Once the spacing is finer than a character there is no line to draw, only
    // a smear, so it stops.
    let line_at = |w: f32| {
        w > cw * 1.7 && (along / w).floor() != ((along - cw) / w).floor()
    };
    let band_at = |w: f32| w > ch * 1.7 && (wy / w).floor() != ((wy - ch) / w).floor();
    match c.win {
        fit::WALL => {
            if dt == 0 {
                // The cove. A room is lit from the top of its walls, and this
                // one line is most of why the light reads as coming from up
                // there rather than from the camera.
                (b'=', hsl(room.light_hue, room.light_sat, 38.0 + 56.0 * b))
            } else if dt == 1 && tall {
                (b'_', hsl(room.light_hue, room.light_sat * 0.5, 16.0 + 50.0 * b))
            } else if dr == 0 {
                (b'_', hsl(hue, sat * 0.6, 4.0 + 40.0 * b)) // skirting
            } else if tall && (0.88..1.06).contains(&wy) {
                (b'-', hsl(hue, sat * 0.9, 16.0 + 58.0 * b)) // dado
            } else if line_at(1.1) {
                (b'|', hsl(hue, sat, 14.0 + 62.0 * b)) // panel joint
            } else if sn < 0.2 * glow {
                (b':', hsl(room.light_hue, room.light_sat, 30.0 + 56.0 * b))
            } else {
                // Compressed toward the light end: a room's own walls are not
                // a hundred and fifty units of falloff, they are a lit surface
                // a few paces off, and the straight ramp made every one of them
                // read as a wall at the far end of a street.
                // The dither carries more weight indoors than it does on a
                // facade: a wall at a fixed distance is at a fixed brightness,
                // and a ramp keyed on brightness alone paints it in one glyph
                // from end to end.
                let idx = ((ROOM_RAMP.len() - 1) as f32 * (1.0 - b).powf(1.7)
                    + 1.3 * (sn - 0.5))
                    .round()
                    .clamp(0.0, (ROOM_RAMP.len() - 1) as f32) as usize;
                (ROOM_RAMP[idx], hsl(hue, sat, 8.0 + 78.0 * b))
            }
        }
        fit::WINDOW => {
            // The sill, and the spandrel under it. Everything above this is the
            // city, and it got there through the same DDA.
            if dt == 0 {
                (b'=', hsl(196.0, 30.0, 46.0 + 44.0 * b))
            } else if line_at(1.1) {
                (b'|', hsl(196.0, 26.0, 32.0 + 40.0 * b))
            } else if dr == 0 {
                (b'_', hsl(hue, sat * 0.6, 12.0 + 26.0 * b))
            } else {
                (b':', hsl(hue, sat * 0.7, 16.0 + 32.0 * b))
            }
        }
        // **The wall at the back of the lift shaft.** The only surface in this
        // engine textured from the world model's own account of the building
        // rather than from the cell it is on — see `shaft_glyph`.
        fit::SHAFT => shaft_glyph(room, c, along, cw, wy + room.base, sn, b),
        // The lift core, from the room. A machine standing in the lobby: a
        // metal box with two doors in its flank and a lit indicator over them.
        fit::LIFT => {
            if dt == 0 {
                (b'=', hsl(room.light_hue, room.light_sat, 40.0 + 50.0 * b))
            } else if dr == 0 {
                (b'_', hsl(hue, sat * 0.6, 6.0 + 40.0 * b))
            } else if (2.28..2.52).contains(&wy) {
                // The head of the doors, run right round the core.
                (b'=', hsl(44.0, 62.0, 34.0 + 44.0 * b))
            } else if line_at(0.75) {
                (b'|', hsl(hue, sat + 8.0, 22.0 + 54.0 * b))
            } else if sn > 0.72 {
                (b'8', hsl(hue, sat, 20.0 + 48.0 * b))
            } else {
                (b'#', hsl(hue, sat * 0.7, 14.0 + 44.0 * b))
            }
        }
        fit::COLUMN => {
            if dt == 0 {
                (b'=', hsl(hue, sat, 44.0 + 42.0 * b))
            } else if dt == 1 {
                (b'-', hsl(hue, sat, 30.0 + 38.0 * b))
            } else if line_at(0.55) {
                (b'|', hsl(hue, sat, 32.0 + 44.0 * b))
            } else {
                (if sn > 0.5 { b'#' } else { b'8' }, hsl(hue, sat * 0.8, 20.0 + 36.0 * b))
            }
        }
        fit::COUNTER | fit::DESK => {
            if dt == 0 {
                (b'=', hsl(room.light_hue, 62.0, 46.0 + 40.0 * b))
            } else if line_at(0.9) {
                (b'|', hsl(hue, sat, 24.0 + 38.0 * b))
            } else if dr == 0 {
                (b'_', hsl(hue, sat * 0.6, 12.0 + 24.0 * b))
            } else {
                (b':', hsl(hue, sat, 20.0 + 38.0 * b))
            }
        }
        fit::RACK => {
            // Shelves, at a shelf's spacing, one character deep.
            let shelf = band_at(0.62);
            if dt == 0 {
                (b'=', hsl(hue, sat, 36.0 + 40.0 * b))
            } else if line_at(0.8) {
                (b'|', hsl(hue, sat, 28.0 + 38.0 * b))
            } else if shelf {
                (b'=', hsl(hue, sat * 0.8, 24.0 + 36.0 * b))
            } else if sn > 0.58 {
                (b'#', hsl((hue + 24.0) % 360.0, sat, 22.0 + 38.0 * b))
            } else {
                (b'.', hsl(hue, sat * 0.5, 12.0 + 26.0 * b))
            }
        }
        fit::MACHINE => {
            if dt == 0 {
                (b'=', hsl(hue, sat, 36.0 + 40.0 * b))
            } else if sn > 0.87 {
                (b'o', hsl(room.light_hue, 92.0, 50.0 + 36.0 * b)) // a dial, lit
            } else if line_at(1.0) {
                (b'|', hsl(hue, sat, 26.0 + 34.0 * b))
            } else {
                (b'#', hsl(hue, sat * 0.8, 18.0 + 34.0 * b))
            }
        }
        fit::CRATE => {
            // A box: an upright at each end of every world unit of it.
            if dt == 0 {
                (b'=', hsl(hue, sat, 34.0 + 38.0 * b))
            } else if line_at(1.0) {
                (b'[', hsl(hue, sat, 28.0 + 36.0 * b))
            } else if line_at_shift(along, cw, 1.0, 0.5) {
                (b']', hsl(hue, sat, 28.0 + 36.0 * b))
            } else if sn > 0.6 {
                (b'-', hsl(hue, sat * 0.8, 22.0 + 34.0 * b))
            } else {
                (b'.', hsl(hue, sat * 0.6, 14.0 + 28.0 * b))
            }
        }
        fit::PARTITION => {
            if dt == 0 {
                (b'_', hsl(hue, sat, 32.0 + 38.0 * b))
            } else if line_at(1.0) {
                (b'|', hsl(hue, sat, 26.0 + 34.0 * b))
            } else {
                (if sn > 0.5 { b':' } else { b'.' }, hsl(hue, sat * 0.7, 16.0 + 30.0 * b))
            }
        }
        fit::RAIL => {
            if dt == 0 {
                (b'=', hsl(hue, sat, 46.0 + 40.0 * b))
            } else if line_at(1.4) {
                (b'|', hsl(hue, sat * 0.8, 28.0 + 36.0 * b))
            } else {
                (b':', hsl(hue, sat * 0.5, 14.0 + 26.0 * b))
            }
        }
        fit::PLANTER => {
            if dt == 0 {
                (if sn > 0.5 { b'*' } else { b'&' }, hsl(112.0, 52.0, 26.0 + 40.0 * b))
            } else {
                (b'=', hsl(28.0, 26.0, 18.0 + 32.0 * b))
            }
        }
        fit::TANK => {
            if dt == 0 {
                (b'=', hsl(hue, sat, 38.0 + 38.0 * b))
            } else if line_at(1.0) {
                (b'(', hsl(hue, sat, 30.0 + 34.0 * b))
            } else if line_at_shift(along, cw, 1.0, 0.5) {
                (b')', hsl(hue, sat, 30.0 + 34.0 * b))
            } else if band_at(0.8) {
                (b'=', hsl(hue, sat * 0.8, 24.0 + 34.0 * b))
            } else {
                (b'O', hsl(hue, sat, 20.0 + 34.0 * b))
            }
        }
        _ => (b':', hsl(hue, sat, 18.0 + 32.0 * b)),
    }
}

/// **The wall at the back of the lift shaft — the floors going past.**
///
/// This is the one surface in the engine whose texture comes out of the world
/// model rather than out of the cell it is drawn on. `Interior::storeys` is the
/// building's own table of floors — the same table every room in the building
/// is built from and the only heights the car is allowed to stop at — and every
/// band here is one fact off it, at its real world height:
///
///   * the **slab** under a floor, `lift::SLAB` deep, with its nosing lit,
///     because the thing you actually watch go past is a floor plate;
///   * above it, on the wall square to the car, **that storey**: lit in its own
///     colours, brightest under its own ceiling, so floor 3 and floor 4 do not
///     look alike and the number on the wall is not the only thing telling them
///     apart;
///   * on the side walls, no storeys at all — a shaft wall and its guide rails.
///     The dark sides against the lit back is most of what frames the picture.
///
/// It is keyed on ABSOLUTE world height, never on height above the car, which
/// is what makes the floors hold still in the world and slide down past the
/// glass as the car rises. Keying it on the car would paper the shaft with a
/// pattern that travelled with you, which is the one thing this must not be.
#[allow(clippy::too_many_arguments)]
fn shaft_glyph(
    room: &Interior,
    c: &crate::world::Cell,
    along: f32,
    cw: f32,
    ay: f32,
    sn: f32,
    b: f32,
) -> (u8, [u8; 3]) {
    let rib = |w: f32| w > cw * 1.7 && (along / w).floor() != ((along - cw) / w).floor();
    let Some((s, rel)) = room.storey_at(ay) else {
        // Below the lowest slab: the pit, and raw structure.
        return (if rib(1.2) { b'|' } else { b'.' }, hsl(206.0, 8.0, 3.0 + 7.0 * sn));
    };
    if rel < 0.0 {
        // **The floor plate.** The heaviest band in the shaft and the one the
        // eye tracks, so it gets the strongest contrast in here: a lit nosing
        // on top of a slab of dark concrete.
        return if rel > -0.16 {
            (b'=', hsl(40.0, 34.0, 52.0 + 24.0 * b))
        } else if rel > -0.34 {
            (b'#', hsl(30.0, 12.0, 24.0 + 14.0 * b))
        } else if rel < -crate::lift::SLAB + 0.16 {
            (b'=', hsl(206.0, 8.0, 15.0 + 12.0 * b))
        } else {
            (if sn > 0.5 { b'H' } else { b'#' }, hsl(206.0, 7.0, 10.0 + 13.0 * b))
        };
    }
    if rel >= s.ceiling {
        // The plenum between one storey's ceiling and the next one's slab.
        return (b'-', hsl(206.0, 6.0, 4.0 + 7.0 * b));
    }
    if c.arch == 0 {
        // A side wall of the shaft: concrete, and the car's guide rails on it.
        return if rib(1.3) {
            (b'|', hsl(206.0, 12.0, 20.0 + 20.0 * b))
        } else if sn > 0.66 {
            (b':', hsl(206.0, 8.0, 10.0 + 13.0 * b))
        } else {
            (b'.', hsl(206.0, 6.0, 6.0 + 11.0 * b))
        };
    }

    // --- the storey itself, square to the car ----------------------------
    // Light comes from its ceiling, the way it does in the room you step out
    // into, because it is the same room and the same `ambient`.
    let up = rel / s.ceiling;
    let lit = (s.ambient * (0.42 + 0.58 * up)).clamp(0.0, 1.0) * b;

    // The storey's number, stencilled on the pier at one side of it. Fixed
    // brightness: it is lit signage in a shaft, and it is the one thing in here
    // that has to read at whatever distance the shaft happens to put it.
    if (0.62..2.08).contains(&rel) {
        let org = if room.ix != 0 { room.z0 } else { room.x0 } as f32;
        // Centred across the wall, whatever the shaft is wide.
        let u = along - org - (crate::lift::CORE_W as f32 * 0.5 - 1.85);
        if (1.02..2.68).contains(&u) {
            // A framed plate, the way a billboard on a facade is framed: a
            // stencil straight on to a lit wall is a stencil you cannot read.
            if !(0.75..1.95).contains(&rel) || !(1.15..2.55).contains(&u) {
                return (b'+', hsl(44.0, 30.0, 16.0 + 14.0 * b));
            }
            let gx = (((u - 1.15) / 1.4) * 7.0) as i32;
            let gy = ((((1.95 - rel) / 1.2) * 5.0) as i32).clamp(0, 4) as usize;
            let n = s.floor.clamp(0, 99);
            // A seven-wide field either way, so a floor number is the same size
            // whether it is one digit or two.
            let (glyph, col) = if n >= 10 {
                if gx < 3 {
                    (b'0' + (n / 10) as u8, gx)
                } else if gx == 3 {
                    (b' ', 0)
                } else {
                    (b'0' + (n % 10) as u8, gx - 4)
                }
            } else if (2..5).contains(&gx) {
                (b'0' + n as u8, gx - 2)
            } else {
                (b' ', 0)
            };
            if glyph != b' '
                && crate::palette::glyph3x5(glyph)[gy].as_bytes()[col.clamp(0, 2) as usize] == b'#'
            {
                return (b'#', hsl(44.0, 96.0, 66.0));
            }
            return (b'.', hsl(44.0, 22.0, 7.0 + 8.0 * b));
        }
    }

    // A lit floor across the well. Strong, simple horizontals — the cove it is
    // lit from, a dado, the plate you would walk out on to — with the glazing's
    // mullions across them. A noise field at this range reads as speckle and
    // not as a room, which is the same lesson `ROOM_RAMP` learned indoors.
    if rel < 0.26 {
        (b'_', hsl(s.light_hue, s.light_sat * 0.7, 30.0 + 30.0 * lit))
    } else if rel > s.ceiling - 0.26 {
        // Its ceiling cove — where the light in there comes from.
        (b'=', hsl(s.light_hue, s.light_sat + 26.0, 54.0 + 38.0 * lit))
    } else if rel > s.ceiling - 0.58 {
        (b'-', hsl(s.light_hue, s.light_sat + 10.0, 38.0 + 34.0 * lit))
    } else if (0.92..1.16).contains(&rel) {
        (b'-', hsl(s.wall_hue, s.wall_sat + 18.0, 32.0 + 34.0 * lit))
    } else if rib(1.0) {
        // The mullions of the storey's own glazing, one to a world unit.
        (b'|', hsl(s.wall_hue, s.wall_sat + 24.0, 34.0 + 36.0 * lit))
    } else if sn > 0.5 {
        (b':', hsl(s.wall_hue, s.wall_sat + 22.0, 24.0 + 42.0 * lit))
    } else {
        (b'+', hsl(s.wall_hue, s.wall_sat + 14.0, 19.0 + 38.0 * lit))
    }
}

struct Sign {
    top: i32,
    bottom: i32,
    u: f32,
    grid: &'static [String; 5],
    hue: f32,
}

/// How far a plate panel is drawn at all, and over what distance it fades out.
/// A vehicle is drawn out to 90 units; its plate gives up well before that, so
/// far traffic is simply traffic.
const PLATE_FADE: f32 = 75.0;
const PLATE_FADE_SPAN: f32 = 34.0;
/// How far a DRAWN plate is ever allowed to dim. Everything else in the frame
/// falls off toward black with distance; a plate must not, because plate
/// yellow is recognised by its saturation long before a character is read off
/// it, and a panel scaled to a fifth of its brightness is a brown smear.
/// Scaling all three channels by one factor leaves hue and saturation exactly
/// where they were and takes only brightness down, so this is a floor on
/// brightness alone: the plate is the same yellow at 70 units as at 8.
const PLATE_FLOOR: f32 = 0.74;
/// Cells of panel width per row of panel height that read as plate-shaped.
///
/// A UK plate is 520x111 mm — about 4.7 times as wide as it is tall. A cell is
/// not square: the SVG writer's is 11x18 and a terminal's is near enough 1:2,
/// so a row of height is worth about 4.7 * 18/11 = 7.7 cells of width on the
/// first and 4.7 * 2 = 9.4 on the second. Eight sits between them and is close
/// enough on both.
const PLATE_ASPECT: i32 = 8;
/// How far below its own proportion a panel may be squeezed and still be drawn
/// at that height, as a fraction — three quarters of `rows * PLATE_ASPECT`.
///
/// The panel is now sized to what it carries, so a short private registration
/// asks for a narrow panel. Two rows of a narrow panel is not a plate, it is a
/// square; below this line the candidate walk steps down to the one-row strip,
/// which at the same width is exactly the right shape for it.
const PLATE_SHAPE_NUM: i32 = 3;
const PLATE_SHAPE_DEN: i32 = 4;
/// How tall a car has to be before its plate is drawn three rows deep — rules
/// closing it top and bottom with the registration between them. That is what a
/// plate actually looks like, and it wants a car big enough to carry it.
const PLATE_THREE_ROW_SPAN: i32 = 9;
/// How tall a car has to be before its plate is drawn two rows deep.
///
/// Two rows is what makes the panel read as a rectangle bolted to the back of
/// a car rather than as a highlighted word, so it is worth reaching for — but
/// a plate is about a quarter of the height of the back of a car, and on a car
/// five rows tall two rows is nearly half of it. Six is where the proportion
/// stops lying.
const PLATE_TWO_ROW_SPAN: i32 = 6;

/// The registration plate on one vehicle.
///
/// Three bands, and the middle one is the whole design. A plate is drawn as
/// REAL CHARACTERS only while every character of it fits in the clear span
/// between the wheels; below that it is a blank panel of the right size and
/// colour — a plate-shaped smudge; below that it is nothing. It never
/// abbreviates and never drops a character, because a plate that reads as a
/// different registration to the one the car is carrying would be worse than
/// no plate at all.
///
/// It is a panel rather than coloured text because that is what a plate is:
/// black on yellow at the back, black on white at the front. `Grid::put_panel`
/// exists for this and nothing else.
///
/// It is laid out as a plate rather than as a highlighted word:
///
/// ```text
///   ooo==|            |==ooo   <- a dark edge cell at each end, and clear
///   ooo==| XY24 ZZT   |==ooo      field between it and the characters; the
///        ^^          ^^           second row only on a car tall enough
///        edge        edge
/// ```
///
/// A dark edge cell at each end and clear field inside it — that is
/// `PLATE_PAD` — so the bodywork abuts the BORDER rather than the characters,
/// and the yellow terminates somewhere definite instead of running into a line
/// of `=`. Without them the registration read as text that happened to be
/// highlighted, which is exactly what it was.
fn plate_on(
    g: &mut Grid,
    plate: &Plate,
    fd: f32,
    row: i32,
    span: i32,
    c0: i32,
    c1: i32,
    readable: i32,
) {
    let k = ((PLATE_FADE - fd) / PLATE_FADE_SPAN).clamp(0.0, 1.0);
    if k < 0.12 {
        return;
    }
    let w = c1 - c0 + 1;
    // The wheels take the two ends of the bottom row; the plate sits in what
    // is left between them.
    let inset = (w >> 3) + 1;
    let (lo, hi) = (c0 + inset, c1 - inset);
    let free = hi - lo + 1;
    if free < 3 {
        return;
    }
    let lit = PLATE_FLOOR + (1.0 - PLATE_FLOOR) * k;
    let dim = |c: [u8; 3]| {
        [(c[0] as f32 * lit) as u8, (c[1] as f32 * lit) as u8, (c[2] as f32 * lit) as u8]
    };
    // The colour of the CHARACTERS the plate is built out of, and the colour of
    // the registration standing in the space they leave.
    let body = dim(PLATE_BODY);
    let ink = dim(PLATE_INK);
    let settings = plate.settings();
    let need = settings[0] as i32 + PLATE_PAD as i32;
    let centre = (lo + hi) / 2;

    if free >= need {
        // Three shots at drawing the registration, deepest first. A plate at
        // its proper proportion needs more clear span than a shallower one
        // does, so when a lamppost or a passing car takes part of it the plate
        // steps down a row rather than giving up on the registration — and only
        // when even one row is not clear does it fall back to the empty plate
        // below. Every candidate is still ALL OR NOTHING: the whole block is
        // tested before one cell of it is written.
        let max_rows = if span >= PLATE_THREE_ROW_SPAN && row > 1 {
            3
        } else if span >= PLATE_TWO_ROW_SPAN && row > 0 {
            2
        } else {
            1
        };
        let mut row_buf = [b' '; PLATE_SET_MAX];
        for rows in (1..=max_rows).rev() {
            // What the proportion asks for at this height, and then the SETTING
            // that lands nearest it: a plate is only ever as wide as the
            // registration on it is set, so the characters fill it.
            let ideal = rows * PLATE_ASPECT;
            let mut pick: Option<(usize, i32)> = None;
            for (s, &e) in settings.iter().enumerate() {
                let pw = e as i32 + PLATE_PAD as i32;
                if pw > free {
                    continue;
                }
                // A tall plate squeezed well under its own width is a square,
                // not a plate — a short private registration cannot fill one,
                // and must take a shallower plate instead.
                if rows > 1 && pw * PLATE_SHAPE_DEN < ideal * PLATE_SHAPE_NUM {
                    continue;
                }
                let better = match pick {
                    None => true,
                    Some((_, w)) => (pw - ideal).abs() < (w - ideal).abs(),
                };
                if better {
                    pick = Some((s, pw));
                }
            }
            let Some((set, pw)) = pick else { continue };
            let x = centre - pw / 2;
            let top = row - rows + 1;
            if !(top..=row).all(|y| g.run_is_clear(x, y, pw, fd)) {
                continue;
            }
            let n = plate.set_into(set, &mut row_buf) as i32;
            debug_assert_eq!(n, pw - PLATE_PAD as i32);
            // On a plate two rows deep or more the registration takes the
            // BOTTOM row and the rules run above it, so the characters sit
            // where a plate sits on the back of a car; on three the rules close
            // it top and bottom and the registration is in the middle.
            let text_row = if rows >= 3 { row - 1 } else { row };
            for y in top..=row {
                for i in 0..pw {
                    let cx = x + i;
                    let end = i == 0 || i == pw - 1;
                    let (ch, colour) = if y != text_row {
                        // A rule: the plate's body, all the way across, turning
                        // a corner at each end so the rectangle closes.
                        (if end { PLATE_CORNER } else { PLATE_RULE }, body)
                    } else if end {
                        // The uprights at each end of the registration's row.
                        // On a one-row plate a bare `|` does not close anything,
                        // so it takes a bracket instead.
                        let cap = if rows == 1 {
                            if i == 0 { PLATE_CAP_L } else { PLATE_CAP_R }
                        } else {
                            PLATE_UPRIGHT
                        };
                        (cap, body)
                    } else {
                        (row_buf[(i - 1) as usize], ink)
                    };
                    g.put_plate(cx, y, ch, colour, fd);
                }
            }
            return;
        }
    }

    // Too small to read, or not wholly in view: a plate the right shape and the
    // right colour, and not one character of the registration. It stays
    // narrower than a registration needs, so "wide enough to be carrying one"
    // and "carrying one" never come apart — an empty plate is an honest smudge
    // and never a shorter registration.
    let bw = (w / 3).clamp(3, free.min(readable - 1));
    let x = centre - bw / 2;
    for i in 0..bw {
        let ch = if i == 0 {
            PLATE_CAP_L
        } else if i == bw - 1 {
            PLATE_CAP_R
        } else {
            PLATE_RULE
        };
        g.put_plate(x + i, row, ch, body, fd);
    }
}

/// Does this wall face carry a lit sign panel, and if so where on screen?
/// Only tall buildings get one, on one face of the block, roughly a third of
/// the way up.
fn billboard_on(
    proj: &Projection,
    hit: &crate::raycast::Hit,
    h: f32,
    hue: f32,
    wall_pos: f32,
) -> Option<Sign> {
    if h < 18.0 {
        return None;
    }
    let bx = hit.cell_x.div_euclid(BLOCK);
    let bz = hit.cell_z.div_euclid(BLOCK);
    // Which of the four block faces this wall belongs to.
    let axis = if hit.side == 0 { hit.cell_x } else { hit.cell_z };
    let face = 2 * hit.side as i32 + if axis.rem_euclid(BLOCK) > 11 { 1 } else { 0 };
    let seed = noise(97 * bx + 1301 * face + 7000, 89 * bz + 1877 * face + 9000);
    if seed <= 0.9 {
        return None;
    }
    let narrow = noise(313 * bx + 1999 * face, 439 * bz + 2657 * face) > 0.72;
    let w = if narrow { 4.8 } else { 10.5 };
    let hh = if narrow { 4.2 } else { 2.45 };
    let u = wall_pos.rem_euclid(BLOCK as f32);
    let centre = 8.0 + 3.0 * (noise(577 * bx + 37 * face, 811 * bz + 53 * face) - 0.5) - 0.5 * w;
    let su = (u - centre) / w;
    if !(0.0..1.0).contains(&su) {
        return None;
    }
    let base = 3.8
        + noise(997 * bx + 83 * face, 1291 * bz + 101 * face) * (h - hh - 5.3).min(4.5).max(0.0);
    let top = proj.row_of(base + hh, hit.dist).ceil() as i32;
    let bottom = proj.row_of(base, hit.dist).floor() as i32;
    if bottom - top < 3 {
        return None;
    }
    let k = ((41.0 * seed) as usize) % SIGN_WORDS.len();
    Some(Sign {
        top,
        bottom,
        u: su,
        grid: sign_rows(k, narrow),
        hue: (hue + 75.0 + (140.0 * seed).floor()) % 360.0,
    })
}
