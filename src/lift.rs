//! The lift.
//!
//! A tall building here is a stack of storeys and a **shaft** cut down through
//! it, with a glass car in the shaft. Everything with consequence is in this
//! module and in `interior`: which buildings have one, what floors it serves,
//! where the car is, and which way the panel under your hand will send it. The
//! renderer draws that state and decides nothing about it.
//!
//! ## Why a car is a room
//!
//! Being inside is already a MODE (`World::place`), and a lift car is a small
//! room that moves: it has a floor slab, a ceiling, walls, glazing you can see
//! the city through, and fittings you can walk up to. So it IS an `Interior`,
//! with a `Lift` bolted on for the one thing a room does not have — a `base`
//! that changes. Nothing in the raycaster, the collision, the depth buffer or
//! the renderer's pass list learned that lifts exist; `World::cell` answers
//! from the car's grid exactly the way it answers from a room's.
//!
//! ## What you see out of it, and why it is real
//!
//! The car is glazed on two opposite sides and they show two different things
//! for the same reason a room's window does — because `World::cell` falls
//! through to whatever the car's own grid has nothing to say about:
//!
//!   * **Outward**, past the glass at `d = 0`, is the outside of the building
//!     line: the CITY, at real distances, through the same DDA. Rising forty
//!     units up a shaft is the same camera move the elevated vista makes, so
//!     the street falls away underneath you for free and correctly.
//!   * **Inward**, past the glass at `GLASS_IN`, is the shaft: a well of open
//!     cells with a wall at the far end of it, `CORE_D` cells back. That wall
//!     is tagged `fit::SHAFT` and is textured from THIS module's storey table —
//!     a slab band at each floor level, the lit landing above it, the storey's
//!     own colour and its number. It is the same table the room you step out
//!     into is built from, so the floor you watched go past is the floor you
//!     arrive at.
//!
//! The well is what makes the second one legible. The vertical field of view is
//! `2 * V_HALF_FOV` ≈ 40 degrees, so a surface an arm's length away shows about
//! one world unit of its own height however tall it is — a stripe, not a floor.
//! Set the wall back `CORE_D - 2` cells and the same cone covers four or five
//! units of it, which is a storey and a bit. The depth of the shaft is a
//! rendering requirement before it is an architectural one, and it is written
//! down here because it does not look like one.

use crate::interior::Room;

/// Across the building's face, in cells, both shaft walls included. The car and
/// the well are the five cells between them.
///
/// The width is a rendering requirement as much as the depth is, and it is the
/// same requirement. The horizontal field of view is about 57 degrees; a well
/// two units wide seen from seven units back subtends fifteen of them, so the
/// picture would be a slot of shaft in a screen of dark side wall. Five units
/// across at that distance fills two thirds of the frame, which is what a shaft
/// looks like when you are standing in it.
pub const CORE_W: i32 = 7;
/// In from the face. `0` is the outward glazing and `CORE_D - 1` the wall at
/// the back of the shaft — the one the storeys are drawn on.
pub const CORE_D: i32 = 9;
/// The car's floor: the depth cells you can stand on.
pub const CAR_D0: i32 = 1;
pub const CAR_D1: i32 = 2;
/// The inward glazing, between the car and the shaft.
pub const GLASS_IN: i32 = 3;
/// The open well, and the wall at the back of it.
pub const WELL_D0: i32 = 4;
pub const FACE_D: i32 = CORE_D - 1;

/// Clear height inside the car.
pub const CAR_CLEAR: f32 = 2.35;
/// How high the car's glass starts off its own floor. It is a kick rail, not a
/// sill: low enough to see the shaft drop away under you and the street drop
/// away on the other side, high enough that the glass is still something solid
/// and you cannot walk out of a moving lift.
pub const CAR_SILL: f32 = 0.35;

/// Structure between one storey's ceiling and the next one's floor.
pub const SLAB: f32 = 0.8;
/// Clear height of a storey ABOVE the ground floor. The ground floor keeps
/// whatever its family gives it — a lobby is double height and should be — but
/// a stack of them cannot, or a tower would hold four floors.
pub const UPPER_CLEAR: f32 = 3.8;

/// How tall a building has to stand, at its own entrance, before it is worth a
/// lift. Not every building has one and this is the whole of why: it is the
/// height that justifies it, decided by the generator off the same seed, so a
/// given building always has one or always does not.
pub const MIN_HEIGHT: u8 = 24;
/// And it has to serve enough floors to be a lift rather than a step.
pub const MIN_FLOORS: usize = 4;
/// A bound, so a table is always small enough to scan.
pub const MAX_FLOORS: usize = 16;

/// Cruising speed of the car, world units a second, and the shortest a ride is
/// ever allowed to take. A lift that arrives instantly is a teleport with a
/// sound effect; these two together make one storey take about two seconds and
/// give the ease in and out room to be felt.
pub const SPEED: f32 = 2.6;
pub const MIN_RIDE: f32 = 1.1;

/// One floor of a building: where its slab is, how much clear height it has,
/// and enough of its character that the shaft wall can show it going past.
///
/// It is the SAME description `Interior::build` uses for the room on that
/// floor, which is what makes the ride honest: the band you watch slide down
/// past the glass is the room you step out into.
#[derive(Clone, Copy)]
pub struct Storey {
    pub floor: i32,
    /// World height of the floor slab.
    pub base: f32,
    /// Clear height above it.
    pub ceiling: f32,
    pub room: Room,
    pub wall_hue: f32,
    pub wall_sat: f32,
    pub light_hue: f32,
    pub light_sat: f32,
    pub ambient: f32,
}

impl Storey {
    #[inline]
    pub fn top(&self) -> f32 {
        self.base + self.ceiling
    }
}

/// Where the lift core stands inside a building, in the room's own local
/// coordinates — `a` across the face the entrance is in, `d` in from it. It is
/// a property of the BUILDING, not of a floor: it comes off the floor-blind
/// half of the generator (`interior::fabric`), so the shaft lands on the same
/// cells on every storey. A shaft that wandered a cell a floor is not a shaft.
#[derive(Clone, Copy)]
pub struct Core {
    /// First cell across. The core spans `a0 .. a0 + CORE_W`.
    pub a0: i32,
    /// The column the landing doors are in — one of the two shaft walls,
    /// whichever faces the room.
    pub door_a: i32,
    /// `interior::INWARD` index of the way INTO the core from the room. Cells
    /// carry `door = 9 + this`, and `Engine::portal` reads it exactly the way
    /// it reads a street threshold: which way is in.
    pub in_face: u8,
}

impl Core {
    /// The two landing cells, in local `(a, d)`.
    #[inline]
    pub fn landing(&self) -> [(i32, i32); 2] {
        [(self.door_a, CAR_D0), (self.door_a, CAR_D1)]
    }

    /// Is this local cell part of the core at all?
    #[inline]
    pub fn holds(&self, a: i32, d: i32) -> bool {
        a >= self.a0 && a < self.a0 + CORE_W && (0..CORE_D).contains(&d)
    }
}

/// **The car, as state.** Where it is, where it is going, and how far through
/// the ride it is. The renderer reads `y` and nothing else.
#[derive(Clone, Copy)]
pub struct Lift {
    /// The storey the car is at, or the one it last left.
    pub at: usize,
    /// The storey it is travelling to. Equal to `at` when it is standing.
    pub target: usize,
    /// World height of the car's floor. The camera stands on this.
    pub y: f32,
    from: f32,
    t: f32,
    dur: f32,
}

impl Lift {
    pub fn standing(at: usize, y: f32) -> Self {
        Lift { at, target: at, y, from: y, t: 0.0, dur: 0.0 }
    }

    #[inline]
    pub fn moving(&self) -> bool {
        self.dur > 0.0
    }

    /// **What the panel does.** `dir` is `+1` for the up button and `-1` for
    /// the down one. Pressing while the car is already moving extends the ride
    /// by another floor the same way, which is what a lift does when you press
    /// again on the way; pressing the other way while moving is ignored, and so
    /// is asking for a floor that does not exist.
    ///
    /// Returns whether the press did anything, so a frontend can say so.
    pub fn call(&mut self, storeys: &[Storey], dir: i32) -> bool {
        let want = self.target as i32 + dir;
        if want < 0 || want as usize >= storeys.len() {
            return false;
        }
        if self.moving() && (self.target as i32 - self.at as i32).signum() != dir {
            return false;
        }
        self.target = want as usize;
        self.from = self.y;
        self.t = 0.0;
        let travel = (storeys[self.target].base - self.y).abs();
        self.dur = MIN_RIDE.max(travel / SPEED);
        true
    }

    /// Advance the ride. Eased at both ends over the whole trip rather than
    /// integrated from an acceleration, so a call taken mid-ride re-plans from
    /// exactly where the car is and can never overshoot the floor it is going
    /// to. **Nothing here can teleport**: `y` is a continuous function of `t`,
    /// and `t` only ever advances by `dt`.
    pub fn update(&mut self, storeys: &[Storey], dt: f32) {
        if !self.moving() {
            return;
        }
        self.t += dt;
        let u = (self.t / self.dur).clamp(0.0, 1.0);
        // Smoothstep: zero velocity at both ends, peak in the middle.
        let e = u * u * (3.0 - 2.0 * u);
        let dest = storeys[self.target].base;
        self.y = self.from + (dest - self.from) * e;
        if u >= 1.0 {
            self.y = dest;
            self.at = self.target;
            self.dur = 0.0;
            self.t = 0.0;
        }
    }

    /// Is the car standing level with a floor? The landing doors are only open
    /// — only a threshold you can walk through at all — while this is true, and
    /// that is the whole of the safety interlock.
    #[inline]
    pub fn level(&self) -> bool {
        !self.moving()
    }

    /// **Which floor the car is passing**, which is not the same question as
    /// which floor it is *at*: on a ride of four storeys `at` stays where the
    /// ride began until it ends, and what an indicator over the doors shows —
    /// and what the shaft wall outside the glass agrees with — is this.
    pub fn passing(&self, storeys: &[Storey]) -> usize {
        let mut k = 0;
        for (i, s) in storeys.iter().enumerate() {
            if self.y >= s.base - SLAB * 0.5 {
                k = i;
            }
        }
        k
    }

    /// How far through the current ride, 0..1. One for a car that is standing.
    #[inline]
    pub fn progress(&self) -> f32 {
        if self.dur > 0.0 {
            (self.t / self.dur).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}
