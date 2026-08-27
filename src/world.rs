//! The city.
//!
//! There is no grid in memory. Every cell is a pure function of its global
//! integer coordinate, so the city is unbounded in every direction and costs
//! nothing to hold — you can walk for a week and never leave it. The other
//! trade, a fixed window of cells slid along as you walk, buys nothing here
//! and costs tens of megabytes resident.
//!
//! The layout is a **32-cell block** whose first 16 cells on each axis are the
//! built quadrant and whose remaining 16 are the avenue. Everything the renderer needs about a cell — height, ground
//! surface, hue, saturation, lit fraction, window lattice, architecture and
//! the plan id that gives a building its facade identity — is packed into one
//! `Cell`, in the same units the renderer wants them: hue in DEGREES,
//! saturation and lit as PERCENTAGES.

use crate::interior::{door_slot, Interior, Site};
use crate::lift::Storey;
use crate::rng::{hash3, hash3f};

pub const BLOCK: i32 = 32;
pub const BLOCK_BUILT: i32 = 16;
/// Nothing in the city is taller than this. The raycaster's occlusion cull
/// leans on it, so it must be a true bound, not a hope.
pub const MAX_HEIGHT: u8 = 52;

/// Ground surface classes. The ground is textured off THIS, not off a
/// road-vs-building kind byte: an even split across these is what fills the
/// lower half of the frame with striation instead of leaving it dark.
pub mod surface {
    pub const ROADWAY: u8 = 0;
    pub const PAVEMENT: u8 = 1;
    pub const PAINTED: u8 = 2;
    pub const GREENERY: u8 = 3;
    pub const PLAZA: u8 = 4;
    pub const SERVICE: u8 = 5;
    /// The floor of a building's entrance bay — the one bit of ground that is
    /// inside the building line and still walkable.
    pub const THRESHOLD: u8 = 6;
}

#[derive(Clone, Copy, Default)]
pub struct Cell {
    /// 0 = open ground. >0 = a solid column this many world units tall.
    pub height: u8,
    pub surface: u8,
    /// HSL hue in degrees.
    pub hue: u16,
    /// HSL saturation, percent.
    pub sat: u8,
    /// Probability a given window on this facade is lit, percent.
    pub lit: u8,
    /// Which window lattice this facade uses (0..3).
    pub win: u8,
    /// 0 flat, 1 curved, 2 spire, 3 masted.
    pub arch: u8,
    /// Non-zero means the cell belongs to a laid-out building and gets a
    /// facade style rather than the generic storefront.
    pub plan: u16,
    /// Offset ACROSS the avenue, 0..15, or 255 on a built cell. The ground
    /// markings — kerbs, centre lines, lane dashes — are keyed to this.
    pub cross: u8,
    /// **A way in, and which way is in.** Zero on almost every cell in the
    /// world. `1..=4` is a threshold you can walk through, and `door - 1`
    /// indexes `interior::INWARD` for the direction that takes you further in.
    /// `5..=8` is the solid wall behind one — the door face itself, which the
    /// renderer draws as a lit doorway so an entrance can be SEEN from the
    /// street.
    ///
    /// It means the same thing indoors, on the same cells, which is what makes
    /// the transition symmetric: walk far enough through a threshold and the
    /// engine changes mode, either way round.
    pub door: u8,
}

/// The hues real towers cluster into: red, amber, yellow / green, cyan, blue.
/// That clustering is what the shipped screenshots are recognisable for.
const HUE_FAMILY: [u16; 6] = [0, 30, 60, 150, 180, 210];

/// Plot splits across a 16-cell built quadrant. Picking one per axis per block
/// is what gives a block anything from one broad tower to a row of narrow ones.
const SPLITS: [&[u8]; 6] = [
    &[0, 16],
    &[0, 8, 16],
    &[0, 6, 16],
    &[0, 10, 16],
    &[0, 5, 11, 16],
    &[0, 7, 16],
];

/// How far the generator is allowed to vary between NEIGHBOURS.
///
/// A seed has always chosen *which* mix of facades you get; it has never
/// chosen *how much* mixing there is. This does. It is a grain, in blocks:
///
///   * `0` — every plot decides for itself. The default, and the look this
///     city has always had.
///   * `n >= 1` — one window lattice, one colour family, one roof shape and one
///     plot split are shared across an `n`-by-`n` block district, so the eye
///     reads a big regular grid rather than a change of pattern every plot.
///
/// It deliberately leaves the HEIGHT mix alone: a district of identical towers
/// reads as a wall rather than as a city. What it makes uniform is pattern and
/// colour.
pub fn grain_for(variety: f32) -> i32 {
    let v = variety.clamp(0.0, 1.0);
    if v >= 0.85 {
        0
    } else if v >= 0.65 {
        1
    } else if v >= 0.45 {
        2
    } else if v >= 0.28 {
        3
    } else if v >= 0.12 {
        5
    } else {
        8
    }
}

/// **Which of the two the engine is in.** Not a flag the renderer consults on
/// the side: it is what `World::cell` answers from, so the raycaster, the
/// collision and the depth buffer all follow it without knowing it exists.
///
/// Outdoors the city is a pure function of coordinate and holds nothing.
/// Indoors there is exactly one room and it is a real grid — see
/// `interior::Interior` for why those are the right opposite trades.
pub enum Place {
    Outdoors,
    Indoors(Box<Interior>),
}

pub struct World {
    pub seed: i32,
    /// District grain; see `grain_for`. Zero is today's look.
    pub grain: i32,
    pub place: Place,
}

impl World {
    pub fn new(seed: u32) -> Self {
        World { seed: seed as i32, grain: 0, place: Place::Outdoors }
    }

    /// The same city, with the generator's neighbour-to-neighbour variation
    /// scaled by `variety` in `0..=1`. One is what `new` gives.
    pub fn with_variety(seed: u32, variety: f32) -> Self {
        World { seed: seed as i32, grain: grain_for(variety), place: Place::Outdoors }
    }

    /// The block coordinates a shared choice is keyed on. At full variety that
    /// is the block itself, so nothing is shared and nothing changes.
    #[inline]
    fn district(&self, bx: i32, bz: i32) -> (i32, i32) {
        if self.grain <= 0 {
            (bx, bz)
        } else {
            (bx.div_euclid(self.grain), bz.div_euclid(self.grain))
        }
    }

    /// What a facade's own choices — lattice, colour family, roof — are keyed
    /// on. The plot only counts at full variety; below that a district's plots
    /// all answer to the same key and therefore come out the same.
    #[inline]
    fn style_key(&self, bx: i32, bz: i32, plot: i32) -> (i32, i32, i32) {
        let (kx, kz) = self.district(bx, bz);
        (kx, kz, if self.grain <= 0 { plot } else { 0 })
    }

    #[inline]
    pub fn solid(&self, x: i32, z: i32) -> bool {
        self.cell(x, z).height > 0
    }

    /// True while the engine is indoors.
    #[inline]
    pub fn indoors(&self) -> bool {
        matches!(self.place, Place::Indoors(_))
    }

    #[inline]
    pub fn interior(&self) -> Option<&Interior> {
        match &self.place {
            Place::Indoors(r) => Some(r),
            Place::Outdoors => None,
        }
    }

    /// A true upper bound on anything solid in the place we are in. The
    /// raycaster's occlusion cull leans on it, so it must be a bound and not a
    /// hope.
    ///
    /// Indoors it is NOT the ceiling, and that is the point: a room's windows
    /// look out at the city, so the tallest thing a ray from inside can reach
    /// is still a tower. It is the room's own tallest only when that is
    /// higher — which is what a room on the thirtieth floor will be.
    #[inline]
    pub fn max_height(&self) -> f32 {
        match &self.place {
            Place::Outdoors => MAX_HEIGHT as f32,
            Place::Indoors(r) => (r.tallest as f32).max(MAX_HEIGHT as f32),
        }
    }

    /// **The floors a building serves.** Empty for a building with no lift,
    /// which is most of them.
    ///
    /// The height that decides it is the building's own height at its entrance
    /// — the wall behind the doorway, `door = face + 5`, which is the part of
    /// the plot the shaft is cut down. Asking `city_cell` rather than `cell`
    /// on purpose: this is a question about the CITY, and it is asked while the
    /// engine may already be indoors.
    pub fn storeys(&self, site: Site) -> Vec<Storey> {
        let f = crate::interior::fabric(site, 0);
        if f.core.is_none() {
            return Vec::new();
        }
        // The height that decides it is the building's own height at its
        // entrance — the wall behind the doorway, `door = face + 5`, which is
        // the part of the plot the shaft is cut down and the part the core
        // stands hard against. Asking `city_cell` rather than `cell` on
        // purpose: this is a question about the CITY, and it is asked while the
        // engine may already be indoors.
        let (ix, iz) = crate::interior::INWARD[(site.face as usize).min(3)];
        let h = self.city_cell(site.dx + ix, site.dz + iz).height;
        crate::interior::plan_storeys(site, h)
    }

    /// Everything about one cell of wherever we are. One well-predicted branch
    /// in front of the whole engine, and it is the only place the two modes
    /// meet.
    #[inline]
    pub fn cell(&self, gx: i32, gz: i32) -> Cell {
        match &self.place {
            Place::Outdoors => self.city_cell(gx, gz),
            // **A room is not a sealed box.** Where the room has nothing to
            // say — past its own glazing, out through its own doorway — the
            // CITY answers, and the ray that was crossing the room carries
            // straight on down the street. That fall-through is the whole
            // mechanism behind seeing out of a window; there is no second
            // world and no backdrop.
            Place::Indoors(r) => match r.at(gx, gz) {
                Some(c) => c,
                None => self.city_cell(gx, gz),
            },
        }
    }

    /// Everything about one cell of the CITY. Pure, deterministic, no
    /// allocation.
    pub fn city_cell(&self, gx: i32, gz: i32) -> Cell {
        let bx = gx.div_euclid(BLOCK);
        let bz = gz.div_euclid(BLOCK);
        let ox = gx.rem_euclid(BLOCK);
        let oz = gz.rem_euclid(BLOCK);
        let s = self.seed;

        if ox >= BLOCK_BUILT || oz >= BLOCK_BUILT {
            // An island tower standing in the middle of the crossroads. Without
            // something out there, a straight avenue on an infinite grid is an
            // infinite corridor and the vanishing point is a black hole
            // where there should be a skyline. The island is 4x4 inside a
            // 16-wide junction, so there is always six cells of clearance to
            // walk round it on either axis.
            if (22..=25).contains(&ox)
                && (22..=25).contains(&oz)
                && hash3(bx.wrapping_mul(3163), bz.wrapping_mul(4079), s ^ 0x1B) % 100 < 34
            {
                let (dx, dz) = self.district(bx, bz);
                return island_cell(ox, oz, bx, bz, dx, dz, s);
            }
            return street_cell(ox, oz, gx, gz, bx, bz, s);
        }

        // --- the built quadrant ------------------------------------------
        let civic = hash3(bx.wrapping_mul(7919), bz.wrapping_mul(6421), s) % 100;
        if civic < 4 {
            // A park: the whole quadrant is open greenery. Gives the skyline
            // gaps to breathe through and the street somewhere to open out.
            return Cell {
                surface: surface::GREENERY,
                cross: 255,
                ..Default::default()
            };
        }
        if civic < 8 {
            return Cell { surface: surface::PLAZA, cross: 255, ..Default::default() };
        }

        // The plot split is a district-wide choice too: a run of blocks that
        // splits the same way is most of what makes a skyline read as regular.
        let (dx, dz) = self.district(bx, bz);
        let sx = SPLITS[(hash3(dx, dz, s ^ 0x51) % 6) as usize];
        let sz = SPLITS[(hash3(dz, dx, s ^ 0xA7) % 6) as usize];
        let (ix, lx0, lx1) = seg(sx, ox);
        let (iz, lz0, lz1) = seg(sz, oz);
        let plot = (ix * 4 + iz) as i32;

        // Plots on the quadrant edge that touch the avenue keep a 1-cell
        // forecourt, so a tower never grows flush into the roadway and the
        // storefront band has pavement to stand on.
        if lx1 == BLOCK_BUILT && ox == BLOCK_BUILT - 1 {
            return Cell { surface: surface::PAVEMENT, cross: 255, ..Default::default() };
        }
        if lz1 == BLOCK_BUILT && oz == BLOCK_BUILT - 1 {
            return Cell { surface: surface::PAVEMENT, cross: 255, ..Default::default() };
        }

        let pw = lx1 - lx0;
        let pd = lz1 - lz0;
        let r = hash3f(bx.wrapping_mul(2654) + plot, bz.wrapping_mul(1597) + plot * 31, s);

        // Height mix. Most of what you see down a long avenue should be a
        // tower — 20-plus storeys — with enough mid- and low-rise to break the
        // roofline.
        let hh = hash3(bx * 37 + plot, bz * 61 + plot * 7, s ^ 0x9E);
        let core_h: i32 = if r < 0.56 {
            26 + (hh % 21) as i32
        } else if r < 0.82 {
            13 + (hh % 12) as i32
        } else {
            5 + (hh % 8) as i32
        };

        // A courtyard on a big plot: keeps deep blocks from reading as one
        // solid mass and lets light down between towers. A plot with one has no
        // middle, so it can carry no profile that needs a middle — `profile`
        // takes `hollow` for exactly that reason.
        let hollow = pw > 6 && pd > 6 && (hash3(bx + plot, bz - plot, s ^ 0x33) % 3) == 0;
        let interior = ox > lx0 + 1 && ox < lx1 - 2 && oz > lz0 + 1 && oz < lz1 - 2;
        if hollow && interior {
            return Cell { surface: surface::SERVICE, cross: 255, ..Default::default() };
        }

        // How far in from the plot's edge this cell is, and how many rings the
        // plot has. A height field can only make a silhouette out of the cells
        // WITHIN a plot, so the profile is a function of the ring index — see
        // `profile`.
        let e = (ox - lx0).min(lx1 - 1 - ox).min(oz - lz0).min(lz1 - 1 - oz).max(0);
        let rings = (pw.min(pd) + 1) / 2;

        // --- the way in ---------------------------------------------------
        // A building that faces a street gets an entrance bay: two cells of
        // its outermost ring taken out down to the pavement, with the wall
        // behind them marked as the door face so the renderer can light it.
        // Only the outer rings can be one, which is what keeps this off the
        // cost of every other cell in the plot: on the two faces that carry a
        // forecourt the threshold is one ring in, so the wall behind it is two.
        //
        // A plot built round a courtyard is not offered one: on the +X and +Z
        // faces its solid ring is a single cell deep, so an entrance bay would
        // punch straight through into the courtyard and the door would open on
        // to open ground. `door_slot` refuses those rather than the geometry
        // being fixed up afterwards.
        let mut door = 0u8;
        // Four integer compares in front of `door_slot`'s two hashes, and they
        // reject all but a handful of columns per plot. This runs for every
        // built cell in the city on every frame — the ground pass alone asks
        // for thousands — so the cheap rejection goes first, the same way
        // `props_near` puts its box test in front of `cell()`.
        // The threshold and the wall behind it sit on exactly two columns of a
        // face, so this asks for exactly those two and not for a band.
        let on_face = (lx0 == 0 && (ox == 0 || ox == 1))
            || (lx1 == BLOCK_BUILT && (ox == lx1 - 2 || ox == lx1 - 3))
            || (lz0 == 0 && (oz == 0 || oz == 1))
            || (lz1 == BLOCK_BUILT && (oz == lz1 - 2 || oz == lz1 - 3));
        if on_face && !hollow {
            if let Some((face, a0)) = door_slot(bx, bz, plot, lx0, lx1, lz0, lz1, BLOCK_BUILT, s) {
                // The threshold ring, and the wall one step further in.
                let (thr, back, along) = match face {
                    0 => (lx0, lx0 + 1, oz),
                    1 => (lx1 - 2, lx1 - 3, oz),
                    2 => (lz0, lz0 + 1, ox),
                    _ => (lz1 - 2, lz1 - 3, ox),
                };
                let across = if face < 2 { ox } else { oz };
                if along >= a0 && along <= a0 + 1 {
                    if across == thr {
                        return Cell {
                            height: 0,
                            surface: surface::THRESHOLD,
                            cross: 255,
                            door: face + 1,
                            plan: 1 + (hash3(bx * 733 + plot, bz * 947, s ^ 0xBE) % 60000) as u16,
                            ..Default::default()
                        };
                    }
                    if across == back {
                        door = face + 5;
                    }
                }
            }
        }

        let (kx, kz, kp) = self.style_key(bx, bz, plot);
        // The profile and the roof texture come off ONE hash, because a spire
        // has to be a spire in outline as well as in texture. A district shares
        // it at low variety, along with everything else it shares.
        let (h_prof, arch) =
            profile(core_h, e, rings, hollow, hash3(kx * 401 + kp, kz * 409, s ^ 0x88));
        let height = h_prof.clamp(2, MAX_HEIGHT as i32) as u8;
        let hue_base = HUE_FAMILY[(hash3(kx * 13 + kp, kz * 29 + kp, s ^ 0x5A) % 6) as usize];
        let jitter = (hash3(kx + 91, kz + kp * 3, s) % 17) as i32 - 8;
        let hue = ((hue_base as i32 + jitter).rem_euclid(360)) as u16;

        Cell {
            height,
            surface: surface::PAVEMENT,
            hue,
            sat: 50 + (hash3(kx * 5, kz * 11 + kp, s ^ 0x1D) % 50) as u8,
            lit: 30 + (hash3(kx * 3 + kp, kz * 17, s ^ 0x2C) % 61) as u8,
            win: (hash3(kx * 101 + kp, kz * 103, s ^ 0x77) % 4) as u8,
            arch,
            // 1..=65535, never 0: a plot always has an identity.
            plan: 1 + (hash3(bx * 733 + plot, bz * 947, s ^ 0xBE) % 60000) as u16,
            cross: 255,
            door,
        }
    }
}

/// Which segment of a split contains offset `o`, and that segment's bounds.
#[inline]
fn seg(split: &[u8], o: i32) -> (usize, i32, i32) {
    for i in 0..split.len() - 1 {
        if o < split[i + 1] as i32 {
            return (i, split[i] as i32, split[i + 1] as i32);
        }
    }
    let n = split.len();
    (n - 2, split[n - 2] as i32, split[n - 1] as i32)
}

/// What a cell `e` rings in from the edge of a plot whose core is `core_h`
/// tall actually stands at, and what the top of it is (`Cell::arch`).
///
/// **A building here is a height field, so its outline is made out of the cells
/// WITHIN its plot or it is not made at all.** For a long time this did one
/// thing — a single 3-unit setback on the outermost ring of a third of tall
/// plots — and from the street that was enough. From a rooftop, where you are
/// looking at a skyline, it reads as a field of flat-topped boxes.
///
/// So a tall plot now gets one of six profiles, and the *mix* is as much of the
/// design as the shapes are. A skyline where every tower is a ziggurat is not a
/// city either: nearly a third of tall stock stays deliberately flat, and
/// everything under 20 units is flat or close to it, so there is still a
/// baseline for a stepped or spired tower to stand out against.
///
/// `arch` comes out of the same decision rather than off its own hash, which is
/// what makes a spire a spire in OUTLINE and not only in texture.
fn profile(core_h: i32, e: i32, rings: i32, hollow: bool, h: u32) -> (i32, u8) {
    let max = MAX_HEIGHT as i32;
    if core_h < 13 || rings < 2 {
        // Low-rise, and any plot too narrow to have an inside: a box is a box.
        return (core_h, 0);
    }
    if core_h < 20 {
        // Mid-rise: mostly flat, a minority with one shallow setback. This is
        // roughly what the whole city used to do, kept for the stock that
        // should not be drawing attention to itself.
        let drop = if (h >> 5) % 100 < 30 && e == 0 { 2 } else { 0 };
        return ((core_h - drop).max(2), 0);
    }
    // `e` runs 0 at the plot's edge to `inner` at its middle.
    let inner = (rings - 1).max(1);
    // A plot built round a courtyard has no middle cells at all, so a crown, a
    // spire or a mast would have nothing to stand on and the tower would come
    // out flat while still being TEXTURED as a spire. Those three are simply
    // not on offer there; the outline and the texture stay honest to each other.
    match if hollow { h % 68 } else { h % 100 } {
        // Flat-topped, and kept that way on purpose.
        0..=29 => (core_h, 0),
        // Stepped: two or three setback rings, so the tower steps in as it
        // rises rather than going straight up and stopping.
        30..=51 => {
            let steps = 2 + ((h >> 7) % 2) as i32;
            let drop = 3 + ((h >> 9) % 3) as i32;
            ((core_h - drop * (steps - e).max(0)).max(4), 0)
        }
        // Tapered: a continuous narrowing to a crown rather than a stack of
        // slabs. `arch` 1 is the curved top, which is now what it is shaped
        // like as well as what it is textured as.
        52..=67 => {
            let t = e.min(inner) as f32 / inner as f32;
            let base = core_h as f32 * 0.58;
            ((base + (core_h as f32 - base) * t.powf(0.55)).round() as i32, 1)
        }
        // A crown: a flat body with a smaller block set on top of it.
        68..=79 => {
            let cap = 4 + ((h >> 11) % 5) as i32;
            let body = core_h.min(max - cap);
            (if e >= inner - 1 { body + cap } else { body }, 0)
        }
        // A spire, with a shoulder under it. The body comes DOWN to make room
        // rather than the spire being clipped at MAX_HEIGHT, which would leave
        // the tallest towers — the ones you can actually see — with no spire.
        80..=89 => {
            let spire = 8 + ((h >> 13) % 7) as i32;
            let body = core_h.min(max - spire);
            let top = if e == inner {
                body + spire
            } else if e == inner - 1 {
                body + spire / 3
            } else {
                body
            };
            (top, 2)
        }
        // A mast: a needle on the very middle of an otherwise flat roof.
        _ => {
            let mast = 6 + ((h >> 15) % 6) as i32;
            let body = core_h.min(max - mast);
            (if e >= inner { body + mast } else { body }, 3)
        }
    }
}

/// A tower standing alone in the middle of a junction.
#[inline]
fn island_cell(ox: i32, oz: i32, bx: i32, bz: i32, dx: i32, dz: i32, s: i32) -> Cell {
    let h = hash3(bx * 811, bz * 977, s ^ 0x6D);
    let core = 16 + (h % 25) as i32;
    // Chamfer the corners so it reads as a slim column, not a cube.
    let corner = (ox == 22 || ox == 25) && (oz == 22 || oz == 25);
    // Colour follows the district it stands in, so an island tower is not the
    // one thing on a uniform street that changes family.
    let hue_base = HUE_FAMILY[(hash3(dx * 19, dz * 23, s ^ 0x3E) % 6) as usize];
    Cell {
        height: (core - if corner { 4 } else { 0 }).clamp(6, MAX_HEIGHT as i32) as u8,
        surface: surface::PAVEMENT,
        hue: hue_base,
        sat: 55 + (h % 45) as u8,
        lit: 35 + (hash3(bx * 7, bz * 13, s) % 55) as u8,
        win: (h % 4) as u8,
        arch: if h % 8 == 0 { 3 } else { 0 },
        plan: 1 + (hash3(bx * 431, bz * 577, s ^ 0xD1) % 60000) as u16,
        cross: 255,
        door: 0,
    }
}

/// The avenue. 16 cells across: pavement out to the kerb at 4, roadway 5..10,
/// kerb at 11, pavement beyond. Centre lines land at 7 and 8, lane dashes at
/// 6 and 9, so the markings read as a carriageway from a standing eye.
#[inline]
fn street_cell(ox: i32, oz: i32, gx: i32, gz: i32, bx: i32, bz: i32, s: i32) -> Cell {
    let along_x = oz >= BLOCK_BUILT; // strip running along X
    let cross = if along_x && ox >= BLOCK_BUILT {
        // Intersection: take whichever axis is nearer its centre line, so the
        // carriageway carries straight through rather than breaking up.
        let cx = ox - BLOCK_BUILT;
        let cz = oz - BLOCK_BUILT;
        if (cz as f32 - 7.5).abs() < (cx as f32 - 7.5).abs() { cz } else { cx }
    } else if along_x {
        oz - BLOCK_BUILT
    } else {
        ox - BLOCK_BUILT
    };

    let roadway = (5..=10).contains(&cross);
    let mut surf = if roadway { surface::ROADWAY } else { surface::PAVEMENT };

    // A minority of pavement runs are painted forecourt or planted verge.
    // Roughly 110:105:33:13 across roadway / pavement / painted / greenery —
    // it is that split that gives the lower half of the frame something to
    // striate instead of one flat grey.
    if !roadway {
        // Runs of three cells, so the verge and the painted forecourt read as
        // markings on the pavement rather than as fields.
        let run = if along_x { gx.div_euclid(3) } else { gz.div_euclid(3) };
        let v = hash3(bx * 71 + run, bz * 89 + run * 7, s) % 12;
        if v < 3 {
            surf = surface::PAINTED;
        } else if v == 3 && (cross <= 1 || cross >= 14) {
            // Planting only ever sits against the building line.
            surf = surface::GREENERY;
        }
    }

    Cell {
        height: 0,
        surface: surf,
        hue: 0,
        sat: 0,
        lit: 0,
        win: 0,
        arch: 0,
        plan: 0,
        cross: cross.clamp(0, 15) as u8,
        door: 0,
    }
}

impl World {
    /// The nearest entrance to a point, as `(cell x, cell z, face)`.
    ///
    /// A box scan, and deliberately so: this is for the tools and the tests —
    /// `--doorway`, and the checks that a door is reachable — not for the
    /// frame. Nothing in the render path ever looks for a door; the renderer
    /// is handed one by the raycaster like any other cell.
    pub fn door_near(&self, cx: f32, cz: f32, radius: i32) -> Option<(i32, i32, u8)> {
        let (ix, iz) = (cx.floor() as i32, cz.floor() as i32);
        let mut best: Option<(f32, i32, i32, u8)> = None;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let (x, z) = (ix + dx, iz + dz);
                let c = self.city_cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let d = (x as f32 + 0.5 - cx).hypot(z as f32 + 0.5 - cz);
                if best.map_or(true, |(b, ..)| d < b) {
                    best = Some((d, x, z, c.door - 1));
                }
            }
        }
        best.map(|(_, x, z, f)| (x, z, f))
    }
}

// --- street furniture ----------------------------------------------------
/// The things that stand on a pavement. Not decoration painted over the frame
/// afterwards: like every other feature of this city they are a pure function
/// of position, so they are in the same place every time you walk past, and
/// they are drawn through the same distance falloff and the same per-column
/// wall buffer as everything else — lit by distance, and hidden behind a
/// facade that is nearer than they are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prop {
    /// A tall post with a lit head, standing just inside the kerb.
    Lamp,
    /// A street tree, against the building line where the planted verge runs.
    Tree,
    /// A low planted box on the forecourt.
    Planter,
}

/// One placed piece of street furniture, in world coordinates.
#[derive(Clone, Copy)]
pub struct Placed {
    pub x: f32,
    pub z: f32,
    pub kind: Prop,
    /// World units tall. Varies a little per instance so a run of lamps down a
    /// street is not a comb.
    pub height: f32,
    /// Per-instance hash, for whatever the renderer wants to vary.
    pub seed: u32,
}

/// Cross-street offsets things stand at. The avenue is 16 cells wide: pavement
/// out to the kerb at 4, roadway 5..10, kerb at 11, pavement beyond. So a lamp
/// belongs just inside the kerb, and a tree against the building line where the
/// planted verge already runs.
const LAMP_CROSS: [i32; 2] = [3, 12];
const TREE_CROSS: [i32; 2] = [1, 14];
const PLANTER_CROSS: [i32; 2] = [2, 13];
/// Spacing along the street, in cells. Deliberately coprime with each other and
/// with the 32-cell block, so lamps, trees and planters do not line up into
/// rows of three.
const LAMP_EVERY: i32 = 9;
const TREE_EVERY: i32 = 7;
const PLANTER_EVERY: i32 = 13;

impl World {
    /// Every piece of street furniture within `radius` of a point, appended to
    /// `out`.
    ///
    /// It walks the street lines rather than the area: furniture only ever
    /// stands at four known cross-offsets of an avenue, at a known spacing
    /// along it, so the candidates can be *generated* instead of found. That is
    /// a few hundred cell lookups a frame rather than the tens of thousands a
    /// box scan of the same ground would cost.
    pub fn props_near(&self, cx: f32, cz: f32, radius: f32, out: &mut Vec<Placed>) {
        out.clear();
        let r = radius.ceil() as i32;
        let (ix, iz) = (cx.floor() as i32, cz.floor() as i32);
        let r2 = radius * radius;
        // Streets running along X sit in the z strip of each block row, and
        // vice versa. Both are walked the same way, with the axes swapped.
        for along_x in [true, false] {
            // The block index of the strip, on the axis ACROSS the street.
            let across0 = (if along_x { iz - r } else { ix - r }).div_euclid(BLOCK);
            let across1 = (if along_x { iz + r } else { ix + r }).div_euclid(BLOCK);
            for b in across0..=across1 {
                let strip = b * BLOCK + BLOCK_BUILT;
                for (crosses, every, kind) in [
                    (LAMP_CROSS, LAMP_EVERY, Prop::Lamp),
                    (TREE_CROSS, TREE_EVERY, Prop::Tree),
                    (PLANTER_CROSS, PLANTER_EVERY, Prop::Planter),
                ] {
                    for c in crosses {
                        let across = strip + c;
                        let lo = (if along_x { ix } else { iz }) - r;
                        let hi = (if along_x { ix } else { iz }) + r;
                        // Phase the run off the strip so parallel streets are
                        // not in lockstep.
                        let phase = (hash3(b, c, self.seed ^ 0x9A) % every as u32) as i32;
                        let mut a = lo - lo.rem_euclid(every) + phase;
                        while a < lo {
                            a += every;
                        }
                        while a <= hi {
                            let (gx, gz) = if along_x { (a, across) } else { (across, a) };
                            a += every;
                            // Never in the middle of a junction: the cross
                            // offsets only mean what they say on a plain strip.
                            if gx.rem_euclid(BLOCK) >= BLOCK_BUILT
                                && gz.rem_euclid(BLOCK) >= BLOCK_BUILT
                            {
                                continue;
                            }
                            // Cheapest rejections first. `cell()` is a
                            // handful of hashes and is the only expensive
                            // thing in this loop, so the box-corner and
                            // leave-a-gap tests go in front of it — that is
                            // most of the candidates gone for almost nothing.
                            let dx = gx as f32 + 0.5 - cx;
                            let dz = gz as f32 + 0.5 - cz;
                            if dx * dx + dz * dz > r2 {
                                continue;
                            }
                            let h = hash3(gx.wrapping_mul(2237), gz.wrapping_mul(3571),
                                          self.seed ^ 0x7C);
                            // A street is not a catalogue: leave gaps.
                            let keep = match kind {
                                Prop::Lamp => h % 100 < 88,
                                Prop::Tree => h % 100 < 70,
                                Prop::Planter => h % 100 < 34,
                            };
                            if !keep {
                                continue;
                            }
                            // Street furniture is an outdoor thing; the city
                            // answers directly.
                            let cell = self.city_cell(gx, gz);
                            if cell.height != 0 || cell.surface == surface::ROADWAY {
                                continue;
                            }
                            // Stand it at the cell centre, nudged off-centre so
                            // a row does not read as a ruled line.
                            let jx = ((h >> 8) % 5) as f32 / 12.0 - 0.17;
                            let jz = ((h >> 12) % 5) as f32 / 12.0 - 0.17;
                            let x = gx as f32 + 0.5 + jx;
                            let z = gz as f32 + 0.5 + jz;
                            let height = match kind {
                                Prop::Lamp => 4.3 + ((h >> 16) % 9) as f32 * 0.11,
                                Prop::Tree => 3.1 + ((h >> 16) % 13) as f32 * 0.16,
                                Prop::Planter => 0.62 + ((h >> 16) % 5) as f32 * 0.05,
                            };
                            out.push(Placed { x, z, kind, height, seed: h });
                        }
                    }
                }
            }
        }
    }
}
