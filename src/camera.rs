//! The camera, and everything that moves it. Walk, strafe, sprint, turn, look
//! up and down, and the eye height that turns a street-level walk into an
//! elevated vista — all of it here, none of it in a host.
//!
//! The basis, once, so nothing downstream has to guess the handedness:
//!
//! ```text
//! forward = ( sin yaw, -cos yaw)
//! right   = ( cos yaw,  sin yaw)
//! ```
//!
//! Speeds are 3.2 world units/s walking, 6.5 sprinting — a walk you can hold a
//! camera steady at, and a run that still lets you take a corner.

use crate::world::World;

/// Input bitmask. The frontends translate their own keys into this and hand it
/// over; nothing else about input crosses into the engine.
pub mod key {
    pub const FWD: u32 = 1 << 0;
    pub const BACK: u32 = 1 << 1;
    pub const STRAFE_L: u32 = 1 << 2;
    pub const STRAFE_R: u32 = 1 << 3;
    pub const TURN_L: u32 = 1 << 4;
    pub const TURN_R: u32 = 1 << 5;
    pub const SPRINT: u32 = 1 << 6;
    pub const LOOK_UP: u32 = 1 << 7;
    pub const LOOK_DOWN: u32 = 1 << 8;
    pub const RISE: u32 = 1 << 9;
    pub const SINK: u32 = 1 << 10;
}

/// Standing eye height, world units.
pub const EYE_STREET: f32 = 1.25;
/// The observation-deck height the vista view settles at.
pub const EYE_VISTA: f32 = 34.0;

pub const WALK_SPEED: f32 = 3.2;
pub const SPRINT_SPEED: f32 = 6.5;
const TURN_SPEED: f32 = 2.2;
const PITCH_SPEED: f32 = 1.1;
const PITCH_LIMIT: f32 = 0.9;
const RISE_SPEED: f32 = 14.0;
/// Body radius used for collision, so we do not clip through wall corners.
const RADIUS: f32 = 0.28;

/// How quickly the camera reaches the speed the keys are asking for. Short
/// enough that a step still starts the instant you press.
const ACCEL_TAU: f32 = 0.05;
/// How quickly it sheds that speed once nothing is asking any more, with no
/// glide. One frame's worth, so releasing a key stops you.
const DECEL_TAU: f32 = 0.06;

pub struct Camera {
    pub x: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// Current eye height. Above `EYE_STREET` the camera lifts clear of the
    /// street and collision stops applying — that is the elevated vista.
    pub eye: f32,
    /// Where the eye is heading; `eye` eases toward it so a view change reads
    /// as a rise rather than a cut.
    pub eye_target: f32,

    /// Current ground velocity, world units per second. Movement is a velocity
    /// the keys steer, not a displacement they apply, so it can outlive the
    /// key for as long as `glide` says.
    pub vx: f32,
    pub vz: f32,
    /// The last input that said anything, and how much of it is still in force.
    last_keys: u32,
    /// 0..1. Full while input is arriving, then ramping off over `glide`.
    strength: f32,
    /// **Seconds the last input still commands the camera after it stops
    /// arriving.** Zero is the honest setting when the terminal reports key
    /// releases: everything then begins and ends exactly with your finger.
    ///
    /// Above zero the whole input — walking, turning, looking, rising — stays
    /// in force for this long and then ramps off, and that is what turns a
    /// key-press *stream* into a movement. A terminal without the kitty
    /// protocol goes completely silent for the whole OS autorepeat delay
    /// (250–660 ms) after the first press, with your finger still on the key;
    /// without a glide that hole is a dead stop mid-stride, every time.
    ///
    /// The frontend sets it from what its terminal can actually do. The engine
    /// never guesses.
    pub glide: f32,
    /// The velocity the keys last asked for, and how long that ask is still
    /// good for.
    want: (f32, f32),
    carry: f32,
}

impl Camera {
    pub fn new(x: f32, z: f32, yaw: f32) -> Self {
        Camera {
            x,
            z,
            yaw,
            pitch: 0.0,
            eye: EYE_STREET,
            eye_target: EYE_STREET,
            vx: 0.0,
            vz: 0.0,
            last_keys: 0,
            strength: 0.0,
            glide: 0.0,
            want: (0.0, 0.0),
            carry: 0.0,
        }
    }

    #[inline]
    pub fn forward(&self) -> (f32, f32) {
        (self.yaw.sin(), -self.yaw.cos())
    }

    #[inline]
    pub fn right(&self) -> (f32, f32) {
        (self.yaw.cos(), self.yaw.sin())
    }

    /// True once the eye has climbed clear of street level.
    #[inline]
    pub fn airborne(&self) -> bool {
        self.eye > EYE_STREET + 0.5
    }

    pub fn toggle_vista(&mut self) {
        self.eye_target = if self.eye_target > EYE_STREET + 0.5 { EYE_STREET } else { EYE_VISTA };
    }

    pub fn update(&mut self, world: &World, dt: f32, keys: u32, look_x: f32, look_y: f32) {
        // Carry the last input across a silence, at a strength that ramps off.
        // This applies to EVERY axis, not just the feet: turning and looking
        // stall in exactly the same autorepeat hole walking does, and a camera
        // that swings, freezes for a third of a second, then swings again is
        // just as unusable as one that walks that way.
        let keys = if keys != 0 {
            self.last_keys = keys;
            self.carry = self.glide;
            self.strength = 1.0;
            keys
        } else if self.carry > 0.0 {
            self.carry -= dt;
            self.strength = (self.carry / self.glide.max(1e-6)).clamp(0.0, 1.0);
            self.last_keys
        } else {
            self.strength = 0.0;
            0
        };
        let dt_in = dt * self.strength;

        if keys & key::TURN_L != 0 {
            self.yaw -= TURN_SPEED * dt_in;
        }
        if keys & key::TURN_R != 0 {
            self.yaw += TURN_SPEED * dt_in;
        }
        self.yaw += look_x;
        // Keep yaw in a sane range so sin/cos never lose precision on a long walk.
        let tau = core::f32::consts::TAU;
        self.yaw -= tau * (self.yaw / tau).floor();

        if keys & key::LOOK_UP != 0 {
            self.pitch += PITCH_SPEED * dt_in;
        }
        if keys & key::LOOK_DOWN != 0 {
            self.pitch -= PITCH_SPEED * dt_in;
        }
        self.pitch = (self.pitch + look_y).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        if keys & key::RISE != 0 {
            self.eye_target = (self.eye_target + RISE_SPEED * dt_in).min(140.0);
        }
        if keys & key::SINK != 0 {
            self.eye_target = (self.eye_target - RISE_SPEED * dt_in).max(EYE_STREET);
        }
        // Ease toward the target: a time-constant follow, framerate independent.
        let k = 1.0 - (-6.0 * dt).exp();
        self.eye += (self.eye_target - self.eye) * k;
        if (self.eye - self.eye_target).abs() < 0.01 {
            self.eye = self.eye_target;
        }

        let sprint = keys & key::SPRINT != 0;
        let speed = if sprint { SPRINT_SPEED } else { WALK_SPEED };
        let (fx, fz) = self.forward();
        let (rx, rz) = self.right();
        let mut mx = 0.0f32;
        let mut mz = 0.0f32;
        if keys & key::FWD != 0 { mx += fx; mz += fz; }
        if keys & key::BACK != 0 { mx -= fx; mz -= fz; }
        if keys & key::STRAFE_R != 0 { mx += rx; mz += rz; }
        if keys & key::STRAFE_L != 0 { mx -= rx; mz -= rz; }
        let len = (mx * mx + mz * mz).sqrt();

        // Speed follows the same ramp everything else does, so a walk fades out
        // over the glide rather than being cut off at the end of it.
        let commanded = len > 1e-4;
        if commanded {
            // Normalise so walking a diagonal is not faster than walking straight.
            self.want = (mx / len * speed, mz / len * speed);
        }
        let tau = if commanded { ACCEL_TAU } else { DECEL_TAU };
        let k = 1.0 - (-dt / tau).exp();
        let (tx, tz) = if commanded {
            (self.want.0 * self.strength, self.want.1 * self.strength)
        } else {
            (0.0, 0.0)
        };
        self.vx += (tx - self.vx) * k;
        self.vz += (tz - self.vz) * k;
        if self.vx.abs() < 1e-3 && self.vz.abs() < 1e-3 {
            self.vx = 0.0;
            self.vz = 0.0;
            return;
        }
        let (hit_x, hit_z) = self.try_move(world, self.vx * dt, self.vz * dt);
        // Walking into a wall must not bank up speed you would shoot off with
        // the moment you turn away from it.
        if hit_x {
            self.vx = 0.0;
        }
        if hit_z {
            self.vz = 0.0;
        }
    }

    /// Stop dead. Used when a view change makes the current velocity
    /// meaningless.
    pub fn halt(&mut self) {
        self.vx = 0.0;
        self.vz = 0.0;
        self.carry = 0.0;
        self.strength = 0.0;
        self.last_keys = 0;
    }

    /// Axis-separated collision, so sliding along a wall works. Above street
    /// level there is nothing to collide with — you are over the rooftops.
    /// Returns which axes were blocked.
    fn try_move(&mut self, world: &World, mx: f32, mz: f32) -> (bool, bool) {
        if self.airborne() {
            self.x += mx;
            self.z += mz;
            return (false, false);
        }
        // If we are somehow already inside a solid cell, do not also refuse to
        // move — that is the difference between a bad frame and being stuck for
        // the rest of the run.
        if world.solid(self.x.floor() as i32, self.z.floor() as i32) {
            self.x += mx;
            self.z += mz;
            return (false, false);
        }
        let mut blocked = (false, false);
        let nx = self.x + mx + RADIUS * mx.signum();
        if !world.solid(nx.floor() as i32, self.z.floor() as i32) {
            self.x += mx;
        } else {
            blocked.0 = true;
        }
        let nz = self.z + mz + RADIUS * mz.signum();
        if !world.solid(self.x.floor() as i32, nz.floor() as i32) {
            self.z += mz;
        } else {
            blocked.1 = true;
        }
        blocked
    }
}

/// Pick a spawn: an open cell on the avenue near the requested origin, facing
/// down the longest clear run — so the first thing you see is a street with a
/// skyline down it, not a wall.
pub fn spawn(world: &World, near_x: i32, near_z: i32) -> Camera {
    let mut best = (near_x, near_z);
    'search: for r in 0..64i32 {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let (x, z) = (near_x + dx, near_z + dz);
                let c = world.cell(x, z);
                // Stand on the carriageway centre line, where the sightline is.
                if c.height == 0 && (6..=9).contains(&c.cross) {
                    best = (x, z);
                    break 'search;
                }
            }
        }
    }
    let (x, z) = best;
    // forward = (sin yaw, -cos yaw): yaw 0 is -Z, PI/2 is +X.
    let dirs = [
        (0, -1, 0.0f32),
        (1, 0, core::f32::consts::FRAC_PI_2),
        (0, 1, core::f32::consts::PI),
        (-1, 0, -core::f32::consts::FRAC_PI_2),
    ];
    let mut best_yaw = 0.0;
    let mut best_run = -1i32;
    for (dx, dz, yaw) in dirs {
        let mut run = 0;
        let (mut ix, mut iz) = (x, z);
        while run < 200 && !world.solid(ix + dx, iz + dz) {
            ix += dx;
            iz += dz;
            run += 1;
        }
        if run > best_run {
            best_run = run;
            best_yaw = yaw;
        }
    }
    Camera::new(x as f32 + 0.5, z as f32 + 0.5, best_yaw)
}
