//! AsciiWorldEngine — a walkable ASCII city.
//!
//! Everything that decides what you see lives in here: the city, the camera,
//! the projection, the raycaster, the glyphs and the colour. A frontend does
//! four things and no more — hand over the elapsed time and an input bitmask,
//! ask for a frame, read one flat buffer, and paint it.
//!
//! ```no_run
//! use asciicity::{Engine, camera::key};
//! let mut e = Engine::new(180, 80, 1.0, 2.0, 0xACC1);
//! e.step(1.0 / 60.0, key::FWD | key::SPRINT, 0.0, 0.0);
//! e.render();
//! let frame = e.frame();          // [glyph, r, g, b] per cell, row-major
//! assert_eq!(frame.len(), 180 * 80 * 4);
//! ```
//!
//! The frame buffer is deliberately one flat byte run. Crossing a language
//! boundary with an object graph per cell is where terminals-in-browsers lose
//! their frame budget; this crosses it with a single `memcpy`-shaped read.

pub mod camera;
pub mod entities;
/// A film script: the same key bitmask a keyboard produces, from a file.
pub mod film;
pub mod interior;
/// The lift: which buildings have one, what floors it serves, and where the
/// car is. All world model; the renderer decides nothing about it.
pub mod lift;
pub mod output;
pub mod palette;
pub mod project;
pub mod raycast;
pub mod render;
pub mod rng;
pub mod world;

#[cfg(feature = "tui")]
pub mod term;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use camera::Camera;
pub use interior::{Interior, Site};
pub use lift::Lift;
pub use world::Place;
pub use output::{grid_to_ansi, grid_to_svg, grid_to_text};
pub use project::Projection;
pub use render::Grid;
pub use world::World;

/// Per-frame cost, in microseconds, split the way it is worth splitting:
/// simulation, raycast and render. Paint is the frontend's own to measure.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub sim_us: f32,
    pub cast_us: f32,
    pub render_us: f32,
}

impl Stats {
    /// Engine-side milliseconds per frame — everything except paint.
    pub fn engine_ms(&self) -> f32 {
        (self.sim_us + self.cast_us + self.render_us) / 1000.0
    }
}

pub struct Engine {
    pub world: World,
    pub cam: Camera,
    pub proj: Projection,
    pub rays: raycast::Rays,
    pub renderer: render::Renderer,
    pub grid: Grid,
    pub pop: entities::Population,
    /// The weather. Off by default: it is the one pass here that costs
    /// anything, so it is asked for rather than assumed.
    pub sky: entities::Sky,
    pub stats: Stats,
    /// Seconds since the engine started. The only thing that needs it is the
    /// star twinkle, but a renderer with no clock at all cannot have anything
    /// that breathes.
    pub time: f32,
    /// Whether the act key was down last frame. Interaction is an EDGE, not a
    /// state: holding the button down must press the panel once, not sixty
    /// times a second. The engine owns the edge rather than the frontend so a
    /// film script and a test press it the same way a keyboard does.
    act_held: bool,
    /// What the last press did, for a frontend that wants to say so.
    pub act_note: &'static str,
    frame: Vec<u8>,
}

/// **Where the nearest lift is.** What `Engine::nearest_lift` found: the
/// entrance cell, the building's name, how many floors it serves, how far off
/// it is and whether it is one of the landmark-shaped ones you can pick out of
/// a skyline. Fixed bytes rather than a `String`, so asking costs no heap.
#[derive(Clone, Copy)]
pub struct Wayfind {
    pub x: i32,
    pub z: i32,
    pub dist: f32,
    pub floors: usize,
    pub landmark: bool,
    name: [u8; 24],
    name_len: usize,
}

impl Wayfind {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// Which way to go, as a compass point. `+Z` is south, which is the
    /// convention the whole engine uses — `Camera::yaw` zero looks north.
    pub fn compass(&self, from_x: f32, from_z: f32) -> &'static str {
        const POINTS: [&str; 8] =
            ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
        let (dx, dz) = (self.x as f32 + 0.5 - from_x, self.z as f32 + 0.5 - from_z);
        // Bearing clockwise from north, and north is `-Z`.
        let bearing = dx.atan2(-dz).rem_euclid(core::f32::consts::TAU);
        POINTS[((bearing / (core::f32::consts::TAU / 8.0)).round() as usize) % 8]
    }

    /// How far to TURN to be facing it, in radians, signed: positive is to the
    /// right. A compass point says where it is; this says what to do about it.
    pub fn turn_from(&self, from_x: f32, from_z: f32, yaw: f32) -> f32 {
        let (dx, dz) = (self.x as f32 + 0.5 - from_x, self.z as f32 + 0.5 - from_z);
        let want = dx.atan2(-dz);
        let mut d = want - yaw;
        while d > core::f32::consts::PI {
            d -= core::f32::consts::TAU;
        }
        while d < -core::f32::consts::PI {
            d += core::f32::consts::TAU;
        }
        d
    }

    /// "ahead", "to your left", "behind you" — the turn as a player would say
    /// it, so the HUD does not have to print radians at somebody.
    pub fn hand(&self, from_x: f32, from_z: f32, yaw: f32) -> &'static str {
        let d = self.turn_from(from_x, from_z, yaw);
        let a = d.abs();
        if a < 0.35 {
            "straight ahead"
        } else if a > 2.35 {
            "behind you"
        } else if a > 1.2 {
            if d > 0.0 { "hard right" } else { "hard left" }
        } else if d > 0.0 {
            "to your right"
        } else {
            "to your left"
        }
    }
}

impl Engine {
    /// `cell_w` / `cell_h` are the display cell's aspect — 1:2 for a terminal
    /// character, roughly 5.5:9 for a canvas of square-ish glyphs. It is the
    /// only thing that sets horizontal FOV.
    pub fn new(cols: usize, rows: usize, cell_w: f32, cell_h: f32, seed: u32) -> Self {
        Engine::with_variety(cols, rows, cell_w, cell_h, seed, 1.0)
    }

    /// The same, with the facade generator's neighbour-to-neighbour variation
    /// scaled by `variety` in `0..=1`. One is what `new` gives — a seed picks
    /// WHICH mix of facades you get; this picks HOW MUCH mixing there is. See
    /// `world::grain_for`.
    pub fn with_variety(
        cols: usize,
        rows: usize,
        cell_w: f32,
        cell_h: f32,
        seed: u32,
        variety: f32,
    ) -> Self {
        let world = World::with_variety(seed, variety);
        // Spawn well away from the origin so the city around us is not the
        // corner case of the block grid.
        let cam = camera::spawn(&world, 4096, 4096);
        let mut proj = Projection::new(cols, rows, cell_w, cell_h);
        proj.set_view(cam.pitch, cam.eye);
        let pop = entities::Population::new(&world, cam.x, cam.z, seed as u64);
        let sky = entities::Sky::new(seed as u64);
        Engine {
            rays: raycast::Rays::new(cols),
            renderer: render::Renderer::new(cols),
            grid: Grid::new(cols, rows),
            world,
            cam,
            proj,
            pop,
            sky,
            stats: Stats::default(),
            time: 0.0,
            act_held: false,
            act_note: "",
            frame: Vec::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell_w: f32, cell_h: f32) {
        if cols == self.proj.cols && rows == self.proj.rows {
            return;
        }
        self.proj = Projection::new(cols, rows, cell_w, cell_h);
        self.proj.set_view(self.cam.pitch, self.cam.eye);
        self.rays = raycast::Rays::new(cols);
        self.renderer = render::Renderer::new(cols);
        self.grid.resize(cols, rows);
    }

    /// Advance the world. `look_x` / `look_y` are pointer deltas already in
    /// radians; a terminal frontend passes zero and uses the turn keys.
    pub fn step(&mut self, dt: f32, keys: u32, look_x: f32, look_y: f32) {
        let t0 = Clock::now();
        self.time += dt;
        self.cam.update(&self.world, dt, keys, look_x, look_y);
        // **The ride.** It runs before the eye is placed, because the eye
        // stands on the car's slab and the slab is what moves. `Lift::update`
        // walks a smoothstep over the trip, so `y` is a continuous function of
        // time and there is no setting of it that jumps a floor.
        if let Place::Indoors(r) = &mut self.world.place {
            if let Some(mut l) = r.lift {
                l.update(&r.storeys, dt);
                r.lift = Some(l);
                r.retune();
            }
        }
        // One bit of the bitmask, edge-triggered here so every frontend gets
        // the same behaviour for free.
        let act = keys & camera::key::ACT != 0;
        if act && !self.act_held {
            self.act();
        }
        self.act_held = act;
        if let Some(base) = self.world.interior().map(|r| r.base) {
            // Inside, the eye is a person's eye standing on that floor's slab
            // and nothing else. The vista is a thing you do over rooftops:
            // allowing it here would lift the camera through the ceiling AND
            // turn collision off with it, since `Camera::airborne` is what
            // gates both.
            self.cam.ground = base;
            self.cam.eye_target = base + camera::EYE_STREET;
            self.cam.eye = self.cam.eye_target;
        } else {
            self.cam.ground = 0.0;
            // The traffic and the weather are the street's. They are not
            // stepped while you are inside — not to save the time, though it
            // does, but because feeding them a camera that is standing in a
            // room would recycle the whole population around a point that is
            // not on any street.
            self.pop.update(&self.world, dt, self.cam.x, self.cam.z);
            self.sky.update(dt, self.cam.x, self.cam.z);
        }
        self.portal();
        self.proj.set_view(self.cam.pitch, self.cam.eye);
        self.stats.sim_us = t0.us();
    }

    /// **The whole of the transition.** Walk far enough through a threshold and
    /// the engine changes what it is; walk back through it and it changes
    /// back. Both directions are the same three lines because `Cell::door`
    /// means the same thing on both sides of it.
    ///
    /// Nothing is teleported. A room is built in the SAME world coordinates as
    /// the doorway it belongs to, so the camera is standing in the same cell
    /// the instant before and the instant after — what changed is what that
    /// cell is part of.
    ///
    /// Collision stops a walker `Camera::RADIUS` short of a solid face, and the
    /// solid face here is the door outside and the street beyond it inside, so
    /// walking into either one lands inside `PORTAL_GAP` and the mode flips.
    /// The other threshold plane is then most of a cell away, which is the
    /// hysteresis: there is no setting of the two that oscillates.
    fn portal(&mut self) {
        let cx = self.cam.x.floor() as i32;
        let cz = self.cam.z.floor() as i32;
        let c = self.world.cell(cx, cz);
        // 1..=4 a street threshold, 5..=8 the wall behind one, 9..=12 a lift
        // landing. All three mean the same thing in the low two bits: which way
        // is IN.
        if c.door == 0 || (5..=8).contains(&c.door) || c.door > 12 {
            return;
        }
        let lift_door = c.door >= 9;
        let (ix, iz) = interior::INWARD[((c.door - 1) % 4) as usize];
        // How far through the threshold cell we are, toward the inside.
        let f = if ix > 0 {
            self.cam.x - cx as f32
        } else if ix < 0 {
            1.0 - (self.cam.x - cx as f32)
        } else if iz > 0 {
            self.cam.z - cz as f32
        } else {
            1.0 - (self.cam.z - cz as f32)
        };
        // What we are stepping into, decided while nothing is borrowed.
        enum Step {
            Nothing,
            Street,
            Room(interior::Site, i32, f32, Vec<lift::Storey>),
            Car(usize),
        }
        let step = match &self.world.place {
            Place::Outdoors => {
                // Over the rooftops there is no walking through anything, and a
                // lift landing does not exist out here.
                if lift_door || self.cam.airborne() || 1.0 - f > interior::PORTAL_GAP {
                    Step::Nothing
                } else {
                    let site = interior::Site {
                        seed: self.world.seed,
                        dx: cx,
                        dz: cz,
                        face: c.door - 1,
                        plan: c.plan,
                        grain: self.world.grain,
                    };
                    // Off the street you land on the ground floor. Whether this
                    // building has a lift at all is settled here, once, off its
                    // own height at its own entrance.
                    let storeys = self.world.storeys(site);
                    Step::Room(site, 0, 0.0, storeys)
                }
            }
            // In the car. The only way out of one is its landing doors, and
            // they are only a threshold at all while it is standing level.
            Place::Indoors(r) if r.lift.is_some() => {
                let l = r.lift.unwrap();
                if !lift_door || !l.level() || f > interior::PORTAL_GAP {
                    Step::Nothing
                } else {
                    let st = r.storeys[l.at];
                    Step::Room(r.site, st.floor, st.base, r.storeys.clone())
                }
            }
            // In a room: out to the street through the door you came in by, or
            // into the lift through the core's flank.
            Place::Indoors(r) => {
                if lift_door {
                    match r.storey_index() {
                        Some(i) if 1.0 - f <= interior::PORTAL_GAP => Step::Car(i),
                        _ => Step::Nothing,
                    }
                } else if f > interior::PORTAL_GAP {
                    Step::Nothing
                } else {
                    Step::Street
                }
            }
        };
        match step {
            Step::Nothing => {}
            Step::Street => {
                self.cam.ground = 0.0;
                self.world.place = Place::Outdoors;
            }
            Step::Room(site, floor_no, base, storeys) => {
                let room = Interior::build(site, floor_no, base, storeys);
                self.cam.ground = room.base;
                self.cam.eye_target = room.base + camera::EYE_STREET;
                self.cam.eye = self.cam.eye_target;
                self.world.place = Place::Indoors(Box::new(room));
            }
            Step::Car(i) => {
                let car = match &self.world.place {
                    Place::Indoors(r) => Interior::car(r, i),
                    Place::Outdoors => return,
                };
                self.cam.ground = car.base;
                self.cam.eye_target = car.base + camera::EYE_STREET;
                self.cam.eye = self.cam.eye_target;
                self.world.place = Place::Indoors(Box::new(car));
            }
        }
    }

    /// **The panel.** One bit of the input bitmask, and the world model decides
    /// what it means: the nearest fixture within reach is the thing you are
    /// acting on, which is what `Interior::interaction_near` was built to
    /// answer. In a car that is one of the two call buttons — whichever end of
    /// it you are standing at — and pressing it sends the car one floor that
    /// way. Everywhere else there is nothing to press yet.
    ///
    /// Deliberately NOT a trigger you walk into: you have to be at the panel
    /// and you have to press.
    pub fn act(&mut self) -> bool {
        let Place::Indoors(r) = &mut self.world.place else {
            self.act_note = "";
            return false;
        };
        let Some(kind) = r.interaction_near(self.cam.x, self.cam.z).map(|(p, _)| p.kind) else {
            self.act_note = "";
            return false;
        };
        let dir = match kind {
            interior::Fitting::CallUp => 1,
            interior::Fitting::CallDown => -1,
            interior::Fitting::LiftSign => {
                self.act_note = "step in";
                return false;
            }
            _ => {
                self.act_note = "";
                return false;
            }
        };
        let Some(mut l) = r.lift else {
            self.act_note = "";
            return false;
        };
        let press = l.call(&r.storeys, dir);
        if press != lift::Press::Refused {
            r.lift = Some(l);
        }
        // **One press is a whole journey, and the second press is the brake.**
        // The note says so, because a control you have to discover by holding
        // it down is the control that shipped and was wrong.
        self.act_note = match press {
            lift::Press::Sent if dir > 0 => "going up — press again to stop",
            lift::Press::Sent => "going down — press again to stop",
            lift::Press::Stopping => "stopping at the next floor",
            lift::Press::Reversing if dir > 0 => "turning round — going up",
            lift::Press::Reversing => "turning round — going down",
            lift::Press::Refused if dir > 0 => "top floor",
            lift::Press::Refused => "ground floor",
        };
        press != lift::Press::Refused
    }

    /// The car we are riding, if we are riding one.
    #[inline]
    pub fn lift(&self) -> Option<&Lift> {
        self.world.interior()?.lift.as_ref()
    }

    /// **The nearest building you can go up in**, and which way it lies.
    ///
    /// The city is unbounded, so wandering is not a strategy: a lift you cannot
    /// find is a lift you have not got. This is the answer to "where is one" —
    /// a pointer, not a map and not a teleport. It is the world model's own
    /// answer, asked the same way `--lift` asks it, so what the HUD says and
    /// what is actually there cannot disagree.
    ///
    /// It searches in expanding RINGS and stops at the end of the first ring
    /// that found anything, rather than scanning a square and sorting. Better
    /// than half the tall stock has a lift, so the nearest is usually a few
    /// blocks off and the search is over long before the bound.
    pub fn nearest_lift(&self, radius: i32) -> Option<Wayfind> {
        let (cx, cz) = (self.cam.x.floor() as i32, self.cam.z.floor() as i32);
        let mut best: Option<Wayfind> = None;
        for r in 0..=radius {
            for dz in -r..=r {
                for dx in -r..=r {
                    // The ring, not the square: everything inside it was
                    // covered by a smaller `r`.
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let (x, z) = (cx + dx, cz + dz);
                    let c = self.world.city_cell(x, z);
                    if c.door == 0 || c.door > 4 {
                        continue;
                    }
                    let site = interior::Site {
                        seed: self.world.seed,
                        dx: x,
                        dz: z,
                        face: c.door - 1,
                        plan: c.plan,
                        grain: self.world.grain,
                    };
                    let floors = self.world.storeys(site).len();
                    if floors == 0 {
                        continue;
                    }
                    let dist = ((x - cx) as f32).hypot((z - cz) as f32);
                    if best.is_some_and(|b| b.dist <= dist) {
                        continue;
                    }
                    // The name as it is written on the front of it, so what
                    // the pointer says and what you will read when you get
                    // there are the same words.
                    let bld = palette::building_of(x, z, c.plan, world::BLOCK, self.world.grain);
                    let room = interior::ground_room(
                        x.div_euclid(world::BLOCK),
                        z.div_euclid(world::BLOCK),
                        c.plan,
                        self.world.seed,
                    );
                    let mut name = [b' '; 24];
                    let mut n = 0;
                    for &ch in bld.name[..bld.name_len].iter().chain(b" ").chain(room.word().as_bytes()) {
                        if n < name.len() {
                            name[n] = ch;
                            n += 1;
                        }
                    }
                    best = Some(Wayfind {
                        x,
                        z,
                        dist,
                        floors,
                        // A landmark is shaped so you can see it coming; an
                        // ordinary tower with a lift in it is not, and the
                        // pointer says which so the player knows whether to
                        // look for a shape or for a word.
                        landmark: self.world.city_cell(x, z + 1).arch & world::ARCH_LIFT != 0
                            || self.world.city_cell(x + 1, z).arch & world::ARCH_LIFT != 0
                            || self.world.city_cell(x, z - 1).arch & world::ARCH_LIFT != 0
                            || self.world.city_cell(x - 1, z).arch & world::ARCH_LIFT != 0,
                        name,
                        name_len: n,
                    });
                }
            }
            if best.is_some() {
                break;
            }
        }
        best
    }

    /// **Ride mode.** Put the car we are standing in on a loop end to end, or
    /// take it off one; says whether there was a car to do it to.
    ///
    /// It is the lift's attract mode and it is the same shape `--demo` has: the
    /// world model does the driving, the player keeps the camera the whole
    /// time, and one touch of the panel hands the controls back
    /// (`Lift::call` clears it). No frontend holds any of this state.
    pub fn set_lift_ride(&mut self, on: bool) -> bool {
        let Place::Indoors(r) = &mut self.world.place else { return false };
        let Some(mut l) = r.lift else { return false };
        l.set_shuttle(&r.storeys, on);
        r.lift = Some(l);
        r.retune();
        self.act_note = if on { "ride mode: the car runs on its own" } else { "" };
        true
    }

    /// The room we are in, if we are in one.
    #[inline]
    pub fn room(&self) -> Option<&Interior> {
        self.world.interior()
    }

    /// The nearest thing in the room worth walking up to. Empty outdoors.
    pub fn interaction(&self) -> Option<(&interior::Fixture, f32)> {
        self.world.interior()?.interaction_near(self.cam.x, self.cam.z)
    }

    /// **What the act key would do if you pressed it now** — the verb and the
    /// thing — so every frontend says the same and says it BEFORE the press.
    ///
    /// A lift panel is the one fixture whose meaning depends on more than
    /// which fixture it is: while the car is committed to a journey the button
    /// under your hand is the brake, and the one at the other end turns it
    /// round. Saying "GO UP" at a car that is already going up is how a player
    /// ends up believing they have to hold the key down.
    pub fn act_prompt(&self) -> Option<(&'static str, &'static str)> {
        let r = self.world.interior()?;
        let (f, _) = r.interaction_near(self.cam.x, self.cam.z)?;
        let dir = match f.kind {
            interior::Fitting::CallUp => 1,
            interior::Fitting::CallDown => -1,
            _ => return Some((f.kind.verb(), f.kind.label())),
        };
        Some(match r.lift.map(|l| l.journey()) {
            Some(j) if j == dir => ("STOP", "THE LIFT"),
            Some(j) if j == -dir => ("TURN ROUND", "THE LIFT"),
            _ => (f.kind.verb(), f.kind.label()),
        })
    }

    /// Cast and draw. Split from `step` so a frontend can time the two halves
    /// honestly rather than quoting one number for both.
    pub fn render(&mut self) {
        let t0 = Clock::now();
        self.rays.cast(&self.world, &self.cam, &self.proj);
        self.stats.cast_us = t0.us();
        let t1 = Clock::now();
        self.renderer
            .render(&mut self.grid, &self.world, &self.cam, &self.proj, &self.rays, &self.pop, &self.sky, self.time);
        self.stats.render_us = t1.us();
    }

    /// The packed frame: 4 bytes per cell, `[glyph, r, g, b]`, row-major.
    pub fn frame(&mut self) -> &[u8] {
        self.grid.pack_into(&mut self.frame);
        &self.frame
    }

    /// Step the weather on to the next of clear / rain / downpour, and say
    /// which it landed on.
    pub fn cycle_weather(&mut self) -> &'static str {
        self.sky.cycle(self.cam.x, self.cam.z).name()
    }

    pub fn set_weather(&mut self, w: entities::Weather) {
        self.sky.set(w, self.cam.x, self.cam.z);
    }

    /// Hand the traffic the operator's own list of registrations. With none
    /// given the population already carries a set generated from the seed.
    pub fn set_plates(&mut self, plates: palette::Plates) {
        self.pop.set_plates(plates);
    }

    /// Draw registration plates at all. Text on every visible car is real
    /// per-frame work, so it is a switch.
    pub fn set_plates_on(&mut self, on: bool) {
        self.pop.plates_on = on;
    }

    pub fn weather_name(&self) -> &'static str {
        self.sky.weather.name()
    }

    /// Total visible cells the raycaster kept this frame — a useful sanity
    /// number when judging whether the occlusion cull is doing its job.
    pub fn hit_count(&self) -> usize {
        self.rays.hits.len()
    }
}

// A clock that compiles everywhere. `std::time::Instant` panics on
// wasm32-unknown-unknown, where the host times the two exported calls instead.
struct Clock {
    #[cfg(not(target_arch = "wasm32"))]
    t: std::time::Instant,
}

impl Clock {
    #[inline]
    fn now() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        { Clock { t: std::time::Instant::now() } }
        #[cfg(target_arch = "wasm32")]
        { Clock {} }
    }
    #[inline]
    fn us(&self) -> f32 {
        #[cfg(not(target_arch = "wasm32"))]
        { self.t.elapsed().as_secs_f32() * 1e6 }
        #[cfg(target_arch = "wasm32")]
        { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_row_and_its_inverse_agree() {
        // row_of and height_at are inverses; the inverse carries the OPPOSITE
        // pitch sign, which is the easiest thing in the whole file to get wrong.
        let mut p = Projection::new(180, 80, 1.0, 2.0);
        for &pitch in &[-0.4f32, 0.0, 0.55] {
            p.set_view(pitch, 1.25);
            for &perp in &[3.0f32, 17.0, 96.0] {
                for &y in &[4.0f32, 22.0, 61.0] {
                    let h = p.height_at(y as usize, perp);
                    let back = p.row_of(h, perp);
                    assert!((back - y).abs() < 0.75, "pitch {pitch} perp {perp} y {y} -> {back}");
                }
            }
        }
    }

    #[test]
    fn world_is_deterministic_and_bounded() {
        let w = World::new(7);
        let mut tallest = 0u8;
        let mut built = 0;
        let mut open = 0;
        for z in 4000..4128 {
            for x in 4000..4128 {
                let c = w.cell(x, z);
                assert_eq!(c.height, w.cell(x, z).height, "cell must be a pure function");
                tallest = tallest.max(c.height);
                if c.height > 0 { built += 1 } else { open += 1 }
            }
        }
        assert!(tallest <= world::MAX_HEIGHT, "MAX_HEIGHT must be a true bound");
        assert!(tallest >= 20, "a city with no towers is not this city (got {tallest})");
        // The avenue is half the block on each axis, so a quarter of the world
        // is buildable at most; anything near 0 or near 1 means the layout broke.
        let ratio = built as f32 / (built + open) as f32;
        assert!((0.08..0.30).contains(&ratio), "built ratio {ratio}");
    }

    #[test]
    fn towers_have_profiles_and_the_skyline_is_still_a_mix() {
        use std::collections::{HashMap, HashSet};
        // A building is a height field, so its outline is made out of the cells
        // within its plot. Group the patch back into plots and look at what
        // each one's heights actually are.
        let w = World::new(0xACC17);
        let mut plots: HashMap<(i32, i32, u16), (HashSet<u8>, u8)> = HashMap::new();
        // Whole blocks, so no plot is clipped by the edge of the sample and
        // read as having no middle.
        for z in 4000..4416 {
            for x in 4000..4416 {
                let c = w.cell(x, z);
                if c.height == 0 {
                    continue;
                }
                let key = (x.div_euclid(world::BLOCK), z.div_euclid(world::BLOCK), c.plan);
                let e = plots.entry(key).or_insert((HashSet::new(), c.arch));
                e.0.insert(c.height);
            }
        }
        let tall: Vec<_> = plots
            .values()
            .filter(|(hs, _)| hs.iter().copied().max().unwrap_or(0) >= 20)
            .collect();
        assert!(tall.len() > 60, "only {} tall plots in the patch", tall.len());

        let shaped = tall.iter().filter(|(hs, _)| hs.len() > 1).count();
        let flat = tall.len() - shaped;
        // Most tall towers should now be shaped rather than extruded...
        assert!(
            shaped * 2 > tall.len(),
            "only {shaped} of {} tall towers have any profile at all",
            tall.len()
        );
        // ...and enough must still be flat-topped that the skyline is a mix
        // rather than a field of ziggurats, which was the whole worry.
        assert!(
            flat * 5 > tall.len(),
            "only {flat} of {} tall towers are flat-topped; that is a ziggurat field",
            tall.len()
        );

        // A spire has to be a spire in OUTLINE, not only in texture: every plot
        // textured as one must actually carry a needle above its body.
        let mut spires = 0;
        for (hs, arch) in plots.values() {
            if *arch != 2 {
                continue;
            }
            spires += 1;
            let (lo, hi) = (*hs.iter().min().unwrap(), *hs.iter().max().unwrap());
            assert!(
                hi as i32 - lo as i32 >= 8,
                "a spired plot rises only {} units above its body",
                hi as i32 - lo as i32
            );
        }
        assert!(spires > 0, "no spire anywhere in the patch");
    }

    #[test]
    fn variety_scales_how_much_neighbours_differ() {
        // The default must be the city exactly as it was: a seed picks WHICH
        // mix you get, and until now there was no way to ask for less mixing.
        let a = World::new(0xACC17);
        let b = World::with_variety(0xACC17, 1.0);
        for z in 4000..4120 {
            for x in 4000..4120 {
                let (p, q) = (a.cell(x, z), b.cell(x, z));
                assert_eq!(
                    (p.height, p.hue, p.sat, p.lit, p.win, p.arch, p.plan, p.surface),
                    (q.height, q.hue, q.sat, q.lit, q.win, q.arch, q.plan, q.surface),
                    "full variety changed the city at {x},{z}"
                );
            }
        }

        // How many distinct facade identities a patch of city carries.
        fn styles(w: &World) -> usize {
            let mut set = std::collections::HashSet::new();
            for z in (4000..4400).step_by(2) {
                for x in (4000..4400).step_by(2) {
                    let c = w.cell(x, z);
                    if c.height > 0 {
                        set.insert((c.hue, c.win, c.arch));
                    }
                }
            }
            set.len()
        }
        let varied = styles(&World::with_variety(0xACC17, 1.0));
        let middle = styles(&World::with_variety(0xACC17, 0.5));
        let uniform = styles(&World::with_variety(0xACC17, 0.0));
        assert!(uniform >= 1, "a city with no facades at all is not a city");
        assert!(
            uniform * 4 <= varied,
            "the uniform end must read as a district: {uniform} identities against {varied}"
        );
        assert!(
            (uniform..=varied).contains(&middle),
            "the knob must be a range, not a switch: {uniform} / {middle} / {varied}"
        );

        // And it must leave the HEIGHT mix alone — a district of identical
        // towers is not what this is for.
        let mut heights = std::collections::HashSet::new();
        let w = World::with_variety(0xACC17, 0.0);
        for z in (4000..4400).step_by(2) {
            for x in (4000..4400).step_by(2) {
                let c = w.cell(x, z);
                if c.height > 0 {
                    heights.insert(c.height);
                }
            }
        }
        assert!(heights.len() > 10, "the uniform end flattened the skyline: {} heights", heights.len());
    }

    #[test]
    fn every_entrance_opens_on_to_a_street_and_on_to_a_wall() {
        // A door has to be two things at once or it is not a door: reachable
        // from the pavement, and set into a building rather than into thin air.
        let w = World::new(0xACC17);
        let mut doors = 0;
        let mut faces = [0usize; 4];
        for z in 4000..4256 {
            for x in 4000..4256 {
                let c = w.cell(x, z);
                if c.door == 0 {
                    continue;
                }
                if c.door > 4 {
                    // The wall behind a threshold. Solid, always.
                    assert!(c.height > 0, "the door face at {x},{z} is not a wall");
                    continue;
                }
                doors += 1;
                faces[(c.door - 1) as usize] += 1;
                assert_eq!(c.height, 0, "the threshold at {x},{z} is solid");
                assert_eq!(c.surface, world::surface::THRESHOLD);
                assert_ne!(c.plan, 0, "a threshold must know which building it belongs to");
                let (ix, iz) = interior::INWARD[(c.door - 1) as usize];
                let back = w.cell(x + ix, z + iz);
                assert!(back.height > 0, "the doorway at {x},{z} opens on to open ground");
                assert_eq!(back.door, c.door + 4, "the wall behind the door is not marked");
                // And out the other way: open ground, and a road within the
                // width of one pavement.
                let out = w.cell(x - ix, z - iz);
                assert_eq!(out.height, 0, "the doorway at {x},{z} is walled in from the street");
                let mut road = false;
                for k in 1..8 {
                    if w.cell(x - ix * k, z - iz * k).surface == world::surface::ROADWAY {
                        road = true;
                        break;
                    }
                }
                assert!(road, "the doorway at {x},{z} does not face a street");
            }
        }
        // 8x8 blocks of city. A door on every street-facing building is a lot
        // of doors; none at all would mean `door_slot` never fires.
        assert!(doors > 200, "only {doors} thresholds in 64 blocks of city");
        assert!(faces.iter().all(|&n| n > 0), "entrances only ever face {faces:?}");
    }

    #[test]
    fn walking_into_a_door_puts_you_in_a_room_and_walking_out_takes_you_back() {
        // The whole feature, end to end, driven the way a player drives it: no
        // engine call but `step`, and no key but forward.
        let mut e = Engine::new(120, 40, 1.0, 2.0, 0xACC17);
        let (dx, dz, face) = e
            .world
            .door_near(e.cam.x, e.cam.z, 96)
            .expect("no entrance within 96 cells of the spawn");
        let (ix, iz) = interior::INWARD[face as usize];
        // Stand on the pavement outside it, facing in.
        e.cam.x = (dx - ix) as f32 + 0.5;
        e.cam.z = (dz - iz) as f32 + 0.5;
        e.cam.yaw = match (ix, iz) {
            (0, -1) => 0.0,
            (1, 0) => core::f32::consts::FRAC_PI_2,
            (0, 1) => core::f32::consts::PI,
            _ => -core::f32::consts::FRAC_PI_2,
        };
        e.cam.halt();
        assert!(!e.world.indoors());

        let walk = |e: &mut Engine, n: usize| {
            for _ in 0..n {
                e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            }
        };
        walk(&mut e, 60);
        assert!(e.world.indoors(), "walking into the door did not put us inside");
        let room = e.room().expect("indoors with no room");
        assert!(room.ceiling > 2.5 && room.ceiling < 10.0, "ceiling {}", room.ceiling);
        assert!(room.label_len > 3, "the room has no name");
        assert!(!room.props.is_empty(), "a room with nothing in it at all");
        // We are standing in it, on open floor, under a ceiling.
        assert!(room.open(e.cam.x.floor() as i32, e.cam.z.floor() as i32));

        // Keep walking: the room has walls, so we stop somewhere inside it and
        // do not come out the other side.
        walk(&mut e, 400);
        assert!(e.world.indoors(), "walked straight through the far wall");

        // And it renders as somewhere else. A frame indoors must not be the
        // frame outdoors.
        e.render();
        let inside = e.grid.ch.clone();
        assert!(inside.iter().filter(|&&c| c != b' ').count() > 120 * 40 / 3);

        // Turn round and walk back out.
        e.cam.yaw += core::f32::consts::PI;
        e.cam.halt();
        walk(&mut e, 900);
        assert!(!e.world.indoors(), "could not find the way back out");
        e.render();
        assert_ne!(inside, e.grid.ch, "the street renders identically to the room");
    }

    #[test]
    fn a_building_is_the_same_inside_every_time_and_two_are_not() {
        let w = World::new(1234);
        let (dx, dz, face) = w.door_near(4096.5, 4096.5, 96).expect("no entrance");
        let plan = w.cell(dx, dz).plan;
        let site = Site { seed: w.seed, dx, dz, face, plan, grain: w.grain };
        let a = Interior::build(site, 0, 0.0, w.storeys(site));
        let b = Interior::build(site, 0, 0.0, w.storeys(site));
        assert_eq!(a.room, b.room);
        assert_eq!(a.label_str(), b.label_str());
        assert_eq!((a.wx, a.wz, a.ceiling), (b.wx, b.wz, b.ceiling));
        for z in a.z0..a.z0 + a.wz {
            for x in a.x0..a.x0 + a.wx {
                let (p, q) = (a.at(x, z).unwrap(), b.at(x, z).unwrap());
                assert_eq!((p.height, p.win, p.hue), (q.height, q.win, q.hue), "{x},{z}");
            }
        }
        // Unlimited variety is the point: ours generates its rooms, so a walk
        // down one street should not hand you the same room twice.
        let mut kinds = std::collections::HashSet::new();
        let mut shapes = std::collections::HashSet::new();
        let mut seen = 0;
        for z in (4000..4400).step_by(3) {
            for x in (4000..4400).step_by(3) {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site = Site { seed: w.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: w.grain };
                let r = Interior::build(site, 0, 0.0, w.storeys(site));
                kinds.insert(r.room);
                shapes.insert((r.wx, r.wz, (r.ceiling * 10.0) as i32));
                seen += 1;
            }
        }
        assert!(seen > 12, "only {seen} entrances sampled");
        assert!(kinds.len() >= 5, "only {} kinds of room in a whole district", kinds.len());
        assert!(
            shapes.len() * 4 >= seen * 3,
            "only {} distinct rooms out of {seen} entrances",
            shapes.len()
        );
    }

    #[test]
    fn a_room_reads_as_a_room_not_a_haze() {
        // A floor, a wall and a ceiling that share a hue and sit in the same
        // narrow lightness band read as one dark haze rather than as three
        // surfaces — that was the bug. `floor_hue` is deliberately rotated off
        // `wall_hue`; this is the regression guard that keeps it that way for
        // every family the generator can hand back, not just the one room a
        // screenshot happened to catch.
        let w = World::new(1234);
        let mut seen = 0;
        for z in (4000..4400).step_by(3) {
            for x in (4000..4400).step_by(3) {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site = Site { seed: w.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: w.grain };
                let r = Interior::build(site, 0, 0.0, w.storeys(site));
                let d = (r.floor_hue - r.wall_hue).rem_euclid(360.0);
                let circular = d.min(360.0 - d);
                assert!(
                    circular >= 60.0,
                    "{:?}: floor_hue {} is only {circular} degrees from wall_hue {}",
                    r.room,
                    r.floor_hue,
                    r.wall_hue
                );
                seen += 1;
            }
        }
        assert!(seen > 12, "only {seen} entrances sampled");
    }

    #[test]
    fn a_room_is_not_a_sealed_box_and_the_view_out_moves_with_you() {
        // The captain's bar, and it is a bar rather than a nice-to-have: an
        // interior you cannot see out of is a cupboard. Three things have to
        // hold, and they are three different claims:
        //
        //   1. the room HAS glazing, and it is in the wall that faces the
        //      street;
        //   2. rays leave through it and hit the REAL city at REAL distances —
        //      not a backdrop, not a texture, the same buildings that are there
        //      when you walk back out; and
        //   3. what you see through it SWINGS as you cross the room, because
        //      that is the difference between a window and a picture of one.
        let mut e = Engine::new(160, 50, 1.0, 2.0, 0xACC17);
        let (dx, dz, face) = e.world.door_near(e.cam.x, e.cam.z, 160).expect("no entrance");
        let (ix, iz) = interior::INWARD[face as usize];
        e.cam.x = (dx - ix) as f32 + 0.5;
        e.cam.z = (dz - iz) as f32 + 0.5;
        e.cam.yaw = (ix as f32).atan2(-(iz as f32));
        e.cam.halt();
        for _ in 0..240 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            if e.world.indoors() {
                break;
            }
        }
        assert!(e.world.indoors());
        // Walk in far enough to be looking at the glazing rather than through
        // the door we came in by.
        for _ in 0..150 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
        }
        assert!(e.world.indoors(), "walked out of the far wall");

        // 1. Glazing, in the wall the door is in.
        let (w0, wx, wz, x0, z0, deep) = {
            let r = e.room().unwrap();
            assert!(r.windows.len() >= 3, "only {} glazed bays", r.windows.len());
            for w in &r.windows {
                let c = r.at(w.x, w.z).expect("a window outside its own room");
                assert_eq!(c.win, interior::fit::WINDOW);
                assert!(c.height > 0, "a window you can walk out of is a hole");
                // On the street wall: one step further out is NOT the room.
                assert!(
                    !r.contains(w.x - r.ix, w.z - r.iz),
                    "the bay at {},{} does not face out of the building",
                    w.x,
                    w.z
                );
            }
            (r.windows.len(), r.wx, r.wz, r.x0, r.z0, r.ceiling)
        };
        let _ = (w0, deep);

        // 2. Turn to face the glazing and check the rays get OUT. A hit
        // outside the room's rectangle is a hit on the city, and the only way
        // one can happen is through a bay or the doorway.
        let (dxf, dzf) = (dx as f32 + 0.5, dz as f32 + 0.5);
        let back = 5.0f32.min((wx.min(wz) - 4) as f32);
        e.cam.x = if ix != 0 { dxf + ix as f32 * back } else { x0 as f32 + wx as f32 * 0.5 };
        e.cam.z = if iz != 0 { dzf + iz as f32 * back } else { z0 as f32 + wz as f32 * 0.5 };
        e.cam.yaw += core::f32::consts::PI;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        e.render();

        let outside: Vec<_> = {
            let r = e.room().unwrap();
            e.rays
                .hits
                .iter()
                .filter(|h| !r.contains(h.cell_x, h.cell_z))
                .collect()
        };
        assert!(
            outside.len() > 40,
            "only {} of {} hits got out of the room at all",
            outside.len(),
            e.rays.hits.len()
        );
        // Real buildings at real distances: the city out of the window is the
        // city, so the same cells must be solid when we are standing outside.
        let mut far = 0;
        for h in &outside {
            assert!(
                e.world.city_cell(h.cell_x, h.cell_z).height > 0,
                "a hit at {},{} through the window is not a building outdoors",
                h.cell_x,
                h.cell_z
            );
            if h.dist > 18.0 {
                far += 1;
            }
        }
        assert!(far > 5, "nothing further than the far kerb was visible: {far} hits");

        // 3. Parallax. Step sideways along the glazing and the view through it
        // must change — and change by MORE than the room itself does, which is
        // what tells a window from a painting of one.
        let before_frame = e.grid.ch.clone();
        let seen_before: std::collections::HashSet<(i32, i32)> =
            outside.iter().map(|h| (h.cell_x, h.cell_z)).collect();
        let (rx, rz) = e.cam.right();
        e.cam.x += rx * 3.0;
        e.cam.z += rz * 3.0;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        e.render();
        let seen_after: std::collections::HashSet<(i32, i32)> = {
            let r = e.room().unwrap();
            e.rays
                .hits
                .iter()
                .filter(|h| !r.contains(h.cell_x, h.cell_z))
                .map(|h| (h.cell_x, h.cell_z))
                .collect()
        };
        assert!(
            !seen_after.is_subset(&seen_before) || seen_after != seen_before,
            "three paces along the window and not one new thing came into view"
        );
        let moved = before_frame
            .iter()
            .zip(&e.grid.ch)
            .filter(|(a, b)| a != b)
            .count();
        assert!(moved > 300, "the whole frame only changed in {moved} cells");
    }

    #[test]
    fn any_room_can_be_walked_out_of_without_a_map() {
        // `Interior::way_out` is what anything without a map of the room holds
        // when what it wants is out — the attract mode does exactly this, and
        // an attract mode that walks into a shop and cannot leave is the one
        // failure it must not have. Seventy seconds of `--demo` never wandered
        // through a door, which is why this is asserted here rather than hoped
        // for there.
        //
        // It has already earned its keep twice: steering straight at the
        // doorway wedged behind a rack, and so did steering at it with a
        // shoulder-check. That is what put the flood field in `Interior`.
        let w = World::new(0xACC17);
        let mut checked = 0;
        for z in (4000..4260).step_by(11) {
            for x in (4000..4260).step_by(11) {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let mut e = Engine::new(96, 32, 1.0, 2.0, 0xACC17);
                let (ix, iz) = interior::INWARD[(c.door - 1) as usize];
                e.cam.x = (x - ix) as f32 + 0.5;
                e.cam.z = (z - iz) as f32 + 0.5;
                e.cam.yaw = (ix as f32).atan2(-(iz as f32));
                e.cam.halt();
                for _ in 0..600 {
                    e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
                    if e.world.indoors() {
                        break;
                    }
                }
                if !e.world.indoors() {
                    continue; // this door was not reachable from where we stood
                }
                // Get well into the room first, so leaving is a real journey.
                for _ in 0..260 {
                    e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
                }
                // Now steer at the exit, exactly the way the autopilot does.
                let mut out = false;
                for _ in 0..1800 {
                    let Some(room) = e.room() else {
                        out = true;
                        break;
                    };
                    let (fwd, turn) = room.way_out(e.cam.x, e.cam.z, e.cam.yaw);
                    let keys = if fwd { camera::key::FWD } else { 0 }
                        | match turn {
                            1 => camera::key::TURN_R,
                            -1 => camera::key::TURN_L,
                            _ => 0,
                        };
                    e.step(1.0 / 60.0, keys, 0.0, 0.0);
                }
                assert!(out, "could not walk out of the room at {x},{z}");
                checked += 1;
            }
        }
        assert!(checked > 4, "only {checked} rooms were entered and left");
    }

    #[test]
    fn a_room_is_walkable_from_its_own_door() {
        // A generated room that furnishes its own doorway shut is a room you
        // cannot get into, and it would be a rare seed that did it — so this
        // floods every room in a district rather than one.
        let w = World::new(0xACC17);
        let mut checked = 0;
        for z in (4000..4300).step_by(7) {
            for x in (4000..4300).step_by(7) {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site = Site { seed: w.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: w.grain };
                let r = Interior::build(site, 0, 0.0, w.storeys(site));
                // Flood from the doorway and count what it reaches.
                let mut seen = std::collections::HashSet::new();
                let mut queue = vec![(x, z)];
                seen.insert((x, z));
                let mut open = 0;
                while let Some((cx, cz)) = queue.pop() {
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let n = (cx + dx, cz + dz);
                        if n.0 < r.x0 || n.1 < r.z0 || n.0 >= r.x0 + r.wx || n.1 >= r.z0 + r.wz {
                            continue;
                        }
                        if !r.open(n.0, n.1) || !seen.insert(n) {
                            continue;
                        }
                        queue.push(n);
                    }
                }
                for gz in r.z0..r.z0 + r.wz {
                    for gx in r.x0..r.x0 + r.wx {
                        if r.open(gx, gz) {
                            open += 1;
                        }
                    }
                }
                assert!(
                    seen.len() * 10 >= open * 9,
                    "{}: only {} of {open} open cells are reachable from the door",
                    r.label_str(),
                    seen.len()
                );
                checked += 1;
            }
        }
        assert!(checked > 8, "only {checked} rooms flooded");
    }

    #[test]
    fn colour_never_multiplies_toward_black() {
        // hsl(h, s, base + range*b): at b = 0 the lightness floor must still
        // produce a visible colour, which is why distant towers stay vivid.
        let dark = palette::hsl(210.0, 100.0, 30.0);
        assert!(dark.iter().any(|&c| c > 40), "{dark:?}");
    }

    /// A lift belongs in something with the height to justify it, the same
    /// building always has one or always does not, and the answer is a pure
    /// function of the seed. All three, over a real slab of city.
    #[test]
    fn a_tall_building_has_a_lift_a_short_one_does_not_and_it_never_changes_its_mind() {
        let w = World::new(0xACC17);
        let mut with = 0;
        let mut without = 0;
        let mut shortest_with = u8::MAX;
        for z in 4000..4200 {
            for x in 4000..4200 {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site =
                    Site { seed: w.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: w.grain };
                let st = w.storeys(site);
                // Asked twice, and by a second World on the same seed: the
                // generator decides, not the walk that got here.
                assert_eq!(st.len(), w.storeys(site).len(), "{x},{z} changed its mind");
                assert_eq!(st.len(), World::new(0xACC17).storeys(site).len(), "{x},{z}");
                let (ix, iz) = interior::INWARD[(c.door - 1) as usize];
                let h = w.city_cell(x + ix, z + iz).height;
                if st.is_empty() {
                    without += 1;
                } else {
                    with += 1;
                    shortest_with = shortest_with.min(h);
                    assert!(
                        st.len() >= lift::MIN_FLOORS,
                        "{x},{z} has a lift serving only {} floors",
                        st.len()
                    );
                    // The floors stack: whole-unit slabs, in order, and the top
                    // one fits under the roof.
                    for (i, s) in st.iter().enumerate() {
                        assert_eq!(s.floor, i as i32);
                        assert_eq!(s.base, s.base.round(), "slab {} is not on a whole unit", s.base);
                        assert!(s.ceiling > 2.5, "floor {i} has {} of headroom", s.ceiling);
                        if i > 0 {
                            assert!(s.base >= st[i - 1].top() + lift::SLAB - 0.001, "floor {i} sits in the one below");
                        }
                    }
                    assert!(
                        st.last().unwrap().top() + lift::SLAB <= h as f32 + 0.001,
                        "the top floor is above the roof at {x},{z}"
                    );
                }
            }
        }
        // Not every building, and not no building.
        assert!(with > 40, "only {with} lifts in 200x200 cells of city");
        assert!(without > 40, "only {without} entrances without a lift — a lift in everything");
        assert!(
            shortest_with >= lift::MIN_HEIGHT,
            "a {shortest_with}-unit building was given a lift"
        );
    }

    /// **A shaft is a shaft.** The core has to land on the same world cells on
    /// every storey of a building, or riding one floor would step you sideways
    /// into a wall — which is why the FOOTPRINT half of the generator does not
    /// take the floor number and the CHARACTER half does.
    #[test]
    fn a_lift_shaft_is_in_the_same_place_on_every_floor() {
        let w = World::new(4242);
        let mut checked = 0;
        for z in 4000..4140 {
            for x in 4000..4140 {
                let c = w.cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site =
                    Site { seed: w.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: w.grain };
                let st = w.storeys(site);
                if st.is_empty() {
                    continue;
                }
                checked += 1;
                let ground = Interior::build(site, 0, 0.0, st.clone());
                let g = ground.core.expect("a lift building with no core");
                let mut kinds = std::collections::HashSet::new();
                for s in st.iter() {
                    let r = Interior::build(site, s.floor, s.base, st.clone());
                    let k = r.core.expect("a floor of a lift building with no core");
                    assert_eq!((k.a0, k.door_a, k.in_face), (g.a0, g.door_a, g.in_face));
                    assert_eq!(
                        (r.x0, r.z0, r.wx, r.wz),
                        (ground.x0, ground.z0, ground.wx, ground.wz),
                        "floor {} has a different footprint",
                        s.floor
                    );
                    // The landing is a threshold on every floor, and the core
                    // is solid all round it.
                    for (a, d) in k.landing() {
                        let (gx, gz) = r.point_of(a as f32 + 0.5, d as f32 + 0.5);
                        let cell = r.at(gx.floor() as i32, gz.floor() as i32).unwrap();
                        assert_eq!(cell.height, 0, "the landing on floor {} is solid", s.floor);
                        assert!((9..=12).contains(&cell.door), "the landing on floor {} is not a way through", s.floor);
                    }
                    assert_eq!(r.ceiling, s.ceiling, "the room disagrees with the storey table");
                    assert_eq!(r.base, s.base);
                    kinds.insert(r.room);
                }
                // Same shell, different rooms: a stack of identical floors is
                // not a building either.
                assert!(kinds.len() > 1, "every floor of {x},{z} is the same room");
                if checked > 6 {
                    return;
                }
            }
        }
        assert!(checked > 0, "no lift building found to check");
    }

    /// **The whole feature, end to end, driven the way a player drives it**: no
    /// engine call but `step`, and no key but forward and the act bit.
    ///
    /// It asserts the four things the lift is for. You get in by walking; the
    /// panel takes you up and it takes you down; the car MOVES rather than
    /// teleporting; and the floor you arrive at is the floor the world model
    /// said you were going to.
    #[test]
    fn the_panel_takes_the_car_up_and_down_and_the_car_never_teleports() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        let storeys = e.room().unwrap().storeys.clone();
        assert!(storeys.len() >= lift::MIN_FLOORS);
        assert_eq!(e.lift().unwrap().at, 0, "the car should be where we got in");

        // Stand at the up button. Which button is under your hand is which one
        // you are nearest — the whole of the panel's mechanism.
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        e.cam.x = up_at.0;
        e.cam.z = up_at.1;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        let (f, _) = e.interaction().expect("nothing within reach at the panel");
        assert_eq!(f.kind, interior::Fitting::CallUp, "the near button is not the up one");

        // **Press it ONCE, then take your hands off.** Not one more act bit for
        // the rest of the ride: the whole point of the commitment is that the
        // player is released to look out of the glass, and a ride that needed
        // the key held is the flaw this replaced.
        let mut heights = vec![e.lift().unwrap().y];
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        assert!(e.lift().unwrap().moving(), "pressing the panel did not send the car anywhere");
        assert!(e.lift().unwrap().riding(), "one press did not commit the car to a journey");
        assert_eq!(
            e.lift().unwrap().target,
            storeys.len() - 1,
            "one press should commit the car to the top of the shaft"
        );
        for _ in 0..6000 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
            heights.push(e.lift().unwrap().y);
            if !e.lift().unwrap().moving() {
                break;
            }
        }
        let l = *e.lift().unwrap();
        assert!(!l.moving(), "the car never arrived");
        assert!(!l.riding(), "the car arrived still committed to a journey");
        assert_eq!(l.at, storeys.len() - 1, "one press, hands off, should reach the top floor");
        assert_eq!(l.y, storeys[l.at].base);
        // **No teleport.** The car passed through every height on the way up
        // and never moved more in one frame than a lift could.
        let step_max = heights.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(step_max < lift::SPEED / 30.0, "the car jumped {step_max} units in a frame");
        assert!(heights.len() > 60, "the ride took {} frames — that is a cut", heights.len());
        for want in [0.2f32, 0.5, 0.8] {
            let y = storeys[0].base + (storeys[l.at].base - storeys[0].base) * want;
            assert!(
                heights.windows(2).any(|w| (w[0] - y).signum() != (w[1] - y).signum() || w[0] == y),
                "the car never passed {y}"
            );
        }

        // Hold the button down at the top: it is an EDGE, and there is nowhere
        // above to go, so nothing happens however long it is held.
        for _ in 0..30 {
            e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        }
        assert!(!e.lift().unwrap().moving(), "the up button moved a car already at the top");

        // And down again, from the other end of the car — one press, all the
        // way back to the ground.
        let down_at = e.room().unwrap().point_of(lift::CORE_W as f32 - 1.2, 1.5);
        e.cam.x = down_at.0;
        e.cam.z = down_at.1;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        assert_eq!(
            e.interaction().unwrap().0.kind,
            interior::Fitting::CallDown,
            "the near button at the other end is not the down one"
        );
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        for _ in 0..6000 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
            if !e.lift().unwrap().moving() {
                break;
            }
        }
        assert_eq!(e.lift().unwrap().at, 0, "the down button did not take the car to the ground");
    }

    /// **A ride you cannot cancel is worse than one you have to hold.** The
    /// second press is the brake and the button at the other end is the
    /// reverse, and both leave the car LEVEL with a floor — never stranded
    /// between two, which is the one state you cannot step out of.
    #[test]
    fn a_committed_ride_can_be_stopped_and_can_be_turned_round() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        let storeys = e.room().unwrap().storeys.clone();
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        let down_at = e.room().unwrap().point_of(lift::CORE_W as f32 - 1.2, 1.5);
        let stand = |e: &mut Engine, p: (f32, f32)| {
            e.cam.x = p.0;
            e.cam.z = p.1;
            e.cam.halt();
            e.step(0.0, 0, 0.0, 0.0);
        };
        fn settle(e: &mut Engine) {
            for _ in 0..6000 {
                if !e.lift().unwrap().moving() {
                    return;
                }
                e.step(1.0 / 60.0, 0, 0.0, 0.0);
            }
            panic!("the car never settled");
        }

        // Commit up, let it get properly under way, then press the SAME button.
        stand(&mut e, up_at);
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        for _ in 0..90 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
        }
        let mid = e.lift().unwrap().y;
        assert!(mid > storeys[0].base, "the car had not left the ground to be stopped");
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        assert!(!e.lift().unwrap().riding(), "the second press did not release the commitment");
        let stopping_at = e.lift().unwrap().target;
        assert!(
            storeys[stopping_at].base > mid,
            "the car was asked to stop at a floor it had already gone past"
        );
        assert!(stopping_at < storeys.len() - 1, "stopping carried on to the top anyway");
        settle(&mut e);
        // Level with a real floor, so there is a landing to step out on to.
        let at = e.lift().unwrap().at;
        assert_eq!(at, stopping_at);
        assert_eq!(e.lift().unwrap().y, storeys[at].base);
        assert!(e.lift().unwrap().level(), "the car stopped between floors");

        // Commit up again, then press the OTHER button: it turns round and
        // commits the other way, all the way to the ground.
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        for _ in 0..60 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
        }
        assert_eq!(e.lift().unwrap().journey(), 1);
        let turn_from = e.lift().unwrap().y;
        stand(&mut e, down_at);
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        assert_eq!(e.lift().unwrap().journey(), -1, "the other button did not turn the car round");
        settle(&mut e);
        assert_eq!(e.lift().unwrap().at, 0, "turning round did not take it to the ground");
        assert!(e.lift().unwrap().y < turn_from);
    }

    /// You cannot walk out of a moving lift, and when it stops you can — into
    /// the room on the floor the shaft wall said you were passing.
    #[test]
    fn the_doors_are_shut_while_the_car_is_moving_and_open_on_to_that_floor_when_it_stops() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        let core = e.room().unwrap().core.unwrap();
        let (la, ld) = core.landing()[0];
        let out = interior::INWARD[core.in_face as usize];

        // Send it up — one press, hands off — and try to walk out of it the
        // whole way. Then press again to pull it up at the next floor, which is
        // the landing this walks out on to.
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        e.cam.x = up_at.0;
        e.cam.z = up_at.1;
        e.cam.yaw = (-out.0 as f32).atan2(out.1 as f32);
        e.cam.halt();
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        // Far enough up that floors 1 and 2 are behind us, so the brake lands
        // on a floor with room under it and the walk out is a real storey.
        while e.lift().unwrap().y < e.room().unwrap().storeys[2].base {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
        }
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        let stopping_at = e.lift().unwrap().target;
        assert!(stopping_at >= 3, "the brake picked floor {stopping_at}");
        let mut frames = 0;
        while e.lift().unwrap().moving() {
            // The landing cell is solid while the car is between floors, so
            // walking at it is walking at a wall.
            let (gx, gz) = e.room().unwrap().point_of(la as f32 + 0.5, ld as f32 + 0.5);
            let cell = e.world.cell(gx.floor() as i32, gz.floor() as i32);
            assert!(cell.height > 0, "the doors are open with the car moving");
            assert_eq!(cell.door, 0);
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            assert!(e.lift().is_some(), "walked out of a moving lift");
            frames += 1;
            assert!(frames < 2000, "the car never got there");
        }
        let floor = e.lift().unwrap().at;
        assert_eq!(floor, stopping_at, "the brake did not stop where it said it would");
        let want = e.room().unwrap().storeys[floor];

        // Now it is level: the doors are a threshold again and walking takes
        // you out on to that floor.
        for _ in 0..400 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            if e.lift().is_none() {
                break;
            }
        }
        let room = e.room().expect("stepped out of the lift and out of the world");
        assert!(e.lift().is_none(), "never got out of the car");
        assert_eq!(room.floor, want.floor, "stepped out on to the wrong floor");
        assert_eq!(room.base, want.base, "the floor is not where the shaft said it was");
        assert_eq!(room.ceiling, want.ceiling);
        assert!(
            room.label_str().contains(&format!("FLOOR {stopping_at}")),
            "the room is called {}",
            room.label_str()
        );
        assert!((e.cam.eye - (want.base + camera::EYE_STREET)).abs() < 0.01, "the eye is not on the slab");
        // And the room is a room: floor under us, way out, glazing.
        assert!(room.open(e.cam.x.floor() as i32, e.cam.z.floor() as i32));
        assert!(!room.windows.is_empty());
    }

    /// **What is written on the front of a building is what the room behind
    /// the door is called.** They were two different tables and, while the
    /// fascia carried no readable text, nobody could see that they disagreed:
    /// `ORBIT CLINIC` was painted on the front of `ORBIT GALLERY`. Putting a
    /// legible name on a facade is what made it a bug rather than a curiosity,
    /// and the fix was to delete the second table, not to reconcile them.
    #[test]
    fn the_name_on_the_front_is_the_name_of_the_room_behind_the_door() {
        let mut checked = 0;
        for seed in [0xACC17u32, 23, 90210] {
            let w = World::new(seed);
            for z in 4000..4200 {
                for x in 4000..4200 {
                    let c = w.city_cell(x, z);
                    if c.door == 0 || c.door > 4 {
                        continue;
                    }
                    let site = Site {
                        seed: seed as i32,
                        dx: x,
                        dz: z,
                        face: c.door - 1,
                        plan: c.plan,
                        grain: w.grain,
                    };
                    // What a player reads on the fascia...
                    let bld = palette::building_of(x, z, c.plan, world::BLOCK, w.grain);
                    let outside = format!(
                        "{} {}",
                        core::str::from_utf8(&bld.name[..bld.name_len]).unwrap(),
                        interior::ground_room(
                            x.div_euclid(world::BLOCK),
                            z.div_euclid(world::BLOCK),
                            c.plan,
                            seed as i32,
                        )
                        .word()
                    );
                    // ...and what the room they walk into calls itself.
                    let room = Interior::build(site, 0, 0.0, w.storeys(site));
                    assert_eq!(
                        outside,
                        room.label_str(),
                        "the fascia and the lobby disagree on seed {seed} at {x},{z}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 200, "only {checked} entrances checked");
    }

    /// **The pointer points at something that is really there.** It is the only
    /// answer to "where is a lift" the player has, and the city is unbounded,
    /// so an answer that is merely plausible is worse than none: it sends
    /// somebody on a walk to a building with no way up in it. What it names,
    /// how far it says, and which way it says are all checked against the world
    /// model rather than against itself.
    #[test]
    fn the_nearest_lift_pointer_names_a_building_that_has_one() {
        for seed in [0xACC17u32, 23, 90210, 7] {
            let e = Engine::new(120, 40, 1.0, 2.0, seed);
            let w = e.nearest_lift(200).expect("no lift anywhere near the spawn");
            let c = e.world.city_cell(w.x, w.z);
            assert!((1..=4).contains(&c.door), "the pointer points at something that is not a door");
            let site = Site {
                seed: seed as i32,
                dx: w.x,
                dz: w.z,
                face: c.door - 1,
                plan: c.plan,
                grain: e.world.grain,
            };
            let storeys = e.world.storeys(site);
            assert_eq!(w.floors, storeys.len(), "the pointer miscounted the floors");
            assert!(w.floors >= lift::MIN_FLOORS, "the pointer named a building with no lift");
            // The name it gives is the name written on the building.
            let room = Interior::build(site, 0, 0.0, storeys);
            assert_eq!(w.name(), room.label_str(), "the pointer calls it something else");
            // And nothing nearer has one, which is what "nearest" means.
            let here = (e.cam.x, e.cam.z);
            let r = w.dist.floor() as i32;
            for dz in -r..=r {
                for dx in -r..=r {
                    let (x, z) = (here.0.floor() as i32 + dx, here.1.floor() as i32 + dz);
                    if ((dx * dx + dz * dz) as f32).sqrt() >= w.dist {
                        continue;
                    }
                    let n = e.world.city_cell(x, z);
                    if n.door == 0 || n.door > 4 {
                        continue;
                    }
                    assert!(
                        e.world
                            .storeys(Site {
                                seed: seed as i32,
                                dx: x,
                                dz: z,
                                face: n.door - 1,
                                plan: n.plan,
                                grain: e.world.grain,
                            })
                            .is_empty(),
                        "there is a nearer lift at {x},{z} than the one the pointer named"
                    );
                }
            }
        }
    }

    /// **A building shaped like a landmark always has a lift in it.** That is
    /// the one direction the seventh silhouette has to be exact in: the whole
    /// point of it is that you can pick a lift building out of a skyline before
    /// you are near enough to read anything on it, and a shape that sometimes
    /// lied would be worse than no shape at all.
    ///
    /// The other direction is deliberately NOT asserted. Better than half the
    /// tall stock has a lift — far too many for the shape to mean anything if
    /// they all had it — so a landmark is one in `LANDMARK_ONE_IN` of the
    /// eligible, and the rest are found by the LIFT mark on the facade and by
    /// the `N` pointer.
    ///
    /// It also pins the height gate. `lift::MIN_HEIGHT` is not enough on its
    /// own: a 24-unit building with the tallest lobby a family offers serves
    /// three storeys, and the fewest floors behind a landmark measured here is
    /// exactly `MIN_FLOORS` — so this is sitting right on the edge and will
    /// catch anyone who lowers `LANDMARK_HEIGHT` to meet it.
    #[test]
    fn a_landmark_always_has_a_lift_in_it() {
        for seed in [0xACC17u32, 23, 90210, 1, 7] {
            let w = World::new(seed);
            // (is it landmark-shaped, where is its street door)
            type Plot = (bool, Option<(i32, i32, u8)>);
            let mut plots: std::collections::HashMap<(i32, i32, u16), Plot> = Default::default();
            for bz in 128..146 {
                for bx in 128..146 {
                    for oz in 0..world::BLOCK_BUILT {
                        for ox in 0..world::BLOCK_BUILT {
                            let (x, z) = (bx * world::BLOCK + ox, bz * world::BLOCK + oz);
                            let c = w.city_cell(x, z);
                            if c.plan == 0 {
                                continue;
                            }
                            let e = plots.entry((bx, bz, c.plan)).or_insert((false, None));
                            if (1..=4).contains(&c.door) {
                                e.1 = Some((x, z, c.door - 1));
                            } else if c.height > 0 {
                                e.0 |= c.arch & world::ARCH_LIFT != 0;
                            }
                        }
                    }
                }
            }
            let mut landmarks = 0;
            let mut fewest = usize::MAX;
            for ((_, _, plan), (landmark, door)) in plots {
                if !landmark {
                    continue;
                }
                landmarks += 1;
                let (dx, dz, face) =
                    door.unwrap_or_else(|| panic!("a landmark with no way into it on seed {seed}"));
                let n = w
                    .storeys(Site { seed: seed as i32, dx, dz, face, plan, grain: w.grain })
                    .len();
                assert!(n > 0, "a landmark with no lift in it on seed {seed}");
                fewest = fewest.min(n);
            }
            assert!(landmarks > 60, "only {landmarks} landmarks in the patch on seed {seed}");
            assert_eq!(
                fewest,
                lift::MIN_FLOORS,
                "the height gate has drifted off the edge it is set to on seed {seed}"
            );
        }
    }

    /// **Ride mode runs the car on its own, and gets out of the way.** It goes
    /// end to end, turns round at both ends, stands long enough at each for
    /// someone to walk out, and the moment the player touches the panel it is
    /// off and the car is theirs.
    #[test]
    fn ride_mode_runs_the_car_end_to_end_and_hands_it_back_on_a_press() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        let top = e.room().unwrap().storeys.len() - 1;
        assert!(e.set_lift_ride(true), "nothing to put on a loop");
        assert!(e.lift().unwrap().shuttling());
        assert!(e.lift().unwrap().moving(), "ride mode did not set off");

        // Two full lengths of the shaft, hands off the whole way.
        let mut seen_top = false;
        let mut seen_back = false;
        let mut stood_level = 0;
        for _ in 0..40_000 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
            let l = *e.lift().unwrap();
            if l.level() {
                stood_level += 1;
            }
            if !seen_top && l.level() && l.at == top {
                seen_top = true;
            } else if seen_top && l.level() && l.at == 0 {
                seen_back = true;
                break;
            }
        }
        assert!(seen_top, "the car never got to the top on its own");
        assert!(seen_back, "the car never came back down on its own");
        // It stood still at the ends rather than snapping round, so there was
        // a door to walk out of.
        assert!(
            stood_level as f32 / 60.0 > lift::SHUTTLE_DWELL,
            "the car never stood still long enough to get out of"
        );

        // One press of the panel and it is an ordinary car again.
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        e.cam.x = up_at.0;
        e.cam.z = up_at.1;
        e.cam.halt();
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        assert!(!e.lift().unwrap().shuttling(), "the panel did not take ride mode off");
        for _ in 0..12_000 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
            if !e.lift().unwrap().moving() {
                break;
            }
        }
        assert!(!e.lift().unwrap().moving(), "the car kept going after ride mode was off");
        for _ in 0..600 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
        }
        assert!(!e.lift().unwrap().moving(), "the car set off again on its own");
    }

    /// **An upper floor is a floor, not a ledge.** Walking at the front wall of
    /// a room on the fifth storey used to put you on the STREET — the room
    /// carried the same street doorway on every floor it was built on, and
    /// `Engine::portal` took it at face value however high the room was. The
    /// camera came out over the roadway at the fifth floor's eye height, which
    /// `Camera::airborne` then read as flying, so collision went off with it
    /// and the whole city was walkable from up there.
    ///
    /// A room above the ground has no street door and never had a reason to:
    /// its way out is the lift it arrived by.
    #[test]
    fn an_upper_floor_cannot_be_walked_out_of_into_the_open_air() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        // Up, and out on to a real storey.
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        e.cam.x = up_at.0;
        e.cam.z = up_at.1;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        for _ in 0..6000 {
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
            if !e.lift().unwrap().moving() {
                break;
            }
        }
        let floor = e.lift().unwrap().at;
        assert!(floor > 0, "the car never left the ground floor");
        let out = interior::INWARD[e.room().unwrap().core.unwrap().in_face as usize];
        e.cam.yaw = (-out.0 as f32).atan2(out.1 as f32);
        e.cam.halt();
        for _ in 0..400 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            if e.lift().is_none() {
                break;
            }
        }
        let room = e.room().expect("never got out of the car");
        assert!(e.lift().is_none());
        assert!(room.floor > 0, "this is meant to be an upper floor");
        let slab = room.base;
        assert!(slab > 4.0, "the floor is only {slab} units up");

        // Now walk off in every direction from where the lift left us.
        // Whatever happens, the answer is never "you are outside, in the air".
        let (mid_x, mid_z) = (e.cam.x, e.cam.z);
        let (site, floor_no, base, storeys) =
            (room.site, room.floor, room.base, room.storeys.clone());
        for k in 0..16 {
            let mut e2 = Engine::new(160, 50, 1.0, 2.0, 0xACC17);
            e2.world.place =
                Place::Indoors(Box::new(Interior::build(site, floor_no, base, storeys.clone())));
            e2.cam.ground = slab;
            e2.cam.eye_target = slab + camera::EYE_STREET;
            e2.cam.eye = e2.cam.eye_target;
            e2.cam.x = mid_x;
            e2.cam.z = mid_z;
            e2.cam.yaw = k as f32 * core::f32::consts::TAU / 16.0;
            e2.cam.halt();
            for _ in 0..900 {
                e2.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
                assert!(
                    e2.world.indoors() || e2.cam.eye <= camera::EYE_STREET + 1.0,
                    "walked off floor {floor_no} into the open air at {:.1},{:.1}, eye {:.1}",
                    e2.cam.x,
                    e2.cam.z,
                    e2.cam.eye
                );
                assert!(
                    !e2.cam.airborne(),
                    "collision went off on floor {floor_no} at {:.1},{:.1}",
                    e2.cam.x,
                    e2.cam.z
                );
            }
        }
    }

    /// **Glass on two sides, and they show two different things.** One side of
    /// the car looks out at the CITY, at real distances, through the same DDA
    /// that finds a wall — the same fall-through a room's window uses. The
    /// other looks at the shaft, and everything a ray meets that way is inside
    /// the core.
    #[test]
    fn the_car_is_glazed_on_two_sides_and_one_of_them_is_the_street() {
        let (mut e, _) = walk_into_a_lift(0xACC17);
        // Take it up, so the street is genuinely below us and there is shaft
        // above and below the car.
        // At the UP button — which button is under your hand is which one you
        // are nearest, so a test that presses from wherever it happens to be
        // standing is a test that can silently press the wrong one.
        let up_at = e.room().unwrap().point_of(1.2, 1.5);
        e.cam.x = up_at.0;
        e.cam.z = up_at.1;
        e.cam.halt();
        e.step(0.0, 0, 0.0, 0.0);
        assert_eq!(e.interaction().unwrap().0.kind, interior::Fitting::CallUp);
        // One press takes it to the top of the shaft on its own.
        e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
        for _ in 0..6000 {
            if !e.lift().unwrap().moving() {
                break;
            }
            e.step(1.0 / 60.0, 0, 0.0, 0.0);
        }
        let up = e.lift().unwrap().y;
        assert!(up > 12.0, "the car only got to {up} units");

        let (glass, shaft, mid) = {
            let car = e.room().unwrap();
            (car.point_of(3.5, -6.0), car.point_of(3.5, 14.0), car.point_of(3.5, 1.5))
        };
        let look = |e: &mut Engine, at: (f32, f32)| {
            e.cam.x = mid.0;
            e.cam.z = mid.1;
            e.cam.yaw = (at.0 - mid.0).atan2(-(at.1 - mid.1));
            e.cam.halt();
            e.step(0.0, 0, 0.0, 0.0);
            e.render();
        };

        // Outward: the ray leaves the car entirely and lands on real city, far
        // away. Not a backdrop — the same cells that are there on the street.
        look(&mut e, glass);
        let mut out_of_the_room = 0;
        let mut furthest = 0.0f32;
        for x in 0..e.proj.cols {
            for h in e.rays.column(x) {
                if !e.room().unwrap().contains(h.cell_x, h.cell_z) {
                    out_of_the_room += 1;
                    furthest = furthest.max(h.dist);
                    assert_eq!(
                        e.world.city_cell(h.cell_x, h.cell_z).height,
                        h.height,
                        "what is past the glass is not the city"
                    );
                }
            }
        }
        assert!(out_of_the_room > 40, "only {out_of_the_room} rays got out of the car");
        assert!(furthest > 24.0, "the furthest thing out of the window is {furthest} units off");
        let street = e.grid.ch.clone();

        // Inward: every hit is inside the core, and the wall at the back of it
        // is a shaft wall — which is what the storeys are drawn on.
        look(&mut e, shaft);
        let mut shaft_hits = 0;
        for x in 0..e.proj.cols {
            for h in e.rays.column(x) {
                let car = e.room().unwrap();
                assert!(
                    car.contains(h.cell_x, h.cell_z),
                    "a ray into the shaft left the core at {},{}",
                    h.cell_x,
                    h.cell_z
                );
                if car.at(h.cell_x, h.cell_z).unwrap().win == interior::fit::SHAFT {
                    shaft_hits += 1;
                }
            }
        }
        assert!(shaft_hits > 20, "only {shaft_hits} rays reached a shaft wall");
        assert_ne!(street, e.grid.ch, "the shaft renders identically to the street");

        // The shaft is open above and below the car: the floors of the building
        // are on it, at their own heights, whether they are over the car or
        // under it.
        let car = e.room().unwrap();
        assert!(car.storey_at(up + 6.0).is_some(), "nothing above the car");
        assert!(car.storey_at(up - 6.0).is_some(), "nothing below the car");
        let (s, rel) = car.storey_at(up).unwrap();
        assert_eq!(s.base, up, "the car is not level with the floor it stopped at");
        assert_eq!(rel, 0.0);
    }

    /// Walk in off the street and into the car, with the keys a player uses.
    /// Shared by the lift tests; the two camera placements it makes are the
    /// same ones `--bench --indoors` already makes to get through a door.
    fn walk_into_a_lift(seed: u32) -> (Engine, (i32, i32)) {
        let mut e = Engine::new(160, 50, 1.0, 2.0, seed);
        let (cx, cz) = (e.cam.x.floor() as i32, e.cam.z.floor() as i32);
        let mut best: Option<(usize, i32, i32, u8)> = None;
        for z in cz - 110..=cz + 110 {
            for x in cx - 110..=cx + 110 {
                let c = e.world.city_cell(x, z);
                if c.door == 0 || c.door > 4 {
                    continue;
                }
                let site =
                    Site { seed: e.world.seed, dx: x, dz: z, face: c.door - 1, plan: c.plan, grain: e.world.grain };
                let n = e.world.storeys(site).len();
                if n > 0 && best.is_none_or(|(m, ..)| n > m) {
                    best = Some((n, x, z, c.door - 1));
                }
            }
        }
        let (_, dx, dz, face) = best.expect("no lift within 110 cells of the spawn");
        let (ix, iz) = interior::INWARD[face as usize];
        e.cam.x = (dx - ix) as f32 + 0.5;
        e.cam.z = (dz - iz) as f32 + 0.5;
        e.cam.yaw = (ix as f32).atan2(-(iz as f32));
        e.cam.halt();
        for _ in 0..400 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            if e.world.indoors() {
                break;
            }
        }
        for _ in 0..60 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
        }
        assert!(e.world.indoors(), "never got in off the street");
        let core = e.room().unwrap().core.expect("a lift building with no core");
        let (la, ld) = core.landing()[0];
        let plus_a = core.door_a == core.a0;
        let at = e.room().unwrap().point_of(la as f32 + if plus_a { -0.6 } else { 1.6 }, ld as f32 + 0.5);
        let inward = interior::INWARD[core.in_face as usize];
        e.cam.x = at.0;
        e.cam.z = at.1;
        e.cam.yaw = (inward.0 as f32).atan2(-(inward.1 as f32));
        e.cam.halt();
        for _ in 0..300 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            if e.lift().is_some() {
                break;
            }
        }
        assert!(e.lift().is_some(), "walked at the landing and never got into the car");
        (e, (dx, dz))
    }

    /// **The floor number on the shaft wall reads left to right, not
    /// mirrored, whichever of the four ways the building's entrance faces.**
    /// `hit.along` — the raw world coordinate the DDA hits the shaft's back
    /// wall at — runs the SAME way the screen does for two of the four
    /// possible entrance orientations and the OPPOSITE way for the other
    /// two (measured directly off `Rays::column`: `along` decreases with
    /// screen column when `room.ix - room.iz < 0`, increases otherwise). A
    /// floor number keyed straight off it without correcting for that came
    /// out mirrored on exactly those two orientations — provable on floor 1
    /// alone, because `glyph3x5('1')` is a single stroke on the RIGHT of its
    /// own field, not centred, so a mirror puts it on the LEFT instead.
    ///
    /// One seed of each `(room.ix, room.iz)` rides from the ground floor,
    /// through the real raycaster and projector, and the lit sign cells are
    /// read back off the real frame buffer: floor 0's box (`glyph3x5('0')`,
    /// symmetric — it only proves the sign is framed and centred at all)
    /// gives the sign's own on-screen span, and floor 1's stroke must fall
    /// right of that span's centre, never left.
    #[test]
    fn a_floor_number_on_the_shaft_wall_reads_left_to_right_in_every_orientation() {
        // (seed, expected room.ix, expected room.iz) — one of each of the
        // four cardinal entrance directions, found by inspection.
        let cases = [(703703u32, 0, 1), (22u32, 1, 0), (456u32, 0, -1), (4242u32, -1, 0)];
        for (seed, want_ix, want_iz) in cases {
            let (mut e, _) = walk_into_a_lift(seed);
            let car_in = e.room().unwrap().point_of(2.5, 6.0);
            let up_at = e.room().unwrap().point_of(1.2, 1.5);
            assert_eq!(
                (e.room().unwrap().ix, e.room().unwrap().iz),
                (want_ix, want_iz),
                "seed {seed}: unexpected orientation"
            );
            let face_shaft = |e: &mut Engine| {
                let p = e.room().unwrap().point_of(1.35, 1.25);
                e.cam.x = p.0;
                e.cam.z = p.1;
                e.cam.yaw = (car_in.0 - e.cam.x).atan2(-(car_in.1 - e.cam.z));
                e.cam.pitch = -0.26;
                e.cam.halt();
                e.step(0.0, 0, 0.0, 0.0);
            };
            // The sign's own fixed lit colour, hsl(44, 96, 66) — nothing
            // else on this wall is this bright and this saturated.
            let lit_span = |e: &mut Engine| -> Option<(usize, usize)> {
                e.render();
                let (cols, rows) = (e.grid.cols, e.grid.rows);
                let mut xs = vec![];
                for y in 0..rows {
                    for x in 0..cols {
                        let i = y * cols + x;
                        let (r, g, b) =
                            (e.grid.rgb[i * 3], e.grid.rgb[i * 3 + 1], e.grid.rgb[i * 3 + 2]);
                        if r > 230 && (180..230).contains(&g) && (60..110).contains(&b) {
                            xs.push(x);
                        }
                    }
                }
                (!xs.is_empty()).then(|| (*xs.iter().min().unwrap(), *xs.iter().max().unwrap()))
            };

            // Floor 0, at the ground: the box, and the sign's own reference span.
            e.cam.x = up_at.0;
            e.cam.z = up_at.1;
            face_shaft(&mut e);
            let (box_x0, box_x1) =
                lit_span(&mut e).unwrap_or_else(|| panic!("seed {seed}: no lit sign at floor 0"));

            // Ride up. Floor 1's sign is a single stroke, off to one side.
            e.step(1.0 / 60.0, camera::key::ACT, 0.0, 0.0);
            let storeys = e.room().unwrap().storeys.clone();
            let mut caught = None;
            for _ in 0..600 {
                let Some(l) = e.lift().copied() else { break };
                if l.passing(&storeys) >= 1 {
                    face_shaft(&mut e);
                    caught = lit_span(&mut e);
                    break;
                }
                e.step(1.0 / 60.0, 0, 0.0, 0.0);
            }
            let (s_x0, s_x1) =
                caught.unwrap_or_else(|| panic!("seed {seed}: floor 1's sign never appeared"));
            let mid = (s_x0 + s_x1) as f32 / 2.0;
            let box_mid = (box_x0 + box_x1) as f32 / 2.0;
            assert!(
                mid > box_mid,
                "seed {seed} (ix={want_ix},iz={want_iz}): floor 1's stroke centred at {mid:.1} \
                 is not right of the sign's own centre {box_mid:.1} (box span {box_x0}..{box_x1}, \
                 stroke span {s_x0}..{s_x1}) — the sign is mirrored"
            );
        }
    }

    #[test]
    fn a_frame_renders_and_is_not_empty() {
        let mut e = Engine::new(180, 80, 1.0, 2.0, 0xACC1);
        e.step(1.0 / 60.0, 0, 0.0, 0.0);
        e.render();
        let n = e.frame().len();
        assert_eq!(n, 180 * 80 * 4);
        let painted = e.grid.ch.iter().filter(|&&c| c != b' ').count();
        assert!(painted > 180 * 80 / 4, "only {painted} cells painted");
    }

    #[test]
    fn rain_falls_and_is_drawn_in_front_of_the_city() {
        let mut e = Engine::new(180, 80, 1.0, 2.0, 0xACC1);
        e.set_weather(entities::Weather::Rain);
        assert!(!e.sky.drops.is_empty(), "asking for rain must produce drops");
        // Every drop must be somewhere it could fall FROM.
        assert!(e.sky.drops.iter().all(|d| d.y > 0.0 && d.speed > 0.0));

        // It falls. Not "the state changed" — the drops are lower than they
        // were, and they are lower by about the distance they should have
        // fallen.
        let before: Vec<(f32, f32)> = e.sky.drops.iter().map(|d| (d.y, d.speed)).collect();
        e.step(0.05, 0, 0.0, 0.0);
        let fell = e
            .sky
            .drops
            .iter()
            .zip(&before)
            .filter(|(now, was)| (was.0 - now.y - was.1 * 0.05).abs() < 1e-3)
            .count();
        assert!(
            fell > before.len() * 8 / 10,
            "only {fell} of {} drops fell by speed x dt",
            before.len()
        );

        // And it lands ON the frame, in front of the buildings rather than
        // behind them: turning the rain on must paint cells the dry frame did
        // not have.
        e.render();
        let wet = e.grid.ch.clone();
        e.set_weather(entities::Weather::Clear);
        e.render();
        let dry = e.grid.ch.clone();
        let changed = wet.iter().zip(&dry).filter(|(a, b)| a != b).count();
        assert!(changed > 20, "rain changed only {changed} cells of the frame");
    }

    #[test]
    fn a_plate_on_screen_is_never_a_registration_other_than_its_own() {
        // The one failure that would be worse than having no plates: a plate
        // that reads as a DIFFERENT registration to the one the car carries.
        // Every plate on the frame is therefore either EMPTY — the honest
        // middle-distance smudge — or exactly one whole registration from the
        // list, inside its own body characters. Nothing in between, ever.
        //
        // This is checked against the frame the viewer actually sees, not
        // against the drawing code, because the two ways it broke in practice
        // were both occlusion: a building corner and a car passing in front.
        let wanted = ["AB12 CDE", "K9 PAW", "1 RG", "BOSS 1", "XY24 ZZT"];
        let (plates, dropped) = palette::Plates::from_list(&wanted);
        assert_eq!(dropped, 0);
        // Every way a registration from the list is allowed to appear: the
        // characters are SET across the plate, so `AB12 CDE` may read
        // `AB12   CDE` or `A B 1 2   C D E` as well. Nothing else counts — a
        // fourth spacing, or any pitch that is not even, would be a plate
        // reading as something other than the registration it carries.
        let mut legal: Vec<String> = Vec::new();
        for w in &wanted {
            let p = palette::Plate::parse(w).unwrap();
            let mut buf = [b' '; palette::PLATE_SET_MAX];
            for s in 0..palette::PLATE_SETTINGS {
                let n = p.set_into(s, &mut buf);
                legal.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        }
        // The characters a plate's own body is drawn with. A run made only of
        // these is a rule, or an empty plate; anything else is carrying a
        // registration and has to be one of `legal`.
        let body = [
            palette::PLATE_RULE,
            palette::PLATE_CORNER,
            palette::PLATE_UPRIGHT,
            palette::PLATE_CAP_L,
            palette::PLATE_CAP_R,
        ];
        let mut e = Engine::new(180, 60, 1.0, 2.0, 0xACC17);
        e.set_plates(plates);
        e.set_weather(entities::Weather::Downpour);

        let mut seen_text = 0;
        let mut seen_empty = 0;
        for i in 0..900 {
            let keys = if (i / 90) % 2 == 0 { camera::key::FWD } else { 0 };
            e.step(1.0 / 60.0, keys, 0.0, 0.0);
            e.render();
            // **Nothing about a plate paints a background any more.** The whole
            // point of drawing it out of characters is that it is made of the
            // same stuff as the rest of the picture.
            assert!(!e.grid.has_panels, "frame {i}: a plate painted a background");
            for y in 0..e.grid.rows {
                let mut x = 0;
                while x < e.grid.cols {
                    if !e.grid.is_plate(x as i32, y as i32) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    let mut run = String::new();
                    while x < e.grid.cols && e.grid.is_plate(x as i32, y as i32) {
                        run.push(e.grid.ch[y * e.grid.cols + x] as char);
                        x += 1;
                    }
                    let bytes = run.as_bytes();
                    // A plate's own BODY is always a glyph — a blank there
                    // would be a hole in the object. Inside it, a blank is a
                    // blank of the registration: `1 RG` has one, and the
                    // three settings open more.
                    assert!(
                        bytes[0] != b' ' && bytes[bytes.len() - 1] != b' ',
                        "frame {i} row {y} col {start}: plate {run:?} is open at an end"
                    );
                    if bytes.iter().all(|c| body.contains(c)) {
                        seen_empty += 1;
                        continue;
                    }
                    seen_text += 1;
                    // A registration row: an upright at each end and the whole
                    // registration between them, and NOTHING wider. The plate
                    // is sized to the setting, so there is no slack in it for a
                    // registration to float in.
                    assert!(
                        bytes.len() > 2
                            && body.contains(&bytes[0])
                            && body.contains(&bytes[bytes.len() - 1]),
                        "frame {i} row {y} col {start}: plate {run:?} is not closed at both ends"
                    );
                    let inner = &run[1..run.len() - 1];
                    assert!(
                        legal.iter().any(|w| w == inner),
                        "frame {i} row {y} col {start}: plate reads {run:?}, which is not one \
                         whole registration from the list"
                    );
                }
            }
        }
        assert!(seen_text > 0, "no registration was ever drawn as text over 900 frames");
        assert!(seen_empty > 0, "the middle-distance plate never appeared");
    }

    #[test]
    fn the_terminal_draws_a_plate_out_of_characters_and_paints_nothing() {
        // The captain plays in a terminal and looks at the pictures, so both
        // output paths have to carry the plate. The SVG one is judged by
        // looking at `docs/plates.png`; this is the other one, and a terminal
        // is not something a test can photograph — so read the bytes back.
        //
        // What it has to prove is the whole of the change: a plate is built out
        // of characters and paints NOTHING. Every other thing in this city is a
        // coloured glyph on black, and the plate used to be the one painted
        // rectangle in the middle of it, which is what made it look pasted on
        // rather than drawn.
        let wanted = ["AB12 CDE", "K9 PAW", "BOSS 1"];
        let (plates, dropped) = palette::Plates::from_list(&wanted);
        assert_eq!(dropped, 0);
        let mut e = Engine::new(180, 60, 1.0, 2.0, 0xACC17);
        e.set_plates(plates);
        let mut ansi = String::new();
        let mut checked = 0;
        for i in 0..600 {
            e.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            e.render();
            if !e.grid.has_plates {
                continue;
            }
            output::grid_to_ansi(&e.grid, &mut ansi);
            // Not one background sequence anywhere on the frame, and not one
            // bold: both came off the panel a plate used to paint.
            assert!(
                !ansi.contains("\x1b[48;2"),
                "frame {i}: the terminal painted a background"
            );
            assert!(!ansi.contains("\x1b[1m"), "frame {i}: something went bold");
            // And the plate really is on the frame, as characters, in the
            // plate's own colours: its body in the plate colour and the
            // registration in ink.
            let mut body_cells = 0;
            let mut ink_cells = 0;
            for y in 0..e.grid.rows {
                for x in 0..e.grid.cols {
                    if !e.grid.is_plate(x as i32, y as i32) {
                        continue;
                    }
                    let j = y * e.grid.cols + x;
                    // Ink is near-neutral; the body is the plate's own hue. One
                    // channel apart is enough to tell them apart and does not
                    // care what the distance dim did to the brightness.
                    let (r, g, b) =
                        (e.grid.rgb[j * 3], e.grid.rgb[j * 3 + 1], e.grid.rgb[j * 3 + 2]);
                    if r.abs_diff(b) < 12 {
                        ink_cells += 1;
                    } else {
                        body_cells += 1;
                    }
                    let _ = g;
                }
            }
            assert!(body_cells > 0, "frame {i}: a plate with no body characters");
            if ink_cells > 0 {
                checked += 1;
            }
            if checked == 12 {
                return;
            }
        }
        panic!("no frame in the walk ever put a registration on screen as characters");
    }

    #[test]
    fn a_vehicle_keeps_its_plate_and_the_seed_decides_it() {
        // Same seed, same cars, same registrations — and a car does not swap
        // plate while it is on the road.
        let mut a = Engine::new(180, 60, 1.0, 2.0, 4242);
        let mut b = Engine::new(180, 60, 1.0, 2.0, 4242);
        let before: Vec<u16> = a.pop.vehs.iter().map(|v| v.plate).collect();
        assert_eq!(before, b.pop.vehs.iter().map(|v| v.plate).collect::<Vec<_>>());

        let mut held = 0;
        for _ in 0..30 {
            let was: Vec<(f32, f32, u16)> =
                a.pop.vehs.iter().map(|v| (v.x, v.z, v.plate)).collect();
            a.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            for (v, w) in a.pop.vehs.iter().zip(&was) {
                // Still the same car — it moved on from where it was rather
                // than being teleported by a recycle.
                if (v.x - w.0).hypot(v.z - w.1) < 1.0 {
                    assert_eq!(v.plate, w.2, "a car changed plate mid-street");
                    held += 1;
                }
            }
            b.step(1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
        }
        assert!(held > 2000, "only {held} car-frames were actually continuous");
        assert_eq!(
            a.pop.vehs.iter().map(|v| v.plate).collect::<Vec<_>>(),
            b.pop.vehs.iter().map(|v| v.plate).collect::<Vec<_>>(),
            "the same seed must hand the same cars the same plates"
        );

        // A different seed must not hand out the same set.
        let c = Engine::new(180, 60, 1.0, 2.0, 99);
        assert_ne!(before, c.pop.vehs.iter().map(|v| v.plate).collect::<Vec<_>>());
    }

    #[test]
    fn street_furniture_stands_on_pavement_and_nowhere_else() {
        let w = World::new(0xACC17);
        let mut props = Vec::new();
        w.props_near(4096.5, 4096.5, 90.0, &mut props);
        assert!(props.len() > 30, "only {} props within 90 units", props.len());

        let mut kinds = [0usize; 3];
        for p in &props {
            let c = w.cell(p.x.floor() as i32, p.z.floor() as i32);
            assert_eq!(c.height, 0, "a {:?} is standing inside a building", p.kind);
            assert_ne!(
                c.surface,
                world::surface::ROADWAY,
                "a {:?} is standing in the carriageway",
                p.kind
            );
            // Furniture belongs on the footway, never out past the kerb: the
            // roadway runs from cross 5 to 10.
            assert!(
                c.cross < 5 || c.cross > 10,
                "a {:?} is at cross offset {}, which is in the road",
                p.kind,
                c.cross
            );
            assert!(p.height > 0.0);
            kinds[match p.kind {
                world::Prop::Lamp => 0,
                world::Prop::Tree => 1,
                world::Prop::Planter => 2,
            }] += 1;
        }
        assert!(kinds.iter().all(|&n| n > 0), "expected all three kinds, got {kinds:?}");

        // Placement is a pure function of position, like every other feature of
        // this city — walk away and come back and the lamps are where you left
        // them.
        let mut again = Vec::new();
        w.props_near(4096.5, 4096.5, 90.0, &mut again);
        assert_eq!(props.len(), again.len());
        assert!(props
            .iter()
            .zip(&again)
            .all(|(a, b)| a.x == b.x && a.z == b.z && a.kind == b.kind));
    }

    #[test]
    fn the_glide_carries_movement_and_then_gives_it_back() {
        // The fallback path's whole trick: input that stops arriving keeps
        // commanding the camera for `glide` seconds and then lets go. Zero
        // glide must behave exactly as before it existed.
        let w = World::new(5);
        for (glide, want_carry) in [(0.0f32, false), (0.35, true)] {
            let mut cam = Camera::new(4096.5, 4096.5, 0.0);
            cam.glide = glide;
            for _ in 0..30 {
                cam.update(&w, 1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
            }
            let moving = (cam.x, cam.z);
            // Input goes silent — as it does for the whole autorepeat delay
            // with a finger still on the key. Coast until it comes to rest.
            let mut frames = 0;
            loop {
                let was = (cam.x, cam.z);
                cam.update(&w, 1.0 / 60.0, 0, 0.0, 0.0);
                frames += 1;
                if (cam.x - was.0).abs() < 1e-6 && (cam.z - was.1).abs() < 1e-6 {
                    break;
                }
                assert!(frames < 600, "the camera must actually stop");
            }
            let coast = (cam.x - moving.0).hypot(cam.z - moving.1);
            // With no glide the only tail is the velocity's own easing, which
            // is one DECEL_TAU of travel — about a fifth of a cell, there to
            // stop movement snapping on and off, not to carry anything.
            let easing = camera::WALK_SPEED * 0.06;
            if want_carry {
                assert!(
                    coast > 3.0 * easing,
                    "glide {glide} coasted {coast:.2} units, barely more than the easing alone"
                );
            } else {
                assert!(
                    coast < 1.5 * easing,
                    "no glide must mean no carry beyond the easing, got {coast:.2}"
                );
            }
        }
    }

    #[test]
    fn walking_into_a_wall_does_not_pass_through_it() {
        let w = World::new(3);
        // Find a solid cell and stand right next to it.
        let mut cam = None;
        'outer: for z in 4000..4064 {
            for x in 4000..4064 {
                if w.solid(x, z) && !w.solid(x - 1, z) {
                    cam = Some(Camera::new(x as f32 - 0.5, z as f32 + 0.5,
                                           core::f32::consts::FRAC_PI_2));
                    break 'outer;
                }
            }
        }
        let mut cam = cam.expect("no wall found");
        for _ in 0..240 {
            cam.update(&w, 1.0 / 60.0, camera::key::FWD, 0.0, 0.0);
        }
        assert!(!w.solid(cam.x.floor() as i32, cam.z.floor() as i32),
                "ended up inside a building at {},{}", cam.x, cam.z);
    }
}

