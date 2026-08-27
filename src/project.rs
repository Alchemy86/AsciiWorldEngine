//! The projection — the screen-row maths that decides how tall a wall appears.
//!
//! A raycaster hands out distances; turning a distance into a wall of a
//! certain height on a certain screen row is this file's whole job, and it
//! lives inside the engine rather than in a frontend so that every frontend
//! gets the same picture.
//!
//! Two things are load-bearing and easy to get wrong:
//!
//!   * Pitch is a **true camera rotation**, not a horizon offset. A horizon
//!     offset shears the picture; a rotation swings it.
//!   * The inverse (world height seen at a screen row) carries the **opposite**
//!     pitch sign, and ground depth takes the minus branch.
//!
//! `row_span` is the per-row world-units-per-row derivative. It feeds the
//! texture quantiser; substituting the flat horizon value `1/proj_y` keeps the
//! quantiser one step too fine on distant walls and aliases the storeys away.

/// Vertical half-FOV, radians (~40 degrees vertical). Fixed — horizontal FOV
/// follows from the pixel aspect, so a wide terminal widens the view rather
/// than stretching it.
pub const V_HALF_FOV: f32 = 0.35;

pub struct Projection {
    pub cols: usize,
    pub rows: usize,
    pub proj_x: f32,
    pub proj_y: f32,
    pub h_fov: f32,
    pub horizon: f32,
    pub pitch: f32,
    pub eye: f32,
    /// `tan` of the elevation of each screen row. Inverse of `row_of`.
    pub view_tan: Vec<f32>,
    /// World units of surface one character row covers, per unit of distance.
    pub row_span: Vec<f32>,
    /// Distance to the ground plane seen at each screen row (inf above the
    /// horizon).
    pub ground_depth: Vec<f32>,
    /// `tan` of the bearing of each screen column off the view axis.
    pub col_tan: Vec<f32>,
}

impl Projection {
    /// `cell_w` / `cell_h` are the display cell's pixel size: that ratio is the
    /// only thing that decides horizontal FOV, which is why a terminal (cells
    /// roughly 1:2) and a browser canvas both come out looking right.
    pub fn new(cols: usize, rows: usize, cell_w: f32, cell_h: f32) -> Self {
        let proj_y = rows as f32 / 2.0 / V_HALF_FOV.tan();
        let h_fov = 2.0
            * (V_HALF_FOV.tan() * (cols as f32 * cell_w) / (cell_h * rows as f32)).atan();
        let proj_x = (cols as f32 / 2.0) / (h_fov / 2.0).tan();
        let mut p = Projection {
            cols,
            rows,
            proj_x,
            proj_y,
            h_fov,
            horizon: rows as f32 / 2.0,
            pitch: 0.0,
            eye: 1.25,
            view_tan: vec![0.0; rows],
            row_span: vec![0.0; rows],
            ground_depth: vec![f32::INFINITY; rows],
            col_tan: (0..cols)
                .map(|x| (x as f32 - (cols as f32 - 1.0) / 2.0) / proj_x)
                .collect(),
        };
        p.set_view(0.0, 1.25);
        p
    }

    /// Recompute the per-row tables for a new pitch / eye height. Cheap: rows
    /// is tens, not thousands.
    pub fn set_view(&mut self, pitch: f32, eye: f32) {
        self.pitch = pitch;
        self.eye = eye;
        for y in 0..self.rows {
            self.view_tan[y] = (((self.horizon - y as f32) / self.proj_y).atan() + pitch).tan();
            let a = ((y as f32 - self.horizon) / self.proj_y).atan() - pitch;
            self.ground_depth[y] = if a < 1e-4 { f32::INFINITY } else { eye / a.tan() };
        }
        for y in 0..self.rows {
            let lo = self.view_tan[y.saturating_sub(1)];
            let hi = self.view_tan[(y + 1).min(self.rows - 1)];
            self.row_span[y] = 0.5 * (lo - hi).abs();
        }
    }

    /// Screen row for a point at world height `y`, perpendicular distance `perp`.
    #[inline]
    pub fn row_of(&self, y: f32, perp: f32) -> f32 {
        let a = (y - self.eye).atan2(perp.max(1e-4)) - self.pitch;
        if a >= core::f32::consts::FRAC_PI_2 - 1e-5 {
            return -1e6;
        }
        if a <= -core::f32::consts::FRAC_PI_2 + 1e-5 {
            return 1e6;
        }
        self.horizon - self.proj_y * a.tan()
    }

    /// Distance to a HORIZONTAL PLANE at world height `h`, seen at screen row
    /// `y`, or infinity where that row does not meet it at all.
    ///
    /// `ground_depth` is this for `h == 0`, precomputed because the ground is
    /// always at zero. A ceiling is not: it is per room, and a floor slab is
    /// per storey, so those two ask for it a row at a time.
    #[inline]
    pub fn plane_depth(&self, y: usize, h: f32) -> f32 {
        let t = self.view_tan[y.min(self.rows - 1)];
        let dh = h - self.eye;
        // A plane above the eye is only ever met looking up, and one below it
        // only looking down: opposite signs mean the row misses it entirely.
        if dh * t <= 1e-6 {
            f32::INFINITY
        } else {
            dh / t
        }
    }

    /// World height seen at screen row `y`, at perpendicular distance `perp`.
    #[inline]
    pub fn height_at(&self, y: usize, perp: f32) -> f32 {
        self.eye + self.view_tan[y.min(self.rows - 1)] * perp
    }

    /// `tan` of the elevation of the top of the screen — the ratio a column has
    /// to beat to be visible at all above an existing wall. The raycaster's
    /// early-out uses it.
    #[inline]
    pub fn top_ratio(&self) -> f32 {
        self.view_tan[0]
    }
}
