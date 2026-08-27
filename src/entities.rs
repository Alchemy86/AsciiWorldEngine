//! The population: people on the pavement, traffic on the carriageway — and
//! the weather, which is the same idea pointed at the sky.
//!
//! Entities live in a moving disc around the camera. One that walks out of it
//! is recycled to a fresh spot on a suitable surface rather than destroyed, so
//! the count is constant and there is no allocation after startup.

use crate::palette::Plates;
use crate::rng::Rng;
use crate::world::{surface, World};

pub const PED_COUNT: usize = 150;
pub const VEH_COUNT: usize = 110;
/// Radius the population is kept within. Beyond this nothing is drawn anyway.
const LIVE_RADIUS: f32 = 115.0;

#[derive(Clone, Copy, Default)]
pub struct Actor {
    pub x: f32,
    pub z: f32,
    pub hx: f32,
    pub hz: f32,
    pub speed: f32,
    pub phase: f32,
    pub hue: u16,
    /// Which registration this vehicle carries, as a raw key resolved against
    /// the plate list at draw time (`Plates::get`). Drawn once, at spawn, from
    /// the seeded generator: a vehicle therefore keeps its plate for as long as
    /// it is on screen, and the same seed hands the same cars the same plates.
    /// Holding the key rather than an index means swapping the list in does not
    /// need every vehicle re-rolled.
    pub plate: u16,
}

pub struct Population {
    pub peds: Vec<Actor>,
    pub vehs: Vec<Actor>,
    /// The registrations on the road. Supplied by the operator or generated
    /// from the seed; see `palette::Plates`.
    pub plates: Plates,
    /// Whether plates are drawn at all. Text on every visible car is real
    /// per-frame work, so it is a switch rather than something every run pays
    /// for unasked — `--no-plates` turns it off.
    pub plates_on: bool,
    rng: Rng,
}

impl Population {
    pub fn new(world: &World, cx: f32, cz: f32, seed: u64) -> Self {
        let mut p = Population {
            peds: vec![Actor::default(); PED_COUNT],
            vehs: vec![Actor::default(); VEH_COUNT],
            plates: Plates::from_seed(seed, 128),
            plates_on: true,
            rng: Rng::new(seed ^ 0xC0FFEE),
        };
        for i in 0..PED_COUNT {
            p.peds[i] = p.spawn_one(world, cx, cz, false);
        }
        for i in 0..VEH_COUNT {
            p.vehs[i] = p.spawn_one(world, cx, cz, true);
        }
        p
    }

    fn spawn_one(&mut self, world: &World, cx: f32, cz: f32, vehicle: bool) -> Actor {
        for _ in 0..48 {
            let a = self.rng.f32() * core::f32::consts::TAU;
            // sqrt keeps the disc uniformly filled instead of clustering at the
            // centre, which is what makes traffic thin out with distance.
            let r = 12.0 + (LIVE_RADIUS - 14.0) * self.rng.f32().sqrt();
            let x = cx + r * a.cos();
            let z = cz + r * a.sin();
            let c = world.cell(x.floor() as i32, z.floor() as i32);
            if c.height != 0 {
                continue;
            }
            let ok = if vehicle {
                c.surface == surface::ROADWAY
            } else {
                c.surface == surface::PAVEMENT || c.surface == surface::PAINTED
            };
            if !ok {
                continue;
            }
            // Head along the street this cell belongs to. Which axis is the
            // street's is read straight off the block layout.
            let along_x = crate::world::BLOCK_BUILT <= z.floor() as i32 % crate::world::BLOCK
                || (z.floor() as i32).rem_euclid(crate::world::BLOCK) >= crate::world::BLOCK_BUILT;
            let dir = if self.rng.next_u32() & 1 == 0 { 1.0 } else { -1.0 };
            let (hx, hz) = if along_x { (dir, 0.0) } else { (0.0, dir) };
            let speed = if vehicle {
                7.0 + 9.0 * self.rng.f32()
            } else {
                1.0 + 0.8 * self.rng.f32()
            };
            let hue = if vehicle {
                (self.rng.below(360)) as u16
            } else {
                // People read as warm; the neon behind them is cool.
                (10 + self.rng.below(60)) as u16
            };
            let plate = self.rng.next_u32() as u16;
            return Actor { x, z, hx, hz, speed, phase: self.rng.f32() * 6.28, hue, plate };
        }
        // Nowhere suitable nearby: park it far off, it will be recycled next tick.
        Actor { x: cx + 1e4, z: cz + 1e4, ..Default::default() }
    }

    /// Hand the traffic a list of registrations. Vehicles already on the road
    /// keep their key, so they simply resolve to a plate in the new list rather
    /// than being re-rolled.
    pub fn set_plates(&mut self, plates: Plates) {
        self.plates = plates;
    }

    pub fn update(&mut self, world: &World, dt: f32, cx: f32, cz: f32) {
        for i in 0..self.vehs.len() {
            let mut a = self.vehs[i];
            a.x += a.hx * a.speed * dt;
            a.z += a.hz * a.speed * dt;
            a.phase += dt * 4.0;
            let c = world.cell(a.x.floor() as i32, a.z.floor() as i32);
            let off = (a.x - cx).hypot(a.z - cz);
            if off > LIVE_RADIUS || c.height != 0 || c.surface != surface::ROADWAY {
                a = self.spawn_one(world, cx, cz, true);
            }
            self.vehs[i] = a;
        }
        for i in 0..self.peds.len() {
            let mut a = self.peds[i];
            a.x += a.hx * a.speed * dt;
            a.z += a.hz * a.speed * dt;
            a.phase += dt * 6.0;
            let c = world.cell(a.x.floor() as i32, a.z.floor() as i32);
            let off = (a.x - cx).hypot(a.z - cz);
            let on_foot = c.surface == surface::PAVEMENT || c.surface == surface::PAINTED;
            if off > LIVE_RADIUS || c.height != 0 || !on_foot {
                a = self.spawn_one(world, cx, cz, false);
            }
            self.peds[i] = a;
        }
    }
}

// --- weather -------------------------------------------------------------
/// What the sky is doing. A toggle, because rain over every frame for ever is
/// a choice the player should get to make — and because it is the one thing
/// here that costs anything, so nobody should pay for it unasked.
///
/// Rain lives in a disc around the camera rather than over the whole world:
/// there is no world-sized array to fill, and nothing off-screen to pay for.
/// The toggle is on `T` because `R` is the look-up key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weather {
    Clear,
    Rain,
    Downpour,
}

impl Weather {
    pub fn name(self) -> &'static str {
        match self {
            Weather::Clear => "clear",
            Weather::Rain => "rain",
            Weather::Downpour => "downpour",
        }
    }

    pub fn next(self) -> Weather {
        match self {
            Weather::Clear => Weather::Rain,
            Weather::Rain => Weather::Downpour,
            Weather::Downpour => Weather::Clear,
        }
    }

    /// How many drops are live. Zero when clear, and then the whole pass is
    /// skipped rather than run over an empty list.
    fn drops(self) -> usize {
        match self {
            Weather::Clear => 0,
            // Most of a camera-centred disc is behind you or off the top of
            // the frame; only about an eighth of it is ever in shot. These are
            // the counts that put a believable amount of rain ON SCREEN, which
            // is the only number that matters.
            Weather::Rain => 2400,
            Weather::Downpour => 5000,
        }
    }
}

/// One falling drop. It is a world-space point with a fall speed and nothing
/// else — the streak, the lean and the colour are the renderer's business.
#[derive(Clone, Copy, Default)]
pub struct Drop {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub speed: f32,
}

/// The disc of sky the rain occupies, centred on the camera and dragged along
/// with it. It has to reach a good way down the avenue: a disc that stops at
/// arm's length leaves the far half of a long street conspicuously dry, and
/// you can see exactly where the storm ends.
const RAIN_RADIUS: f32 = 36.0;
/// Drops enter here and are recycled once they reach the ground.
const RAIN_CEILING: f32 = 12.0;

pub struct Sky {
    pub weather: Weather,
    pub drops: Vec<Drop>,
    rng: Rng,
}

impl Sky {
    pub fn new(seed: u64) -> Self {
        Sky { weather: Weather::Clear, drops: Vec::new(), rng: Rng::new(seed ^ 0x_A1_5C_ED) }
    }

    pub fn set(&mut self, w: Weather, cx: f32, cz: f32) {
        self.weather = w;
        let n = w.drops();
        self.drops.truncate(n);
        while self.drops.len() < n {
            let d = self.fresh(cx, cz, true);
            self.drops.push(d);
        }
    }

    pub fn cycle(&mut self, cx: f32, cz: f32) -> Weather {
        let w = self.weather.next();
        self.set(w, cx, cz);
        w
    }

    /// A drop somewhere in the disc. `seeded` scatters it down the whole
    /// column so a shower does not begin as one solid sheet arriving together.
    fn fresh(&mut self, cx: f32, cz: f32, seeded: bool) -> Drop {
        let a = self.rng.f32() * core::f32::consts::TAU;
        // sqrt keeps the disc evenly filled rather than clumped at the middle.
        let r = RAIN_RADIUS * self.rng.f32().sqrt();
        let heavy = self.weather == Weather::Downpour;
        Drop {
            x: cx + r * a.cos(),
            z: cz + r * a.sin(),
            y: if seeded {
                RAIN_CEILING * self.rng.f32()
            } else {
                RAIN_CEILING + 5.0 * self.rng.f32()
            },
            speed: if heavy { 20.0 + 14.0 * self.rng.f32() } else { 13.0 + 9.0 * self.rng.f32() },
        }
    }

    pub fn update(&mut self, dt: f32, cx: f32, cz: f32) {
        if self.drops.is_empty() {
            return;
        }
        let r2 = (RAIN_RADIUS + 4.0) * (RAIN_RADIUS + 4.0);
        for i in 0..self.drops.len() {
            let mut d = self.drops[i];
            d.y -= d.speed * dt;
            let dx = d.x - cx;
            let dz = d.z - cz;
            if d.y <= 0.0 {
                // It landed. The one that replaces it starts at the ceiling.
                d = self.fresh(cx, cz, false);
            } else if dx * dx + dz * dz > r2 {
                // It was left behind by a camera that walked out from under it.
                // Its replacement is rain you are walking INTO, which is already
                // falling and already at every height — putting it at the
                // ceiling instead drops the whole storm in as one flat sheet,
                // and after a jump cut leaves the sky empty entirely.
                d = self.fresh(cx, cz, true);
            }
            self.drops[i] = d;
        }
    }
}
