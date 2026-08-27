//! Insides.
//!
//! The city outside is a pure function of coordinate and holds nothing, which
//! is what lets it be unbounded. **A room is the opposite of that on every
//! axis** and is modelled the opposite way: it is bounded, you are only ever in
//! one, and the things in it have *state* — a rack you cannot walk through, a
//! terminal worth walking up to. So a room is a real grid, built once when you
//! step through the door and held for as long as you are inside. A 20x24 room
//! is about six kilobytes; the city is still nothing.
//!
//! That grid answers `World::cell` while `World::place` is `Indoors`, which is
//! the whole of the trick: the raycaster, the collision and the depth buffer
//! never learn that anything changed. **Being indoors is a mode the engine is
//! in, not a special case in the renderer.**
//!
//! ## A room is not a sealed box
//!
//! Anywhere the room's grid does not answer, `World::cell` falls through to the
//! **city**. So a wall cell tagged `fit::WINDOW` — a sill you cannot climb over
//! and clear air above it — lets the same DDA that found the wall carry on out
//! into the real street and hit real buildings at real distances. There is no
//! second world, no painted-on backdrop and no parallax to fake: what you see
//! out of a window is the city, from where you are standing, and it swings the
//! way a window's view swings when you cross the room.
//!
//! ## Height, and floors
//!
//! A room sits on a **floor slab at `base`**, and `ceiling` is the clear height
//! above it. Nothing here assumes `base` is zero. `Cell::height` is absolute
//! world height as it is everywhere else in this engine, so a wall on the
//! thirty-first floor is simply a tall one and the raycaster never learns there
//! is such a thing as a storey. What you never see is that the wall carries on
//! below your feet: the floor slab is drawn opaque, and you are standing on it.
//!
//! Two `Cell` fields mean something different indoors, and only indoors:
//!
//!   * `win` — outside, the window lattice a facade uses; inside, `fit::*`,
//!     *what a cell is*: wall, window, column, counter, rack. It is the same
//!     question either way — which texture this surface takes — so it is the
//!     same field.
//!   * `surface` — outside, the ground class; inside, `floor::*`.
//!
//! `door` means exactly one thing in both places: a threshold you can walk
//! through, and which way is in. That is what makes the transition symmetric.

use crate::lift::{self, Core, Lift, Storey};
use crate::palette::building_of;
use crate::rng::{hash3, Rng};
use crate::world::{Cell, BLOCK};

/// The four inward directions a doorway can face, indexed by `Cell::door - 1`.
/// `door` counts from one so that zero can mean "no door", which is almost
/// every cell in the world.
pub const INWARD: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// How close to the plane at the far side of a threshold cell you have to be
/// for the engine to change mode. Collision stops a walker `Camera::RADIUS`
/// (0.28) short of a solid face, so this has to clear that — and it has to stay
/// well under half a cell, or the two thresholds would overlap and you would
/// oscillate between inside and out.
pub const PORTAL_GAP: f32 = 0.34;

/// How high a window's sill stands off the floor, world units. It is solid, so
/// you cannot walk out of a window; above it the cell is clear all the way to
/// the ceiling, which is what makes the glazing floor-to-head rather than a
/// slot. A height field cannot express a wall with a hole in the middle of it,
/// and a full-height bay over a sill is a real thing this city is full of.
pub const SILL: f32 = 1.0;

/// The car's own colours. A lift is the same lift in every building — brushed
/// steel and a warm light — so unlike a room these do not come off the
/// building's hash. It is a machine, not a room in the family.
const CAR_HUE: f32 = 206.0;
const CAR_SAT: f32 = 12.0;
const CAR_LIGHT_HUE: f32 = 44.0;

/// What a solid cell IS, indoors. Carried in `Cell::win`.
pub mod fit {
    pub const FLOOR: u8 = 0;
    pub const WALL: u8 = 1;
    /// A glazed bay: a sill you cannot pass, and the city above it.
    pub const WINDOW: u8 = 2;
    pub const COLUMN: u8 = 3;
    pub const COUNTER: u8 = 4;
    pub const RACK: u8 = 5;
    pub const MACHINE: u8 = 6;
    pub const CRATE: u8 = 7;
    pub const PARTITION: u8 = 8;
    pub const RAIL: u8 = 9;
    pub const PLANTER: u8 = 10;
    pub const DESK: u8 = 11;
    pub const TANK: u8 = 12;
    /// The lift core, seen from a room: a solid box with two doors in its
    /// flank. Everything that happens inside it happens in a different mode.
    pub const LIFT: u8 = 13;
    /// A wall of the shaft, seen from inside it. **The one surface in this
    /// engine drawn from the storey table** — see `render::shaft_face`.
    pub const SHAFT: u8 = 14;
    /// Open air in the shaft. Height zero like any open cell, so the DDA walks
    /// straight through it; tagged so the car's floor slab and ceiling stop at
    /// the glass instead of closing the shaft off above and below you.
    pub const WELL: u8 = 15;
}

/// Floor and ceiling material. Carried in `Cell::surface`.
pub mod floor {
    pub const TILE: u8 = 0;
    pub const BOARD: u8 = 1;
    pub const POURED: u8 = 2;
    pub const CARPET: u8 = 3;
    pub const GRATE: u8 = 4;
    pub const TERRAZZO: u8 = 5;
    /// The mat in the doorway. Lit from outside, so it is its own material.
    pub const THRESHOLD: u8 = 6;
}

/// The room families. A family fixes the *character* — how high the ceiling is,
/// what the light is like, what furniture belongs, how the floor is laid out —
/// and everything numeric inside it comes off the building's own hash, so two
/// markets are the same idea and not the same room.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Room {
    Lobby,
    Offices,
    Market,
    Workshop,
    Gallery,
    Concourse,
    Residence,
    Plant,
    Bar,
    Archive,
}

impl Room {
    pub fn word(self) -> &'static str {
        match self {
            Room::Lobby => "LOBBY",
            Room::Offices => "OFFICES",
            Room::Market => "MARKET",
            Room::Workshop => "WORKS",
            Room::Gallery => "GALLERY",
            Room::Concourse => "CONCOURSE",
            Room::Residence => "RESIDENCE",
            Room::Plant => "PLANT",
            Room::Bar => "BAR",
            Room::Archive => "ARCHIVE",
        }
    }
}

/// How the floor plan is organised. Three shapes cover every family; which one
/// a family takes is most of why a workshop does not feel like a bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Long runs with aisles between them, running away from the door. You
    /// walk down an aisle. Racks, desk rows, market stalls.
    Bands,
    /// Free-standing pieces with room between them. Tables, plinths, sofas.
    Scatter,
    /// Nearly empty, and structural: a column grid, a counter, some seating.
    Open,
}

/// Everything fixed about a family.
pub struct Style {
    room: Room,
    plan: Plan,
    /// Clear ceiling height range, world units above the floor slab.
    ceil: (f32, f32),
    floor_mat: u8,
    wall_hue: f32,
    wall_sat: f32,
    /// The ceiling light: hue, saturation, and how bright the room is before
    /// any distance falloff.
    light_hue: f32,
    light_sat: f32,
    /// 0.78..1.00 across the ten families. This is the ROOM's brightness, not
    /// a surface's: `room_light` still runs the same near/far falloff shape
    /// on top of it (unchanged), so raising the floor here is what makes a
    /// room readable without flattening the depth cue that shape gives. It
    /// used to run 0.40..0.86, which read as "differentiated" once floor and
    /// wall got their own hues, but was still too dim to navigate by — hue
    /// separation and light level are two different bugs.
    ///
    /// Deliberately NOT pushed to match the street: an interior's own hue
    /// saturation stays well under the city's (`world::cell`'s buildings roll
    /// 50..99, no family here goes over 34), and this band still measures a
    /// clear step down from a street frame's own mean/stddev — see the
    /// `--bench`-adjacent luminance numbers in the project's status history.
    /// Stepping through a door should feel like somewhere else, not like the
    /// same brightness with different wallpaper.
    ambient: f32,
    /// How fast light falls off across the room, world units to darkness. A
    /// room lit end to end and a room with one lit corner are different places.
    fall: f32,
    /// How much of the street wall is glass rather than pier, 0..1. A workshop
    /// has slot windows; a lobby is a shopfront.
    glazing: f32,
    /// What goes in the runs, and what stands about singly.
    run: u8,
    spots: &'static [u8],
    columns: bool,
    /// Roughly what fraction of the free floor ends up occupied.
    density: f32,
}

/// The ten families. The mix is the design: three of them are nearly empty and
/// tall, three are dense and low, and the rest sit between — so walking into
/// two buildings in a row does not give you the same room at two sizes.
static STYLES: [Style; 10] = [
    Style {
        room: Room::Lobby,
        plan: Plan::Open,
        ceil: (6.0, 8.4),
        floor_mat: floor::TERRAZZO,
        wall_hue: 36.0,
        wall_sat: 16.0,
        light_hue: 44.0,
        light_sat: 34.0,
        ambient: 0.93,
        fall: 40.0,
        glazing: 0.85,
        run: fit::RAIL,
        spots: &[fit::PLANTER, fit::COUNTER, fit::RAIL],
        columns: true,
        density: 0.05,
    },
    Style {
        room: Room::Offices,
        plan: Plan::Bands,
        ceil: (3.4, 4.3),
        floor_mat: floor::CARPET,
        wall_hue: 208.0,
        wall_sat: 12.0,
        light_hue: 196.0,
        light_sat: 20.0,
        ambient: 0.97,
        fall: 34.0,
        glazing: 0.75,
        run: fit::DESK,
        spots: &[fit::PARTITION, fit::PLANTER, fit::RACK],
        columns: false,
        density: 0.26,
    },
    Style {
        room: Room::Market,
        plan: Plan::Bands,
        ceil: (4.2, 5.8),
        floor_mat: floor::TILE,
        wall_hue: 16.0,
        wall_sat: 30.0,
        light_hue: 32.0,
        light_sat: 78.0,
        ambient: 1.00,
        fall: 30.0,
        glazing: 0.66,
        run: fit::COUNTER,
        spots: &[fit::CRATE, fit::RACK, fit::PLANTER],
        columns: false,
        density: 0.30,
    },
    Style {
        room: Room::Workshop,
        plan: Plan::Bands,
        ceil: (5.0, 7.2),
        floor_mat: floor::POURED,
        wall_hue: 30.0,
        wall_sat: 22.0,
        light_hue: 42.0,
        light_sat: 62.0,
        ambient: 0.87,
        fall: 22.0,
        glazing: 0.34,
        run: fit::MACHINE,
        spots: &[fit::CRATE, fit::TANK, fit::RACK],
        columns: true,
        density: 0.24,
    },
    Style {
        room: Room::Gallery,
        plan: Plan::Scatter,
        ceil: (5.4, 7.6),
        floor_mat: floor::BOARD,
        wall_hue: 262.0,
        wall_sat: 14.0,
        light_hue: 280.0,
        light_sat: 40.0,
        ambient: 0.79,
        fall: 18.0,
        glazing: 0.40,
        run: fit::PARTITION,
        spots: &[fit::COLUMN, fit::PLANTER, fit::PARTITION],
        columns: false,
        density: 0.10,
    },
    Style {
        room: Room::Concourse,
        plan: Plan::Open,
        ceil: (6.2, 8.6),
        floor_mat: floor::GRATE,
        wall_hue: 188.0,
        wall_sat: 26.0,
        light_hue: 184.0,
        light_sat: 66.0,
        ambient: 0.92,
        fall: 44.0,
        glazing: 0.80,
        run: fit::RAIL,
        spots: &[fit::RAIL, fit::COUNTER, fit::COLUMN],
        columns: true,
        density: 0.08,
    },
    Style {
        room: Room::Residence,
        plan: Plan::Scatter,
        ceil: (2.8, 3.5),
        floor_mat: floor::BOARD,
        wall_hue: 26.0,
        wall_sat: 26.0,
        light_hue: 36.0,
        light_sat: 56.0,
        ambient: 0.89,
        fall: 16.0,
        glazing: 0.55,
        run: fit::PARTITION,
        spots: &[fit::COUNTER, fit::RACK, fit::PLANTER, fit::PARTITION],
        columns: false,
        density: 0.16,
    },
    Style {
        room: Room::Plant,
        plan: Plan::Bands,
        ceil: (4.6, 6.8),
        floor_mat: floor::GRATE,
        wall_hue: 150.0,
        wall_sat: 22.0,
        light_hue: 128.0,
        light_sat: 60.0,
        ambient: 0.81,
        fall: 19.0,
        glazing: 0.22,
        run: fit::TANK,
        spots: &[fit::MACHINE, fit::CRATE, fit::RAIL],
        columns: false,
        density: 0.32,
    },
    Style {
        room: Room::Bar,
        plan: Plan::Scatter,
        ceil: (3.1, 4.0),
        floor_mat: floor::BOARD,
        wall_hue: 330.0,
        wall_sat: 34.0,
        light_hue: 318.0,
        light_sat: 82.0,
        ambient: 0.78,
        fall: 14.0,
        glazing: 0.60,
        run: fit::COUNTER,
        spots: &[fit::COUNTER, fit::PLANTER, fit::CRATE],
        columns: false,
        density: 0.18,
    },
    Style {
        room: Room::Archive,
        plan: Plan::Bands,
        ceil: (3.6, 4.8),
        floor_mat: floor::POURED,
        wall_hue: 156.0,
        wall_sat: 16.0,
        light_hue: 150.0,
        light_sat: 34.0,
        ambient: 0.85,
        fall: 20.0,
        glazing: 0.30,
        run: fit::RACK,
        spots: &[fit::CRATE, fit::RACK, fit::MACHINE],
        columns: false,
        density: 0.34,
    },
];

/// Something in a room worth standing in front of. Not solid — the solid
/// things are cells, because a thing you cannot walk through *is* geometry and
/// belongs where the collision and the raycaster already look.
///
/// This is the other half: a fixture carries the label, the verb and the reach
/// an interaction needs, so "there is a terminal by the door" is a fact about
/// the world and not something the renderer decided to draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fitting {
    /// Over the door, and the only thing in a room that is allowed to shout.
    ExitSign,
    Terminal,
    Notice,
    Lamp,
    Plant,
    Vent,
    /// The two buttons of the lift panel. Which one is under your hand is
    /// which one you are standing at — see `Interior::interaction_near`.
    CallUp,
    CallDown,
    /// Over the landing doors, and lit like the exit sign for the same reason:
    /// a lift you cannot find is a lift you do not have.
    LiftSign,
}

impl Fitting {
    pub fn label(self) -> &'static str {
        match self {
            Fitting::ExitSign => "EXIT",
            Fitting::Terminal => "TERMINAL",
            Fitting::Notice => "NOTICE BOARD",
            Fitting::Lamp => "STANDING LAMP",
            Fitting::Plant => "PLANTING",
            Fitting::Vent => "AIR HANDLER",
            Fitting::CallUp => "LIFT PANEL",
            Fitting::CallDown => "LIFT PANEL",
            Fitting::LiftSign => "LIFT",
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Fitting::ExitSign => "LEAVE",
            Fitting::Terminal => "USE",
            Fitting::Notice => "READ",
            Fitting::Lamp => "SWITCH",
            Fitting::Plant => "LOOK",
            Fitting::Vent => "LISTEN",
            Fitting::CallUp => "GO UP",
            Fitting::CallDown => "GO DOWN",
            Fitting::LiftSign => "STEP IN",
        }
    }

    /// How close you have to be for it to be the thing you mean.
    pub fn reach(self) -> f32 {
        match self {
            Fitting::ExitSign => 3.0,
            Fitting::Terminal => 1.8,
            Fitting::Notice => 2.0,
            Fitting::Lamp => 1.6,
            Fitting::Plant => 1.6,
            Fitting::Vent => 2.4,
            // The two buttons are at either end of the car, and this is set to
            // cover the whole of it from either one: wherever you stand, one of
            // them is the nearer, and the HUD says which before you press.
            Fitting::CallUp | Fitting::CallDown => 3.4,
            Fitting::LiftSign => 2.6,
        }
    }

    /// The word a lit sign carries, for the two fittings that are signs.
    pub fn word(self) -> &'static [u8] {
        match self {
            Fitting::LiftSign => b"LIFT",
            _ => b"EXIT",
        }
    }
}

/// One placed fixture, in world coordinates — the same shape `world::Placed`
/// takes for street furniture, and drawn the same way. `bottom` and `top` are
/// heights above the room's own floor slab, not above the ground.
#[derive(Clone, Copy)]
pub struct Fixture {
    pub x: f32,
    pub z: f32,
    pub bottom: f32,
    pub top: f32,
    pub kind: Fitting,
    pub hue: f32,
    pub seed: u32,
}

/// One glazed bay, in world coordinates. Part of the room's description in its
/// own right — the generator decides where the glass is and the cell grid is
/// stamped from this — so a later floor, or a lift car, can be told "you are
/// glazed on this side" without anything having to read the wall back.
#[derive(Clone, Copy)]
pub struct Window {
    pub x: i32,
    pub z: i32,
    /// Height of the sill above the floor slab.
    pub sill: f32,
}

/// **Which building's inside this is** — everything the generator needs to
/// build any storey of it, and nothing about which storey. Carried by every
/// `Interior` so a car can rebuild the room it opens on to without the engine
/// having to remember where it came from.
#[derive(Clone, Copy)]
pub struct Site {
    pub seed: i32,
    /// The first cell of the street doorway, and which way it faces in.
    pub dx: i32,
    pub dz: i32,
    pub face: u8,
    pub plan: u16,
    pub grain: i32,
}

pub struct Interior {
    pub room: Room,
    pub site: Site,
    /// Which floor of the building this is. Zero is the one you walk into off
    /// the street. Nothing in here assumes it — `base` carries the height and
    /// the label carries the number.
    pub floor: i32,
    /// `<the building's own name> <room word>` — the name over the door
    /// outside and the name of the room inside are the same name, because it
    /// is the same building.
    pub label: [u8; 40],
    pub label_len: usize,

    /// The room's rectangle, in the SAME world coordinates the street uses.
    /// Nothing is re-based when you walk in, so the transition has no teleport
    /// in it at all: you are in the doorway cell before and after.
    pub x0: i32,
    pub z0: i32,
    pub wx: i32,
    pub wz: i32,

    /// World height of the floor slab. Always a whole number of units, so a
    /// cell height — which is whole units too — can carry a storey exactly.
    pub base: f32,
    /// Clear height ABOVE the slab.
    pub ceiling: f32,
    /// Inward, from the doorway into the room.
    pub ix: i32,
    pub iz: i32,
    /// The doorway cells.
    pub door_cells: [(i32, i32); 2],
    pub windows: Vec<Window>,
    /// How high the glazing's sill stands off this floor's slab. A room's is
    /// `SILL`; a car's is a kick rail. It is read back rather than taken off
    /// the cell's own height because a car's slab is at a fraction of a unit
    /// and a cell height is a whole one — quantising the sill would make it
    /// jump a unit at a time as the car rose.
    pub sill: f32,

    /// **The building's floors, if it has a lift.** Empty otherwise, which is
    /// most buildings. The same table builds the room on every storey, decides
    /// where the car can stop, and is what the shaft wall is textured from.
    pub storeys: Vec<Storey>,
    /// Where the lift core stands in this building's plan. `Some` exactly when
    /// `storeys` is non-empty.
    pub core: Option<Core>,
    /// Set on the CAR and on nothing else: this interior is the one that moves.
    pub lift: Option<Lift>,

    pub floor_mat: u8,
    pub wall_hue: f32,
    pub wall_sat: f32,
    /// The floor's own hue — deliberately NOT `wall_hue`. A wall and a floor
    /// that share a hue and sit in the same narrow lightness band (the ceiling
    /// strips were the only thing pulling this room out of monochrome) is the
    /// exact failure `a_room_reads_as_a_room_not_a_haze` guards: rotated a
    /// fixed 160 degrees off the wall so every family gets a floor that reads
    /// as its own surface at a glance, whatever `wall_hue` happens to be.
    pub floor_hue: f32,
    pub light_hue: f32,
    pub light_sat: f32,
    /// Ceiling light strips: how many cells apart, which axis they run along,
    /// and where the run starts.
    pub light_pitch: i32,
    pub light_along_x: bool,
    pub light_phase: i32,
    /// Structural beams across the ceiling, in cells.
    pub beam_pitch: i32,
    pub ambient: f32,
    pub fall: f32,
    /// Tallest solid thing in here, for the raycaster's occlusion cull.
    pub tallest: u8,

    pub props: Vec<Fixture>,
    cells: Vec<Cell>,
    /// Cells from the doorway, per cell, `u16::MAX` where nothing stands. Built
    /// by flood once the furniture is in; see `way_out`.
    to_exit: Vec<u16>,
}

/// **The half of a building's inside that does not depend on which floor you
/// are on.** How big its rooms are, where they sit in the world, where the
/// entrance falls along the frontage, and where the lift core stands.
///
/// It exists because a shaft has to be a shaft: the core must land on the same
/// world cells on every storey, or riding one floor would step you sideways
/// into a wall. So the FOOTPRINT comes off a key with the storey number left
/// out, and only the *character* — the family, and with it the colours, the
/// ceiling height and everything in the room — comes off a key with it in.
///
/// Floor zero's per-floor key IS the floor-blind key (`floor_no * 9973 == 0`),
/// so every ground-floor room in the city is exactly the room it was before
/// there were lifts, cell for cell.
pub struct Fabric {
    pub st: &'static Style,
    /// The generator's stream, left exactly where `build` used to have it: the
    /// draws after this point are the glazing and the furniture.
    pub r: Rng,
    pub key: u32,
    pub across: i32,
    pub deep: i32,
    pub off: i32,
    pub ceiling: f32,
    pub ix: i32,
    pub iz: i32,
    pub x0: i32,
    pub z0: i32,
    pub wx: i32,
    pub wz: i32,
    /// Where a lift core would stand. Whether there IS one is a question about
    /// the building's height, and `plan_storeys` is what answers it.
    pub core: Option<Core>,
}

pub fn fabric(site: Site, floor_no: i32) -> Fabric {
    let (ix, iz) = INWARD[(site.face as usize).min(3)];
    let bx = site.dx.div_euclid(BLOCK);
    let bz = site.dz.div_euclid(BLOCK);
    let fabric_key = hash3(
        bx.wrapping_mul(6301) + site.plan as i32,
        bz.wrapping_mul(4507),
        site.seed ^ 0x1F7,
    );
    let key = hash3(
        bx.wrapping_mul(6301) + site.plan as i32,
        bz.wrapping_mul(4507) + floor_no.wrapping_mul(9973),
        site.seed ^ 0x1F7,
    );
    let mut r = Rng::new(fabric_key as u64 | ((site.plan as u64) << 32));

    let st = &STYLES[(key % 10) as usize];
    // Across the street frontage, and back from it. A room is never square by
    // accident: the two come off different draws. Depth is kept near the depth
    // of a real block, so the back wall is roughly where the back of the
    // building is.
    let across = 14 + r.below(12) as i32;
    let deep = 15 + r.below(9) as i32;
    let raw_ceiling = st.ceil.0 + (st.ceil.1 - st.ceil.0) * r.f32();
    // The ground floor keeps whatever its family gives it — a lobby is double
    // height and should be. A stack of double-height storeys is not a building,
    // so everything above it is capped to an ordinary floor-to-ceiling.
    let ceiling = if floor_no == 0 { raw_ceiling } else { raw_ceiling.min(lift::UPPER_CLEAR) };
    // The doorway is two cells wide; put it a little off the middle of the near
    // wall, so you do not walk into every room down its own axis.
    let off = 2 + r.below((across as u32).saturating_sub(6).max(1)) as i32;

    let (x0, z0, wx, wz);
    if ix != 0 {
        // Depth along X, across along Z.
        wx = deep;
        wz = across;
        x0 = if ix > 0 { site.dx } else { site.dx - deep + 1 };
        z0 = site.dz - off;
    } else {
        wx = across;
        wz = deep;
        x0 = site.dx - off;
        z0 = if iz > 0 { site.dz } else { site.dz - deep + 1 };
    }

    // **The core stands beside the entrance**, three cells clear of the doorway
    // on whichever side has room for it. Two reasons, and the second is the one
    // that matters: a lift by the door is where a lift is, so walking in off
    // the street puts the landing in front of you; and a room's frontage runs
    // 14 to 25 cells on a block whose plots are often half that, so a core at
    // the FAR end of one would routinely be standing over the avenue rather
    // than in the building. Hard by the doorway it is as far inside the plot as
    // the doorway is.
    //
    // A frontage with room on neither side simply gets no lift — one of the two
    // reasons a building may be tall and still have none, the other being that
    // it is not tall enough (`World::storeys`).
    //
    // `+a` is `+Z` when the room runs back along X and `+X` when it runs back
    // along Z — the same mapping `world_of` uses.
    let hi = off + 3;
    let lo = off - lift::CORE_W - 1;
    let hi_ok = hi + lift::CORE_W < across;
    let lo_ok = lo > 0;
    let core = if hi_ok && (!lo_ok || across - 1 - hi - lift::CORE_W >= lo) {
        // Core on the far side of the doorway; the room, and the way you came
        // in, are on the near side, so the doors face back that way.
        Some(Core { a0: hi, door_a: hi, in_face: if ix != 0 { 2 } else { 0 } })
    } else if lo_ok {
        Some(Core {
            a0: lo,
            door_a: lo + lift::CORE_W - 1,
            in_face: if ix != 0 { 3 } else { 1 },
        })
    } else {
        None
    };

    Fabric { st, r, key, across, deep, off, ceiling, ix, iz, x0, z0, wx, wz, core }
}

impl Fabric {
    /// The world cell of a local coordinate, before there is an `Interior` to
    /// ask. Same mapping `Interior::world_of` uses, and it has to stay the same
    /// one: the storey table is planned off this and the room is stamped off
    /// that.
    #[inline]
    pub fn world_of(&self, a: i32, d: i32) -> (i32, i32) {
        if self.ix > 0 {
            (self.x0 + d, self.z0 + a)
        } else if self.ix < 0 {
            (self.x0 + self.wx - 1 - d, self.z0 + a)
        } else if self.iz > 0 {
            (self.x0 + a, self.z0 + d)
        } else {
            (self.x0 + a, self.z0 + self.wz - 1 - d)
        }
    }
}

/// **The floors a building serves, and whether it has a lift at all.**
///
/// A lift belongs in something with the height to justify it, and `height` is
/// the building's own height at its entrance — the wall behind the doorway,
/// which is the part of the building the shaft is cut down. The answer is a
/// pure function of the seed and the plot, so a given building always has one
/// or always does not; walk past it twice and it says the same thing.
///
/// Slabs land on whole units because a cell height is a whole unit and a wall
/// on the seventh floor is just a tall one — that is what lets the raycaster,
/// the collision and the depth buffer stay ignorant of storeys.
pub fn plan_storeys(site: Site, height: u8) -> Vec<Storey> {
    if height < lift::MIN_HEIGHT || fabric(site, 0).core.is_none() {
        return Vec::new();
    }
    let roof = height as f32;
    let mut out: Vec<Storey> = Vec::new();
    let mut base = 0.0f32;
    while out.len() < lift::MAX_FLOORS {
        let n = out.len() as i32;
        let f = fabric(site, n);
        if base + f.ceiling + lift::SLAB > roof {
            break;
        }
        out.push(Storey {
            floor: n,
            base,
            ceiling: f.ceiling,
            room: f.st.room,
            wall_hue: f.st.wall_hue,
            wall_sat: f.st.wall_sat,
            light_hue: f.st.light_hue,
            light_sat: f.st.light_sat,
            ambient: f.st.ambient,
        });
        base = (base + f.ceiling + lift::SLAB).ceil();
    }
    if out.len() < lift::MIN_FLOORS {
        return Vec::new();
    }
    out
}

impl Interior {
    /// World height of the ceiling plane.
    #[inline]
    pub fn ceiling_y(&self) -> f32 {
        self.base + self.ceiling
    }

    /// Is this cell part of the room at all? The one question the renderer asks
    /// to tell the room from the city it can see out of the window, and it is a
    /// bounds check.
    #[inline]
    pub fn contains(&self, gx: i32, gz: i32) -> bool {
        let (u, v) = (gx - self.x0, gz - self.z0);
        u >= 0 && v >= 0 && u < self.wx && v < self.wz
    }

    /// The room's own answer for a cell, or `None` where the room has nothing
    /// to say and the CITY answers instead. That fall-through is what a window
    /// is.
    #[inline]
    pub fn at(&self, gx: i32, gz: i32) -> Option<Cell> {
        if !self.contains(gx, gz) {
            return None;
        }
        Some(self.cells[((gz - self.z0) * self.wx + (gx - self.x0)) as usize])
    }

    /// The continuous world point of a local coordinate — `a` across the
    /// entrance's frontage, `d` in from it. `world_of` is this at whole cells;
    /// a fixture on a wall needs it between them.
    #[inline]
    pub fn point_of(&self, a: f32, d: f32) -> (f32, f32) {
        if self.ix > 0 {
            (self.x0 as f32 + d, self.z0 as f32 + a)
        } else if self.ix < 0 {
            (self.x0 as f32 + self.wx as f32 - d, self.z0 as f32 + a)
        } else if self.iz > 0 {
            (self.x0 as f32 + a, self.z0 as f32 + d)
        } else {
            (self.x0 as f32 + a, self.z0 as f32 + self.wz as f32 - d)
        }
    }

    /// **Which storey a world height is on, and how far above its slab.** The
    /// one lookup the shaft wall is textured through, so the floors going past
    /// the glass are the building's real floors and not a repeating pattern
    /// that happens to look like some. A table is at most `MAX_FLOORS` long and
    /// is walked from the top, so this is a handful of compares.
    #[inline]
    pub fn storey_at(&self, y: f32) -> Option<(&Storey, f32)> {
        self.storeys
            .iter()
            .rev()
            .find(|s| y >= s.base - lift::SLAB)
            .map(|s| (s, y - s.base))
    }

    /// Where this floor sits in the building's own table.
    #[inline]
    pub fn storey_index(&self) -> Option<usize> {
        self.storeys.iter().position(|s| s.floor == self.floor)
    }

    /// World height of the top of the shaft: the soffit over the well.
    #[inline]
    pub fn shaft_head(&self) -> f32 {
        self.storeys.last().map(|s| (s.top() + lift::SLAB).ceil()).unwrap_or(0.0)
    }

    #[inline]
    pub fn label_str(&self) -> &str {
        core::str::from_utf8(&self.label[..self.label_len]).unwrap_or("")
    }

    /// The nearest thing worth interacting with, and how far off it is. State
    /// in the world model, available to a HUD or to anything later that wants
    /// to act on it — never something the renderer knows and nobody else does.
    pub fn interaction_near(&self, x: f32, z: f32) -> Option<(&Fixture, f32)> {
        let mut best: Option<(&Fixture, f32)> = None;
        for p in &self.props {
            let d = (p.x - x).hypot(p.z - z);
            if d > p.kind.reach() {
                continue;
            }
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((p, d));
            }
        }
        best
    }

    /// The middle of the doorway, in world coordinates — where to walk if what
    /// you want is out. The autopilot needs it and so would anything else that
    /// has to leave a room without a map of it.
    pub fn exit_point(&self) -> (f32, f32) {
        let (a, b) = (self.door_cells[0], self.door_cells[1]);
        (
            0.5 * (a.0 + b.0) as f32 + 0.5 - self.ix as f32 * 0.2,
            0.5 * (a.1 + b.1) as f32 + 0.5 - self.iz as f32 * 0.2,
        )
    }

    /// How far you could walk from `(x, z)` on heading `yaw` before something
    /// stopped you, capped at `max`. Half a cell at a time, so it cannot tunnel
    /// a corner — the same probe the attract mode uses outdoors.
    pub fn clearance(&self, x: f32, z: f32, yaw: f32, max: f32) -> f32 {
        let (dx, dz) = (yaw.sin(), -yaw.cos());
        let mut d = 0.4f32;
        while d < max {
            if !self.open((x + dx * d).floor() as i32, (z + dz * d).floor() as i32) {
                return d;
            }
            d += 0.4;
        }
        max
    }

    /// **The way out, from anywhere in the room.** Returns what a walker should
    /// hold: whether to go forward, and which way to turn (-1, 0, +1).
    ///
    /// It follows `to_exit`, a flood of how many cells each open cell is from
    /// the doorway, computed once when the room is built. Steering straight at
    /// the door instead is not enough and the test that says so found it twice:
    /// a rack between you and the way out turns "walk at the exit" into walk,
    /// slide, turn back, walk, for ever, and no amount of shoulder-checking
    /// heuristics fixes the general case. A distance field has no general case
    /// — every open cell in the room is one step downhill from a shorter one,
    /// because that is what building it by flood means.
    ///
    /// It lives here rather than in the attract mode because the way out of a
    /// room is a fact about the room, and anything that has to leave one
    /// without a map needs it. Eight hundred bytes, built once.
    pub fn way_out(&self, x: f32, z: f32, yaw: f32) -> (bool, i32) {
        let here = (x.floor() as i32, z.floor() as i32);
        // Two cells down the gradient rather than one, so a walker crossing a
        // room aims at where it is going instead of zig-zagging between cell
        // centres.
        let mut at = here;
        let mut aim = (x, z);
        for _ in 0..2 {
            match self.downhill(at) {
                Some(n) => {
                    at = n;
                    aim = (n.0 as f32 + 0.5, n.1 as f32 + 0.5);
                }
                None => break,
            }
        }
        if at == here {
            // Standing on the doorway, or somewhere the flood never reached.
            let (ex, ez) = self.exit_point();
            aim = (ex, ez);
        }
        let want = (aim.0 - x).atan2(-(aim.1 - z));
        let tau = core::f32::consts::TAU;
        let mut off = want - yaw;
        off -= tau * (off / tau + 0.5).floor();
        let turn = if off.abs() < 0.14 { 0 } else if off > 0.0 { 1 } else { -1 };
        // Edge forward while turning too, so a pivot cannot stall in a corner.
        (off.abs() < 0.9, turn)
    }

    /// The neighbouring open cell that is nearer the doorway, if there is one.
    fn downhill(&self, (cx, cz): (i32, i32)) -> Option<(i32, i32)> {
        let d0 = self.step_count(cx, cz)?;
        let mut best: Option<((i32, i32), u16)> = None;
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = (cx + dx, cz + dz);
            let Some(d) = self.step_count(n.0, n.1) else { continue };
            if d < d0 && best.is_none_or(|(_, b)| d < b) {
                best = Some((n, d));
            }
        }
        best.map(|(n, _)| n)
    }

    /// How many cells this one is from the doorway, or `None` where nothing
    /// stands.
    #[inline]
    fn step_count(&self, gx: i32, gz: i32) -> Option<u16> {
        if !self.contains(gx, gz) {
            return None;
        }
        let d = self.to_exit[((gz - self.z0) * self.wx + (gx - self.x0)) as usize];
        if d == u16::MAX {
            None
        } else {
            Some(d)
        }
    }

    /// Is this a cell you can stand in?
    #[inline]
    pub fn open(&self, gx: i32, gz: i32) -> bool {
        self.at(gx, gz).is_some_and(|c| c.height == 0)
    }

    // --- generation ------------------------------------------------------

    /// Build the inside of the building whose doorway is at `(dx, dz)` facing
    /// `face`. Everything comes off `seed` and the building's own identity, so
    /// a given building is the same inside every time you walk in — and two
    /// different buildings are not.
    ///
    /// `floor_no` is which storey, `base` the world height of its slab, and
    /// `storeys` the building's own floor table — empty for a building with no
    /// lift, which is most of them. Nothing below this line reads the floor
    /// number as "probably zero".
    pub fn build(site: Site, floor_no: i32, base: f32, storeys: Vec<Storey>) -> Interior {
        let f = fabric(site, floor_no);
        let mut r = f.r;
        let st = f.st;
        let key = f.key;
        let (ix, iz) = (f.ix, f.iz);
        let ceiling = f.ceiling;
        let (x0, z0, wx, wz) = (f.x0, f.z0, f.wx, f.wz);
        let (dx, dz, face, plan_id, grain) = (site.dx, site.dz, site.face, site.plan, site.grain);
        let base = base.round();
        let top_h = (base + ceiling).ceil() as u8;

        let ground = Cell {
            height: 0,
            surface: st.floor_mat,
            hue: st.wall_hue as u16,
            sat: st.wall_sat as u8,
            lit: 0,
            win: fit::FLOOR,
            arch: 0,
            plan: plan_id,
            cross: 255,
            door: 0,
        };
        let wall = Cell {
            height: top_h,
            win: fit::WALL,
            lit: 8 + r.below(14) as u8,
            ..ground
        };
        let mut cells = vec![ground; (wx * wz) as usize];
        for v in 0..wz {
            for u in 0..wx {
                if u == 0 || v == 0 || u == wx - 1 || v == wz - 1 {
                    cells[(v * wx + u) as usize] = wall;
                }
            }
        }
        // One room in four faces an accent wall as you come in. It is what
        // stops a room reading as a cylinder.
        if (key >> 7) % 4 == 0 {
            let ah = (st.wall_hue + 120.0 + 90.0 * r.f32()) % 360.0;
            for u in 0..wx {
                for v in 0..wz {
                    let far = if ix > 0 {
                        u == wx - 1
                    } else if ix < 0 {
                        u == 0
                    } else if iz > 0 {
                        v == wz - 1
                    } else {
                        v == 0
                    };
                    if far {
                        let c = &mut cells[(v * wx + u) as usize];
                        c.hue = ah as u16;
                        c.sat = (st.wall_sat + 18.0) as u8;
                    }
                }
            }
        }

        let mut it = Interior {
            room: st.room,
            site,
            floor: floor_no,
            label: [0; 40],
            label_len: 0,
            x0,
            z0,
            wx,
            wz,
            base,
            ceiling,
            ix,
            iz,
            door_cells: [(dx, dz), (dx, dz)],
            windows: Vec::new(),
            sill: SILL,
            core: if storeys.is_empty() { None } else { f.core },
            storeys,
            lift: None,
            floor_mat: st.floor_mat,
            wall_hue: st.wall_hue,
            wall_sat: st.wall_sat,
            floor_hue: (st.wall_hue + 160.0) % 360.0,
            light_hue: st.light_hue,
            light_sat: st.light_sat,
            light_pitch: 3 + r.below(4) as i32,
            light_along_x: ix != 0,
            light_phase: r.below(6) as i32,
            beam_pitch: 5 + r.below(5) as i32,
            ambient: st.ambient,
            fall: st.fall,
            tallest: top_h,
            props: Vec::new(),
            cells,
            to_exit: Vec::new(),
        };

        // --- the doorway ---------------------------------------------------
        // Two cells wide, carved out of the near wall, on the same world cells
        // the street-side threshold occupies.
        let (ax, az) = if ix != 0 { (0, 1) } else { (1, 0) };
        it.door_cells = [(dx, dz), (dx + ax, dz + az)];
        for &(cx, cz) in &it.door_cells.clone() {
            if let Some(c) = it.at_mut(cx, cz) {
                c.height = 0;
                c.win = fit::FLOOR;
                c.surface = floor::THRESHOLD;
                c.door = face + 1;
                c.lit = 100;
            }
        }

        // --- the lift core -------------------------------------------------
        // Solid floor to ceiling, with two doors in the flank that faces the
        // room. From in here a lift is a box you walk up to; the shaft inside
        // it is a mode away. It is carved BEFORE the glazing and the furniture
        // so both simply find the cells taken.
        if let Some(core) = it.core {
            let shell = Cell { height: top_h, win: fit::LIFT, lit: 24, ..ground };
            for a in core.a0..core.a0 + lift::CORE_W {
                for d in 0..lift::CORE_D {
                    let (gx, gz) = it.world_of(a, d);
                    if let Some(c) = it.at_mut(gx, gz) {
                        *c = shell;
                    }
                }
            }
            let step = if core.in_face % 2 == 0 { 1 } else { -1 };
            for (a, d) in core.landing() {
                let (gx, gz) = it.world_of(a, d);
                if let Some(c) = it.at_mut(gx, gz) {
                    c.height = 0;
                    c.win = fit::FLOOR;
                    c.surface = floor::THRESHOLD;
                    // The same field, meaning the same thing it always means:
                    // a threshold you can walk through, and which way is in.
                    c.door = 9 + core.in_face;
                    c.lit = 100;
                }
                // And the back of the recess, warm: a doorway one cell deep in
                // a wall of the same steel as the wall reads as no doorway at
                // all, which is what the first cut of this looked like.
                let (bx, bz) = it.world_of(a + step, d);
                if let Some(c) = it.at_mut(bx, bz) {
                    c.hue = 44;
                    c.sat = 48;
                    c.lit = 90;
                }
            }
        }

        it.glaze(st, &mut r);
        it.furnish(st, &mut r, key);
        it.fixtures(st, &mut r, key);
        it.name(plan_id, grain, dx, dz);
        it.tallest = it.cells.iter().map(|c| c.height).max().unwrap_or(top_h).max(top_h);
        it.flood_from_the_door();
        it
    }

    // --- the car ---------------------------------------------------------

    /// **The glass car, built off the room you stepped in from.**
    ///
    /// It stands in exactly the world cells the core occupies, so stepping in
    /// has no teleport in it — the same property that makes walking through a
    /// street door honest. What it is made of is the whole feature:
    ///
    /// ```text
    ///   d = 0            glass, and past it the CITY        (outward)
    ///   d = 1 .. 2       the car floor, and you on it
    ///   d = 3            glass, and past it the SHAFT        (inward)
    ///   d = 4 .. 7       the well: open, no floor, no ceiling
    ///   d = 8            the shaft wall, textured by STOREY
    ///   a = 0, a = 4     the shaft walls, and the landing doors in one of them
    /// ```
    ///
    /// Neither side is painted on. Outward, the car's grid simply stops and
    /// `World::cell` falls through to the city — the same mechanism a room's
    /// window has, and rising the height of a tower is a camera move the
    /// elevated vista already makes, so the street falls away underneath you
    /// correctly and for free. Inward, the well is open cells the DDA walks
    /// straight through to a wall `CORE_D - 2` back, which is far enough for
    /// the vertical field of view to cover a storey and a bit of it at once.
    pub fn car(from: &Interior, at: usize) -> Interior {
        let core = from.core.expect("a lift car in a building with no core");
        let st = &from.storeys[at];
        let head = from.shaft_head();
        let top_h = head.min(u8::MAX as f32) as u8;

        // The core's own rectangle, in world coordinates, with local `a`
        // re-based to zero. `world_of` then means the same thing in the car
        // that it means in the room it was cut out of.
        let (x0, z0, wx, wz);
        if from.ix != 0 {
            wx = lift::CORE_D;
            wz = lift::CORE_W;
            x0 = if from.ix > 0 { from.x0 } else { from.x0 + from.wx - lift::CORE_D };
            z0 = from.z0 + core.a0;
        } else {
            wx = lift::CORE_W;
            wz = lift::CORE_D;
            x0 = from.x0 + core.a0;
            z0 = if from.iz > 0 { from.z0 } else { from.z0 + from.wz - lift::CORE_D };
        }

        let blank = Cell {
            height: 0,
            surface: floor::GRATE,
            hue: CAR_HUE as u16,
            sat: CAR_SAT as u8,
            lit: 0,
            win: fit::FLOOR,
            arch: 0,
            plan: from.site.plan,
            cross: 255,
            door: 0,
        };
        let mut it = Interior {
            room: Room::Lobby,
            site: from.site,
            floor: st.floor,
            label: [0; 40],
            label_len: 0,
            x0,
            z0,
            wx,
            wz,
            base: st.base,
            ceiling: lift::CAR_CLEAR,
            ix: from.ix,
            iz: from.iz,
            door_cells: [(0, 0), (0, 0)],
            windows: Vec::new(),
            sill: lift::CAR_SILL,
            core: Some(Core { a0: 0, door_a: core.door_a - core.a0, in_face: core.in_face }),
            storeys: from.storeys.clone(),
            lift: Some(Lift::standing(at, st.base)),
            floor_mat: floor::GRATE,
            wall_hue: CAR_HUE,
            wall_sat: CAR_SAT,
            floor_hue: (CAR_HUE + 160.0) % 360.0,
            light_hue: CAR_LIGHT_HUE,
            light_sat: 34.0,
            light_pitch: 1,
            light_along_x: from.ix != 0,
            light_phase: 0,
            beam_pitch: 3,
            // A car is a lit box you are standing in. There is no far end of it
            // to fall off into shadow, so the falloff is only there to keep the
            // near surfaces from flattening.
            ambient: 1.0,
            fall: 26.0,
            tallest: top_h,
            props: Vec::new(),
            cells: vec![blank; (wx * wz) as usize],
            to_exit: Vec::new(),
        };

        let shaft = Cell { height: top_h, win: fit::SHAFT, lit: 20, arch: 0, ..blank };
        let car_top = (st.base + lift::CAR_CLEAR).ceil() as u8;
        for a in 0..lift::CORE_W {
            for d in 0..lift::CORE_D {
                let (gx, gz) = it.world_of(a, d);
                let side = a == 0 || a == lift::CORE_W - 1;
                // A side wall alongside the CAR is the car's own wall — steel,
                // a handrail, floor to ceiling — and one alongside the WELL is
                // the shaft. They are different cells and they are different
                // things, and drawing them the same is what made the car
                // invisible from inside itself: at this range the car's own
                // structure is the only thing that says you are in a box.
                let cell = if side && d <= lift::GLASS_IN {
                    Cell { height: car_top, win: fit::WALL, lit: 12, ..blank }
                } else if side || d == lift::FACE_D {
                    // `arch` marks the wall at the BACK of the shaft — the one
                    // square to you as the car rises, and the only one that
                    // carries the storeys and their numbers. Outdoors `arch` is
                    // a roof shape; indoors, like `win` and `surface`, it is
                    // reinterpreted rather than paid for with a new field.
                    Cell { arch: u8::from(!side && d == lift::FACE_D), ..shaft }
                } else if d == 0 || d == lift::GLASS_IN {
                    Cell { height: 1, win: fit::WINDOW, lit: 70, ..blank }
                } else if d >= lift::WELL_D0 {
                    Cell { win: fit::WELL, ..blank }
                } else {
                    blank
                };
                if let Some(c) = it.at_mut(gx, gz) {
                    *c = cell;
                }
            }
        }
        for (a, d) in it.core.unwrap().landing() {
            it.door_cells[usize::from(d == lift::CAR_D1)] = it.world_of(a, d);
        }

        // The panel: two buttons, one at each end of the car. Which one is
        // under your hand is which one you are standing at, and the HUD says so
        // before you press. That is the whole of the interaction mechanism —
        // one bit of the input bitmask, and `interaction_near` deciding what it
        // means, exactly as it was built to.
        for (kind, a) in [(Fitting::CallUp, 1.2), (Fitting::CallDown, lift::CORE_W as f32 - 1.2)]
        {
            let (px, pz) = it.point_of(a, 1.5);
            it.props.push(Fixture {
                x: px,
                z: pz,
                bottom: 1.02,
                top: 1.54,
                kind,
                hue: CAR_LIGHT_HUE,
                seed: 0x11F7 ^ (st.floor as u32),
            });
        }

        it.name_car();
        it.retune();
        it.flood_from_the_door();
        it
    }

    /// **Move the car.** Everything about it that depends on where it *is* —
    /// the slab the camera stands on, the height its glass starts at, and
    /// whether the landing doors are a threshold or a wall — is written here,
    /// once a frame, over fifty cells. Nothing in the raycaster, the renderer
    /// or the collision ever asks where the car is; they read the same grid
    /// they read for a room.
    ///
    /// The doors are solid unless the car is standing level. That is the whole
    /// of the interlock, and it is why you cannot walk out of a moving lift.
    pub fn retune(&mut self) {
        let Some(l) = self.lift else { return };
        self.base = l.y;
        // The floor a car is ON is the floor it is passing, not the one it set
        // off from: on a four-storey ride `Lift::at` stays where the ride began
        // until it ends, and an indicator that reads FLOOR 0 all the way to the
        // ninth is worse than none.
        let passing = self.storeys[l.passing(&self.storeys)].floor;
        if passing != self.floor {
            self.floor = passing;
            self.name_car();
        }
        // A cell height is a whole unit; the sill this claims is read back off
        // `Interior::sill` rather than off the cell, so the glass does not jump
        // a unit at a time as the car rises. The cell height only has to be
        // non-zero — enough to be solid, and enough for the DDA to record it.
        let sill = ((l.y + lift::CAR_SILL).ceil() as u8).max(1);
        for d in [0, lift::GLASS_IN] {
            for a in 1..lift::CORE_W - 1 {
                let (gx, gz) = self.world_of(a, d);
                if let Some(c) = self.at_mut(gx, gz) {
                    c.height = sill;
                }
            }
        }
        let open = l.level();
        let in_face = self.core.map(|k| k.in_face).unwrap_or(0);
        for (a, d) in self.core.map(|k| k.landing()).unwrap_or([(0, 0); 2]) {
            let (gx, gz) = self.world_of(a, d);
            if let Some(c) = self.at_mut(gx, gz) {
                if open {
                    c.height = 0;
                    c.win = fit::FLOOR;
                    c.surface = floor::THRESHOLD;
                    c.door = 9 + in_face;
                    c.lit = 100;
                } else {
                    // Between floors the doors are shut, and what you are
                    // looking at is the back of them — the car's own leaves,
                    // not the shaft behind them.
                    c.height = (l.y + lift::CAR_CLEAR).ceil() as u8;
                    c.win = fit::LIFT;
                    c.surface = floor::GRATE;
                    c.arch = 0;
                    c.door = 0;
                    c.lit = 30;
                }
            }
        }
    }

    /// `LIFT / FLOOR n`. A car is the same car in every building, so unlike a
    /// room it does not take the building's name — what you want to read in one
    /// is which floor you are on.
    fn name_car(&mut self) {
        let mut n = 0usize;
        let mut label = [0u8; 40];
        for &c in b"LIFT / FLOOR " {
            label[n] = c;
            n += 1;
        }
        let mut d = [0u8; 4];
        let mut k = 0;
        let mut v = self.floor.unsigned_abs();
        loop {
            d[k] = b'0' + (v % 10) as u8;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            label[n] = d[k];
            n += 1;
        }
        self.label = label;
        self.label_len = n;
    }

    /// How far every open cell is from the doorway, in cells. A plain BFS, run
    /// once, over at most a few hundred cells — and the reason `way_out` cannot
    /// get stuck behind a rack.
    fn flood_from_the_door(&mut self) {
        self.to_exit = vec![u16::MAX; (self.wx * self.wz) as usize];
        let mut queue = std::collections::VecDeque::new();
        for &(dx, dz) in &self.door_cells {
            if let Some(i) = self.index(dx, dz) {
                if self.cells[i].height == 0 && self.to_exit[i] == u16::MAX {
                    self.to_exit[i] = 0;
                    queue.push_back((dx, dz));
                }
            }
        }
        while let Some((cx, cz)) = queue.pop_front() {
            let d = self.to_exit[self.index(cx, cz).unwrap()];
            for (ax, az) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, nz) = (cx + ax, cz + az);
                let Some(i) = self.index(nx, nz) else { continue };
                if self.cells[i].height != 0 || self.to_exit[i] != u16::MAX {
                    continue;
                }
                self.to_exit[i] = d + 1;
                queue.push_back((nx, nz));
            }
        }
    }

    #[inline]
    fn index(&self, gx: i32, gz: i32) -> Option<usize> {
        if !self.contains(gx, gz) {
            return None;
        }
        Some(((gz - self.z0) * self.wx + (gx - self.x0)) as usize)
    }

    #[inline]
    fn at_mut(&mut self, gx: i32, gz: i32) -> Option<&mut Cell> {
        if !self.contains(gx, gz) {
            return None;
        }
        Some(&mut self.cells[((gz - self.z0) * self.wx + (gx - self.x0)) as usize])
    }

    /// World cell for local (across the doorway, in from it).
    #[inline]
    fn world_of(&self, a: i32, d: i32) -> (i32, i32) {
        if self.ix > 0 {
            (self.x0 + d, self.z0 + a)
        } else if self.ix < 0 {
            (self.x0 + self.wx - 1 - d, self.z0 + a)
        } else if self.iz > 0 {
            (self.x0 + a, self.z0 + d)
        } else {
            (self.x0 + a, self.z0 + self.wz - 1 - d)
        }
    }

    /// **The glazing.** The wall the door is in is the one that faces the
    /// street, so it is the one that gets glass: piers on a pitch, and a glazed
    /// bay between every pair of them.
    ///
    /// Nothing about it is specific to *this* wall or to the ground floor — a
    /// bay is a cell whose height is a sill and whose tag is `WINDOW`, and the
    /// city answers for everything past it. Give a lift car four of these and
    /// it looks out on four sides while it moves.
    fn glaze(&mut self, st: &Style, r: &mut Rng) {
        let aw = if self.ix != 0 { self.wz } else { self.wx };
        // Between one and four bays to a pier: a workshop's slot windows and a
        // lobby's shopfront are the two ends of the same number.
        let bays = 1 + (3.0 * st.glazing).round() as i32;
        let pitch = bays + 1;
        let phase = r.below(pitch as u32) as i32;
        let sill_h = (self.base + SILL).round() as u8;
        let doors = self.door_cells;
        let hue = self.wall_hue;
        let mut glass = Vec::new();
        let core = self.core;
        for a in 1..aw - 1 {
            if (a - phase).rem_euclid(pitch) == 0 {
                continue; // a pier, holding the wall up
            }
            // The lift core takes its own frontage: from in here that is a
            // solid box, and the glass on it belongs to the car.
            if core.is_some_and(|k| k.holds(a, 0)) {
                continue;
            }
            let (gx, gz) = self.world_of(a, 0);
            if doors.iter().any(|&(dx, dz)| (dx, dz) == (gx, gz)) {
                continue;
            }
            glass.push((gx, gz));
        }
        for (gx, gz) in glass {
            if let Some(c) = self.at_mut(gx, gz) {
                c.height = sill_h;
                c.win = fit::WINDOW;
                c.hue = hue as u16;
                c.lit = 70;
            }
            self.windows.push(Window { x: gx, z: gz, sill: SILL });
        }
    }

    /// Put something solid in a cell, unless the cell is spoken for or is part
    /// of the way in. `h` is height above the floor slab. Returns whether it
    /// landed.
    fn place(&mut self, a: i32, d: i32, kind: u8, h: u8, hue: f32, sat: u8, lit: u8) -> bool {
        // The first few cells in from the door stay clear whatever else
        // happens: a room you cannot get into is worse than an empty one.
        if d < 3 {
            return false;
        }
        let (gx, gz) = self.world_of(a, d);
        let mat = self.floor_mat;
        let base = self.base as u8;
        match self.at_mut(gx, gz) {
            Some(c) if c.height == 0 && c.door == 0 => {
                c.height = base + h;
                c.win = kind;
                c.hue = hue as u16;
                c.sat = sat;
                c.lit = lit;
                c.surface = mat;
                true
            }
            _ => false,
        }
    }

    /// Furniture. The layout is the family's; the numbers are the building's.
    fn furnish(&mut self, st: &Style, r: &mut Rng, key: u32) {
        let (aw, dd) = if self.ix != 0 { (self.wz, self.wx) } else { (self.wx, self.wz) };
        let clear = self.ceiling.ceil() as u8;

        // Structural columns first, on their own grid — they are the building
        // standing up and are not negotiable with the furniture.
        if st.columns {
            let pitch = 5 + r.below(4) as i32;
            let pa = 2 + r.below(3) as i32;
            let pd = 4 + r.below(3) as i32;
            let mut d = pd;
            while d < dd - 2 {
                let mut a = pa;
                while a < aw - 2 {
                    self.place(a, d, fit::COLUMN, clear, st.wall_hue + 8.0, st.wall_sat as u8 + 10, 26);
                    a += pitch;
                }
                d += pitch + 1;
            }
        }

        match st.plan {
            Plan::Bands => {
                // Runs that go AWAY from the door with aisles between them, so
                // the first thing you see on walking in is a way through.
                let pitch = 3 + (key >> 11) as i32 % 3;
                let phase = r.below(pitch as u32) as i32;
                let run_h = run_height(st.run, r);
                let run_hue = fit_hue(st.run, st.light_hue, r);
                let mut a = 2;
                while a < aw - 2 {
                    if (a - phase).rem_euclid(pitch) != 0 {
                        a += 1;
                        continue;
                    }
                    // Break the run into segments so it is furniture and not a
                    // second wall.
                    let mut d = 3;
                    while d < dd - 2 {
                        let seg = 2 + r.below(5) as i32;
                        let gap = 1 + r.below(3) as i32;
                        for k in 0..seg {
                            if d + k >= dd - 2 {
                                break;
                            }
                            self.place(
                                a,
                                d + k,
                                st.run,
                                run_h,
                                run_hue,
                                40 + r.below(28) as u8,
                                8 + r.below(30) as u8,
                            );
                        }
                        d += seg + gap;
                    }
                    a += 1;
                }
            }
            Plan::Scatter => {
                let n = ((aw * dd) as f32 * st.density / 3.0) as u32;
                for _ in 0..n {
                    let kind = st.spots[r.below(st.spots.len() as u32) as usize];
                    let h = run_height(kind, r);
                    let hue = fit_hue(kind, st.light_hue, r);
                    let a = 2 + r.below((aw as u32).saturating_sub(4).max(1)) as i32;
                    let d = 3 + r.below((dd as u32).saturating_sub(5).max(1)) as i32;
                    // A cluster, not a dot: two or three cells in a row reads
                    // as a table or a sofa; one cell reads as a mistake.
                    let len = 1 + r.below(3) as i32;
                    let along = r.below(2) == 0;
                    for k in 0..len {
                        let (aa, dd2) = if along { (a + k, d) } else { (a, d + k) };
                        if !self.clear_around(aa, dd2) {
                            break;
                        }
                        self.place(aa, dd2, kind, h, hue, 44 + r.below(30) as u8, 10 + r.below(34) as u8);
                    }
                }
            }
            Plan::Open => {
                // A counter facing the door, off to one side, and then almost
                // nothing. The room itself is the thing you are looking at.
                let side = if r.below(2) == 0 { 3 } else { aw - 4 };
                let d0 = 5 + r.below(3) as i32;
                let len = 3 + r.below(4) as i32;
                for k in 0..len {
                    self.place(side, d0 + k, fit::COUNTER, 1, st.light_hue, 62, 44 + r.below(30) as u8);
                }
                let n = ((aw * dd) as f32 * st.density / 2.0) as u32;
                for _ in 0..n {
                    let kind = st.spots[r.below(st.spots.len() as u32) as usize];
                    let a = 2 + r.below((aw as u32).saturating_sub(4).max(1)) as i32;
                    let d = 4 + r.below((dd as u32).saturating_sub(6).max(1)) as i32;
                    if !self.clear_around(a, d) {
                        continue;
                    }
                    self.place(
                        a,
                        d,
                        kind,
                        run_height(kind, r),
                        fit_hue(kind, st.light_hue, r),
                        40 + r.below(30) as u8,
                        12 + r.below(30) as u8,
                    );
                }
            }
        }
    }

    /// Is there room to put something here without walling an aisle off? Every
    /// piece keeps a clear cell on at least two opposite sides.
    fn clear_around(&self, a: i32, d: i32) -> bool {
        let free = |a: i32, d: i32| {
            let (gx, gz) = self.world_of(a, d);
            self.at(gx, gz).is_some_and(|c| c.height == 0)
        };
        free(a, d) && ((free(a - 1, d) && free(a + 1, d)) || (free(a, d - 1) && free(a, d + 1)))
    }

    /// Signs, terminals, lamps: the things that carry a label rather than a
    /// footprint. Placed here, kept in the world model, drawn like street
    /// furniture.
    fn fixtures(&mut self, st: &Style, r: &mut Rng, key: u32) {
        // Over the door, always, and on the inside face. It is the one fixture
        // in the room that has a job rather than a character.
        let (dx, dz) = self.door_cells[0];
        let (ex, ez) = self.door_cells[1];
        self.props.push(Fixture {
            x: 0.5 * (dx + ex) as f32 + 0.5 + self.ix as f32 * 0.6,
            z: 0.5 * (dz + ez) as f32 + 0.5 + self.iz as f32 * 0.6,
            bottom: (self.ceiling - 1.35).max(1.9),
            top: (self.ceiling - 0.45).max(2.3),
            kind: Fitting::ExitSign,
            hue: 8.0,
            seed: key,
        });

        // And one over the lift landing, if there is one, on the face of the
        // core that the room can see.
        if let Some(core) = self.core {
            let (a, _) = core.landing()[0];
            let out = if core.in_face % 2 == 0 { -0.15 } else { 1.15 };
            let (px, pz) = self.point_of(a as f32 + out, 1.5 + lift::CAR_SILL);
            self.props.push(Fixture {
                x: px,
                z: pz,
                // Over the doors at door-head height, not up under the
                // ceiling the way the exit sign is: a lobby ceiling here can be
                // eight units up, and a sign you have to crane at is a sign
                // that is off the top of the frame when you are looking at the
                // doors.
                bottom: 2.62,
                top: 3.34,
                kind: Fitting::LiftSign,
                hue: 44.0,
                seed: key ^ 0x5A17,
            });
        }

        let (aw, dd) = if self.ix != 0 { (self.wz, self.wx) } else { (self.wx, self.wz) };
        let want = 3 + (key >> 3) % 7;
        let menu = [Fitting::Terminal, Fitting::Notice, Fitting::Lamp, Fitting::Plant, Fitting::Vent];
        for _ in 0..want {
            let kind = menu[r.below(5) as usize];
            let a = 2 + r.below((aw as u32).saturating_sub(4).max(1)) as i32;
            let d = 3 + r.below((dd as u32).saturating_sub(5).max(1)) as i32;
            let (gx, gz) = self.world_of(a, d);
            if !self.at(gx, gz).is_some_and(|c| c.height == 0 && c.door == 0) {
                continue;
            }
            let (bottom, top) = match kind {
                Fitting::Notice => (1.1, 2.3),
                Fitting::Vent => ((self.ceiling - 1.1).max(1.6), (self.ceiling - 0.25).max(2.0)),
                Fitting::Lamp => (0.0, 1.7 + 0.5 * r.f32()),
                Fitting::Plant => (0.0, 1.0 + 0.7 * r.f32()),
                _ => (0.0, 1.35),
            };
            self.props.push(Fixture {
                x: gx as f32 + 0.5,
                z: gz as f32 + 0.5,
                bottom,
                top,
                kind,
                hue: match kind {
                    Fitting::Plant => 108.0 + 30.0 * r.f32(),
                    Fitting::Lamp => st.light_hue,
                    Fitting::Terminal => 178.0,
                    Fitting::Notice => 48.0,
                    _ => st.wall_hue,
                },
                seed: r.next_u32(),
            });
        }
    }

    /// `<building name> <room word>`, and `FLOOR n` once there is a floor
    /// number worth saying. The sign over the door outside and the name of the
    /// room inside come off the same `building_of`, because it is the same
    /// building — nothing here is a fixed list of room names.
    fn name(&mut self, plan_id: u16, grain: i32, dx: i32, dz: i32) {
        let b = building_of(dx, dz, plan_id, BLOCK, grain);
        let mut n = 0usize;
        // `building_of` runs the building's NAME straight into its shop TYPE
        // and the room word replaces the type, so only the name comes across.
        for &c in b.label[..b.name_len].iter() {
            self.label[n] = c;
            n += 1;
        }
        while n > 0 && self.label[n - 1] == b' ' {
            n -= 1;
        }
        let mut label = self.label;
        let mut push = |s: &str| {
            for &c in s.as_bytes() {
                if n < 40 {
                    label[n] = c;
                    n += 1;
                }
            }
        };
        push(" ");
        push(self.room.word());
        if self.floor != 0 {
            push(" / FLOOR ");
            let mut d = [0u8; 12];
            let mut k = 0;
            let mut v = self.floor.unsigned_abs();
            if self.floor < 0 {
                push("-");
            }
            loop {
                d[k] = b'0' + (v % 10) as u8;
                k += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            while k > 0 {
                k -= 1;
                push(core::str::from_utf8(&d[k..k + 1]).unwrap_or("0"));
            }
        }
        self.label = label;
        self.label_len = n;
    }
}

/// How tall a piece of furniture stands above the floor, in whole world units
/// — the same units a building's height is in, because it is the same field
/// and goes through the same raycaster.
fn run_height(kind: u8, r: &mut Rng) -> u8 {
    match kind {
        fit::COUNTER | fit::DESK | fit::RAIL | fit::PLANTER => 1,
        fit::CRATE => 1 + r.below(2) as u8,
        fit::MACHINE | fit::PARTITION => 2,
        fit::RACK => 2 + r.below(2) as u8,
        fit::TANK => 3,
        fit::COLUMN => 4 + r.below(3) as u8,
        _ => 1,
    }
}

fn fit_hue(kind: u8, light: f32, r: &mut Rng) -> f32 {
    let jitter = 22.0 * (r.f32() - 0.5);
    (match kind {
        fit::PLANTER => 112.0,
        fit::TANK => 190.0,
        fit::MACHINE => 24.0,
        fit::RACK => 210.0,
        fit::CRATE => 34.0,
        fit::COUNTER | fit::DESK => light,
        fit::RAIL => 48.0,
        fit::PARTITION => 206.0,
        _ => light,
    } + jitter)
        .rem_euclid(360.0)
}

/// Where a building's entrance sits on its plot, if it has one.
///
/// A door is only offered where it can actually be reached and actually be
/// seen: the plot has to touch an avenue, and it has to be deep enough that
/// there is still a building behind the entrance bay once a cell is taken out
/// of it. Everything else — which of up to four street faces, and where along
/// that face — is a hash of the plot, so a building's door is in the same place
/// every time you walk down that street.
///
/// Returns `(face, a0)`: which side of the plot, and the coordinate along it of
/// the first of the two doorway cells. `face` is indexed the same way
/// `Cell::door - 1` is.
#[inline]
pub fn door_slot(
    bx: i32,
    bz: i32,
    plot: i32,
    lx0: i32,
    lx1: i32,
    lz0: i32,
    lz1: i32,
    built: i32,
    s: i32,
) -> Option<(u8, i32)> {
    let pw = lx1 - lx0;
    let pd = lz1 - lz0;
    if pw < 5 || pd < 5 {
        return None;
    }
    // Which of the four faces touch an avenue. Offset zero on either axis abuts
    // the previous block's avenue; offset `built` abuts this one's.
    let mut faces = [0u8; 4];
    let mut n = 0;
    if lx0 == 0 {
        faces[n] = 0;
        n += 1;
    }
    if lx1 == built {
        faces[n] = 1;
        n += 1;
    }
    if lz0 == 0 {
        faces[n] = 2;
        n += 1;
    }
    if lz1 == built {
        faces[n] = 3;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    // ONE hash for both decisions. This runs for two columns of every
    // street-facing plot on every frame the ground pass touches them, so the
    // difference between one hash and three is measurable on the street view —
    // and the two decisions are independent as long as they read disjoint bits.
    let h = hash3(bx.wrapping_mul(9391) + plot, bz.wrapping_mul(2237) + plot * 13, s ^ 0x40D);
    let face = faces[(h % n as u32) as usize];
    // Along the face: the doorway is two cells wide, and never in a corner.
    let (lo, hi) = if face < 2 { (lz0, lz1) } else { (lx0, lx1) };
    let span = hi - lo - 3; // two door cells and a cell of return at each end
    if span < 1 {
        return None;
    }
    let a0 = lo + 1 + ((h >> 11) % span as u32) as i32;
    Some((face, a0.min(hi - 3).max(lo + 1)))
}
