# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Add durable project-specific notes here as they are discovered through real work.

## The engine (`Cargo.toml` + `src/`)

- **The product is the native binary.** `src/` is native Rust and there is no
  wasm on the default path; the browser target is an optional extra behind
  `--features wasm`, and nothing on the default path knows it exists.
- **The crate shape follows the sibling TerminalGB**
  (`~/Github/firstmate/projects/gameboy`): one lib crate holding the whole
  engine, real `[[bin]]` targets, feature-gated frontends. The core is
  dependency-free (`cargo build --no-default-features`); the `tui` feature pulls
  in exactly one crate, `libc`, for termios and `TIOCGWINSZ`.
- **`render.rs` and `palette.rs` are the whole look.** Every facade rule,
  falloff curve, dither threshold and ground rule is in those two files and
  nowhere else. Change the look there rather than compensating for it
  downstream, and re-shoot the affected pictures in `docs/` in the same commit.
- **Two sharp edges to keep.** Surface texture is an 8x8 ORDERED
  dither, not hash noise (that is what lines lit panes up into storey rows), and
  a building's bright vertical corner is drawn where the wall SEGMENT changes
  between screen COLUMNS, not at every sixth sub-column. Both are the
  difference between a facade and vertical hash.
- **There is no world grid.** `world.rs` computes every cell as a pure function
  of its global coordinate, so the city is unbounded and holds no memory — the
  other trade, a fixed window of cells slid along as you walk, costs tens of
  megabytes resident and buys nothing here. Anything that wants a cell just
  calls `World::cell(x, z)`.
- **The wall pass marches near to far with a running `ybuf`.** Rendering
  far-to-near with only a depth buffer costs 3x more from an elevated vista,
  where thousands of cells are in frame and almost all of them are overdrawn.
  See `render.rs::walls`. The matching cull is in `raycast.rs`.
- **A straight avenue on an infinite grid is an infinite corridor**, and its
  vanishing point renders black where there should be a skyline. The island
  towers standing in the middle of junctions (`world.rs::island_cell`)
  exist for exactly that reason; they are sized to leave clearance on both axes
  so they can never box the player in.
- **Measuring.** `cargo run --release -- --bench` reports sim / cast / render /
  paint separately, so a regression can be attributed to a stage rather than to
  the frame as a whole. Terminal *paint* cost measured through a pty is I/O
  backpressure, not the engine's cost — `--bench` times the ANSI encode, which
  is the honest engine-side figure.
- **`--bench` absolute numbers move with machine load by 30% or more**, so a
  figure quoted from a document is worth nothing on its own. Always measure the
  before and the after **interleaved in one run** and compare those. To get a
  "before", `git archive <ref> | tar -x -C <tmpdir>` and build there; do not use
  `git stash` (the worktree pool is shared).
  Last measured, 180x60, 5 interleaved runs each:

  | | sim | cast | render | paint | total |
  |---|---|---|---|---|---|
  | before street furniture + sky + weather | 0.009 | 0.066 | 0.311 | 0.116 | 0.502 |
  | with them, weather **clear** (the default) | 0.009 | 0.068 | 0.340 | 0.127 | 0.544 |
  | weather **rain** | 0.012 | 0.069 | 0.358 | 0.133 | 0.572 |
  | weather **downpour** | 0.015 | 0.069 | 0.389 | 0.135 | 0.608 |

  Plates + building profiles, 6 interleaved runs each, same machine and session:

  | | sim | cast | render | paint | total |
  |---|---|---|---|---|---|
  | before all three | 0.009 | 0.068 | 0.340 | 0.128 | 0.545 |
  | with them | 0.009 | 0.075 | 0.347 | 0.130 | 0.561 |
  | with them, `--no-plates` | 0.009 | 0.075 | 0.346 | 0.128 | 0.558 |

  The cast column is the profiles: an occlusion cull has more varied heights to
  work against. From the elevated vista, where the roofs are, it is unchanged.

  Doors, interiors and see-through windows, 8 interleaved runs of 400 frames
  each, same machine and session, 180x60:

  | | sim | cast | render | paint | total |
  |---|---|---|---|---|---|
  | before doors and interiors | 0.009 | 0.076 | 0.362 | 0.133 | 0.577 |
  | with them, on the STREET | 0.009 | 0.080 | 0.379 | 0.133 | 0.596 |
  | with them, INDOORS | 0.001 | 0.029 | 0.403 | 0.070 | 0.504 |

  The lift, 10 interleaved runs of 400 frames each, same machine and session,
  180x60:

  | | sim | cast | render | paint | total |
  |---|---|---|---|---|---|
  | before the lift, on the STREET | 0.009 | 0.082 | 0.364 | 0.131 | 0.587 |
  | with it, on the STREET | 0.009 | 0.083 | 0.367 | 0.137 | 0.596 |
  | before the lift, INDOORS | 0.001 | 0.030 | 0.397 | 0.093 | 0.522 |
  | with it, INDOORS | 0.001 | 0.031 | 0.399 | 0.097 | 0.527 |
  | with it, in a MOVING LIFT | 0.001 | 0.082 | 0.420 | 0.108 | 0.611 |

  Street **+0.009**, indoors **+0.005**, a moving lift **+0.016 over the
  street**. Verify with `--bench`, `--bench --indoors` and
  `--bench --lift-bench`.

- **A delta this small needs a null control, so run one.** The same build in
  BOTH slots of the same alternating pattern, ten pairs, comes out +0.0004
  ms/frame with a per-pair range of +-0.006 — so the harness has no slot bias
  and the lift's +0.009 on the street is real measurement. It is not real
  WORK: three fifths of it is in `paint`, a stage the lift does not touch,
  encoding byte-identical output. `Cell` is still 12 bytes and `Hit` 24; only
  `Camera` grew (56 -> 60, for `ground`). What moved is where the linker put
  things. The same feature measured **+0.001** against an earlier base, which
  is the whole argument for interleaved pairs and against quoting a figure.
  When the machine is loaded — another crew's `rustc` at 800% CPU — the same
  measurement returned a street delta of **-0.09**, which would mean this
  build made an untouched code path 15% faster. Check `/proc/loadavg` before
  believing a bench, and re-run it settled.

  Paired over ten back-to-back runs the street delta is **+0.019 ms/frame
  (+3.4%)**, against a run-to-run spread of 0.007 ms on the same build.
  **Indoors is CHEAPER than the street** — 0.504 against 0.596 — and that is
  not a rounding artefact: a room cell is an array index where a city cell is
  five hashes, and a wall two paces away ends a raycast column on its first
  step. Of that,
  `cast` is real new geometry — an entrance bay is a notch in a facade and an
  occlusion cull has less to work with. Drawing the doorway itself and carving
  the bays were each measured at **nothing** by disabling them in turn; what
  had cost +0.037 was `facade`/`ground_glyph` losing their inlining, and
  `#[inline(always)]` is what took it back.

- **Weather is a drawing, not a simulation.** Rain is a camera-centred disc of
  drops in `hsl(188, …)` with a streak on the near ones, dragged along with you
  and drawn through the same per-column wall buffer the pedestrians use, so it
  falls in front of the city and hides behind anything nearer. There is no
  world-sized array of drops and nothing off-screen is paid for. The star field
  is the same idea in the other direction: three magnitudes off a lattice hash,
  computed per frame, never stored.
- **Street furniture is enumerated, not searched for.** `World::props_near`
  walks the four known cross-offsets of an avenue at a known spacing along it —
  a few hundred candidates a frame — instead of scanning the ground for
  somewhere a lamp could stand, which would be tens of thousands of `cell()`
  calls. The cheap rejections (distance, the leave-a-gap hash) go **in front of**
  `cell()`, which is the only expensive call in that loop.
- Props are drawn as billboards through `Renderer::nearest`, the same
  per-column wall buffer the population uses, and are **not** solid: making a
  lamppost a `Cell` with height would make it a wall you cannot walk past and
  would break the raycaster's assumption that a column is a building. You can
  walk through a lamppost. That is a known trade, not an oversight.

## Inside and outside

- **It is a MODE, not a branch in the renderer.** `World::place` is
  `Outdoors` or `Indoors(Interior)`; `World::cell` answers from whichever, so
  the raycaster, the collision and the depth buffer are untouched, and
  `Renderer::render` picks its whole pass list off the same enum. If you find
  yourself adding an `if indoors` inside a drawing loop, the design has slipped.
- **A room is a real grid, and that is the right opposite trade.** The city is
  a pure function because it is unbounded; a room is bounded, there is one at a
  time, and its contents have state — so it is `Vec<Cell>`, about six kilobytes,
  and a lookup is an array index rather than five hashes. That is why
  `--bench --indoors` comes out CHEAPER than the street.
- **A window is `World::cell` falling through to the city.** A glazed bay is a
  cell tagged `fit::WINDOW` with a one-unit sill; above the sill it is clear, and
  anywhere the room's grid says nothing the CITY answers. So the same DDA
  carries out of the window into the real street at real distances, with real
  parallax, and `Renderer::room_walls` tells the two apart with one bounds check
  (`Interior::contains`). There is no second world and no backdrop. The test is
  `a_room_is_not_a_sealed_box_and_the_view_out_moves_with_you`; keep it.
- **The transition has no teleport in it.** A room is built in the SAME world
  coordinates as its doorway, so the camera is in the same cell before and
  after. `Cell::door` means the same thing on both sides — a threshold, and
  which way is in — which is what makes `Engine::portal` symmetric. Collision
  stops you `RADIUS` short of a solid face and `PORTAL_GAP` clears that; the
  other threshold plane is then most of a cell away, which is the hysteresis.
- **Two `Cell` fields are reinterpreted indoors and only indoors**: `win` is
  `fit::*` (what a solid cell IS) and `surface` is `floor::*`. That was chosen
  over adding fields because `Cell` is the currency of the raycaster and is
  returned by value on every lookup; it is still 12 bytes.
- **Obstacles are cells, fixtures are not.** A thing you cannot walk through is
  geometry and is baked into the grid, so collision and the raycaster handle it
  for free. A thing that carries a label, a verb and a reach is an
  `interior::Fixture` and lives in the world model where a HUD or an interaction
  can reach it — `Interior::interaction_near`. Nothing about a room is painted
  on by the renderer.
- **A floor, a wall and a ceiling each need their OWN hue, or a room reads as
  one dark haze.** They used to all draw from `room.wall_hue` at nearly the
  same lightness band, which is why a room was legible in outline but not in
  surface — you could see there was a room, not where the floor ended.
  `Interior::floor_hue` is `wall_hue` rotated a fixed 160 degrees (guarded by
  `a_room_reads_as_a_room_not_a_haze`, which asserts every family's floor and
  wall are at least 60 degrees apart on the wheel); the ceiling's unlit
  material is `render.rs::CEIL_HUE`, a fixed neutral, independent of the
  room entirely — the room's own colour on the ceiling plane comes from the
  LIT strips (`light_hue`), not the slab between them. Indoor lightness bases
  and ranges (`surface_of`, `floor_glyph`, `room_ceiling`) were widened
  alongside this — same distance-falloff shape, bigger swing — because hue
  separation alone still left every surface in a narrow brightness band.
- **Hue separation and light LEVEL are two different bugs, found in two
  passes.** The first pass (above) made a floor, a wall and a ceiling
  distinguishable at a fixed, still-dim brightness — differentiated but still
  unreadable. `Style::ambient` (in every family, `interior.rs`) then went from
  0.40..0.86 to 0.78..1.00 to actually light the room. It is deliberately NOT
  pushed to street level: an interior's saturation stays well under the
  city's (buildings roll `sat` 50..99 in `world::cell`; no indoor family goes
  over 34) and a measured frame still comes out a clear step down from a
  street frame's own mean/stddev. Stepping through a door should read as
  somewhere else, not as the same brightness in different wallpaper — matching
  the street exactly would be the wrong fix, not a more thorough one.
- **Indoor lattices are counted in CHARACTERS, not in fractions of a cell.**
  `palette::quant` floors at one world unit, which is right outdoors (a facade
  is across a street) and catastrophic indoors: a wall is an arm's length away
  and one unit is sixty columns, so a "panel joint" came out as a sixty-column
  stripe. `render.rs::surface_of` takes `cw`/`ch` — world units to one character
  — and draws a line where the two sides of a character disagree about which
  interval they are in. `palette::quant_fine` is the same power-of-two ladder
  continued below one unit, for the dither.
- **`Renderer::facade` and `ground_glyph` are `#[inline(always)]` and that is
  load-bearing.** Both were lifted out of `walls`/`ground` so the view through a
  window could reuse them; out of line they cost **+0.037 ms of a 0.57 ms
  frame** — more than the entire interiors feature. Measured, not assumed. Do
  not "tidy" the attribute away.
- **A room knows the way out of itself.** `Interior::to_exit` is a flood of
  cells-from-the-doorway, built once with the furniture in, and
  `Interior::way_out` walks down it. Steering straight at the door is NOT
  enough and `any_room_can_be_walked_out_of_without_a_map` found it twice — a
  rack between you and the exit turns "walk at the door" into walk, slide, turn
  back, for ever, and shoulder-check heuristics only move which room it happens
  in. Keep the test; it is the only thing standing between the attract mode and
  a wedge, and 70 seconds of real `--demo` never once wandered through a door,
  so this path will not be exercised by playing.
- **`--doorway` is the evidence tool**, next to `--plate-shot` and for the same
  reason: `--vista` and `--capture` pick their frame on the shape of the CITY,
  so whether an entrance is in view is luck. It finds a door, walks in with the
  real keys, prints the floor plan as ASCII and shoots five frames.
  `--bench --indoors` benches from inside and says what fraction of frames
  actually were.
- **The floor-number groundwork paid off exactly as written.** `Interior`
  carried a `floor` and a `base` slab height and nothing below that line assumed
  either was zero — `Cell::height` is absolute world height, the floor slab is
  drawn opaque, `World::max_height` is a method rather than `MAX_HEIGHT` inlined
  at the raycaster, and the glazing is a property of a CELL rather than of "the
  street wall". The lift is built on all four and changed none of them. See
  "The lift" below.

## The lift

- **A car is a room that moves, and that is why nothing else had to change.**
  `Place` is still `Outdoors | Indoors(Interior)`; the car IS an `Interior` with
  a `Lift` on it for the one thing a room does not have, a `base` that changes.
  The raycaster, the collision, the depth buffer and the renderer's pass list
  never learned lifts exist. If you find yourself adding a third arm to
  `Place` for this, look again at what is actually different.
- **The two glazed sides show two different things for the same reason a window
  does.** Outward, the car's grid stops and `World::cell` falls through to the
  CITY — and rising forty units up a shaft is the camera move the elevated vista
  already makes, so the street falls away underneath you correctly and for free.
  Inward is a well of open cells the DDA walks straight through to a wall at the
  back of the core.
- **The depth AND the width of the shaft are rendering requirements before they
  are architectural ones**, and both are written down in `lift.rs` because they
  do not look like requirements. The vertical field of view is ~40 degrees and
  the horizontal ~57: a surface an arm's length away shows about one world unit
  of its own height however tall it is, so a shaft wall right behind the glass
  is a stripe, not a floor. `CORE_D = 9` sets it back far enough for the cone to
  cover a storey and a bit; `CORE_W = 7` makes the well five units across so it
  fills two thirds of the frame instead of being a slot in a screen of dark side
  wall. Both were arrived at by shooting `--lift` and looking.
- **`render::shaft_glyph` is the only surface in the engine textured from the
  world model rather than from the cell it is on.** `Interior::storeys` is the
  building's own floor table — the same one each room is built from and the only
  heights the car may stop at — and it is keyed on ABSOLUTE world height. Key it
  on height above the car instead and you paper the shaft with a pattern that
  travels with you, which is the one thing the feature must not be.
- **The footprint half of the room generator does not take the storey number and
  the character half does.** `interior::fabric` splits them: `across`, `deep`,
  the entrance offset and the core come off a floor-blind key, so a shaft lands
  on the same cells floor after floor; the family, and with it the colours, the
  ceiling and everything in the room, come off a per-floor key. Floor zero's
  per-floor key IS the floor-blind key, which is what makes every ground-floor
  room in the city bit-identical to the build before lifts existed —
  `--vista` and `--capture` come out byte-for-byte the same, and `--doorway`
  differs on exactly the frames where a lobby now has a core in it.
- **Two reasons a tall building may still have no lift**, and both are the
  generator's: the height at its own entrance (`World::storeys`), and whether
  its frontage can hold the core clear of its own doorway
  (`interior::fabric`). About half of entrances get one, serving 4..10 floors.
- **The core stands hard by the entrance, and that is not only for wayfinding.**
  A room's frontage runs 14..25 cells on a block whose plots are often half
  that, and rooms have always overhung their plots — invisibly, because a wall
  out over the avenue looks like a wall. A lift core does not: at the far end of
  a wide frontage it would routinely be standing over the roadway with its doors
  opening on to mid-air. Beside the doorway it is as far inside the plot as the
  doorway is. Do not "tidy" it back to the middle of the wall.
- **`Camera::airborne` is measured from `Camera::ground`, not from zero**, and
  that is load-bearing rather than tidy. It gates collision; comparing the eye
  against street level meant a camera anywhere above the ground floor was
  "flying", so you walked through the walls of every room a lift can reach and
  out through the side of a rising car. It was the lift that found it, but the
  bug was already there for any storey above the first.
- **`Cell::door` grew a third meaning and no new field.** `1..=4` a street
  threshold, `5..=8` the wall behind one, `9..=12` a lift landing; the low two
  bits index `interior::INWARD` in all three, so `Engine::portal` reads which
  way is IN the same way every time. `Cell::arch` is reinterpreted indoors too —
  it marks the wall at the back of the shaft, the only one that carries storey
  numbers.
- **Interaction is ONE bit** (`camera::key::ACT`, bound to `X` and `Enter`),
  edge-triggered inside the engine so a frontend, a film script and a test all
  press a panel once for one press. What it MEANS is the world model's:
  `Interior::interaction_near` returns the nearest fixture within reach and
  `Engine::act` acts on it. The lift panel is two fixtures, one at each end of
  the car, so which button is under your hand is which one you are standing at —
  no second key, no menu, and the HUD says which before you press.
- **`--lift` is the evidence tool**, next to `--doorway` and `--plate-shot` and
  for the same reason: `--vista` and `--capture` pick their frame on the shape
  of the CITY. It finds the tallest lift building near the spawn, walks in with
  the real keys, walks into the car with the real keys and presses the panel
  with the real act bit. `--bench --lift-bench` benches a MOVING car and drives
  the panel rather than walking, because you do not walk in a lift.
- **Where the frames come from, and what they cost.** `docs/lift.png` is
  `--lift` at 180x60 through the committed recipe below. Numbers in "Measuring".

## Registration plates, and the grid's background plane

- **A plate is drawn out of CHARACTERS and paints nothing.** Body and edge are
  yellow ASCII (`#` rules, `+` corners, `|` uprights, `[` `]` on a one-row
  plate) and the registration is ordinary near-white characters in the space
  they leave. It was a painted rectangle and the medium was the objection: every
  other thing on this screen is a coloured glyph on black, so one filled block
  looked pasted on. Two things fell out of it and both are load-bearing:
  **none of the body characters may be one the bodywork uses** (`-`, `=`, `:`,
  `o`) or the plate's top and bottom dissolve into the back of the car — the
  first attempt used `=` and did exactly that; and there is **one colour front
  and rear** now, because a white frame on black is interface furniture and
  collides with the ink.
- **`Grid::plate` is how a plate is found**, not its colours. One byte a cell,
  set by `put_plate`, cleared by `put` when something overdraws. Three things
  read it: the rain pass (a drop through a registration reads as a different
  registration), `--plate-shot`, and
  `a_plate_on_screen_is_never_a_registration_other_than_its_own`.
- **`Grid`'s background colour plane is now UNUSED.** It existed for one thing —
  black characters on yellow — and that thing is gone. It costs nothing while it
  is black (`has_panels` is false on every frame, so all three output paths skip
  it), so it was left rather than torn out of ANSI, SVG and the wasm ABI in the
  same breath as a change to how a plate looks. Bold went with it: weight came
  off the panel flag, and there is no panel.
- **Plates are drawn in a SECOND pass, after every vehicle body is in the depth
  buffer, and they are all-or-nothing.** Both rules are load-bearing and both
  were put there by an observed failure, not by caution: a plate clipped by a
  building corner or by a car drawn later came out as `1 R` when the car's
  registration is `1 RG`, which is the one outcome worse than having no plates.
  When the whole run is not clear the plate falls back to the empty plate, and
  rain will not overwrite a plate cell (`Grid::is_plate`). `a_plate_on_screen_is_never_a_
  registration_other_than_its_own` walks 900 frames in a downpour and asserts
  every panel is either blank or one whole registration; keep it.
- **A plate is drawn as a PLATE, not as a highlighted word**, and every part of
  that is deliberate: one character of its own body at each end of the
  registration's row (`palette::PLATE_PAD`, now two cells, formerly four when a
  dark edge and a clear margin were doing the job an upright does by itself);
  `PLATE_ASPECT` cells of width per row of height, which is a real plate's 4.7:1
  corrected for a cell that is not square; and a second or third row on a car at
  least `PLATE_TWO_ROW_SPAN` / `PLATE_THREE_ROW_SPAN` tall, which is what makes
  it read as an object. The candidate walk in `plate_on` tries
  deepest-and-widest first and steps DOWN a row when the block is not clear, so
  occlusion costs the plate its shape rather than costing the car its
  registration.
- **The panel is sized to the registration, and the registration is SET across
  the panel.** `PLATE_ASPECT` gives the width that height of plate wants to be;
  `Plate::settings` gives the three even pitches the registration can be set at
  — tight, group gap opened, every character one apart — and `plate_on` picks
  whichever of those lands NEAREST the wanted width, then `Plate::set_into`
  lays it down. So the panel width is always a registration width, never a
  fixed cell count, and `a_plate_on_screen_is_never_a_registration_other_than_
  its_own` asserts the panel is `PLATE_PAD` cells wider than what it carries and
  not one cell more. Three things about that are load-bearing:
  - **Only three pitches, and all of them even.** One blank between characters
    is a plate set wide; two is two words. And a registration with one pair
    touching and the rest apart reads as a typo, which is why the choice is a
    pitch and not a per-gap fit.
  - **The rear plate and the front plate are ONE path.** `rear` is used in
    exactly one place in `plate_on` — picking the colour pair. If a white panel
    and a yellow panel on the same frame come out different widths, that is the
    ROW COUNT, not the colour: seed 11 step 162 on the build before this had a
    white one-row 12-cell panel and a yellow two-row 16-cell panel carrying the
    SAME registration. Measure before believing a front/rear difference.
  - **`PLATE_SHAPE_NUM/DEN` is why two rows did not simply get narrower.** A
    short private registration cannot fill a 16-cell panel, and a two-row panel
    squeezed to its width is a square rather than a plate; below three quarters
    of `rows * PLATE_ASPECT` the walk steps down to the one-row strip, which at
    that width is the right shape for it.
  - The **one-row** panel is unchanged from before this work, to the cell,
    because `settings[0] + PLATE_PAD` is what `max(PLATE_ASPECT, need)` already
    gave for any registration long enough to need it. That was deliberate: it
    is the shape that was judged correct, so it is the baseline, not the
    target.
- **A plate does not fade to black the way the rest of the frame does.**
  `PLATE_FLOOR` holds a drawn panel at 74% brightness; the scale is applied to
  all three channels at once, so hue and saturation are untouched and a distant
  plate is a dimmer yellow rather than a brown one. Plate yellow is
  `[255, 204, 0]` — BS AU 145d — because the saturation is what makes a plate
  recognisable before anything is read off it.
- **Weight used to come off the panel flag in all three frontends** — `SGR 1`,
  `font-weight: 900` plus a stroked outline in the SVG, `strokeText` on the
  canvas. There is no panel now, so there is no bold, and the plate reads by its
  SHAPE instead: a rectangle closed on all sides is a stronger "this is an
  object" signal than weight ever was at one character per cell.
- `--plate-shot` is the evidence tool. It scores frames on the background plane
  — nothing but a plate ever paints one — and prints which registrations from
  whatever list is actually on the road (`eng.pop.plates`, not the raw
  `--plates` argv) appear on the frame, which is the honest legibility check.
  "Appear" means any of the three settings of `Plate::settings`, not the tight
  one alone, because `RT08 AAR` on a near car is drawn `RT08   AAR`; a plain
  `contains` of the supplied string reports a false negative and did. The near
  band scores on the CHARACTERS a panel carries, not its width. `--at X,Z`
  fixes `--vista`'s camera, which is the only way two settings of `--seed` or
  `--variety` are comparable.
- **The traffic's default registrations are committed data, in
  `src/registrations.txt`, `include_str!`'d into `palette::Plates::default_set`
  — not a string literal in the renderer and not generated from the seed.**
  `entities::Population::new` calls `default_set()` directly, so a plain
  `cargo run --release` / `./play` with no flag carries that list; `--plates`,
  `--plates-file` and `--no-plates` all still override it exactly as before.
  `palette::PlateSource` (`Generated` / `Default` / `Supplied`) replaced the old
  `Plates::generated: bool` so the on-screen note and `--plate-shot` can tell a
  real committed list apart from both a CLI-supplied one and the seed-derived
  placeholder patterns (the placeholder path only remains reachable if
  `registrations.txt` were ever left with nothing usable in it). The file
  lives under `src/` rather than a top-level `data/` specifically so `./play`'s
  own staleness check (`find src Cargo.toml -newer target/release/asciicity`)
  picks up an edit to it without needing a change to `play` itself.
- **Measuring a plate from the SVG beats reading it off a picture.** The panel
  is a `<rect>` whose `fill` is `#ffcc00` (rear) or `#f6f6f0` (front) and whose
  width is cells x 11px; the registration is the `<text>` with `font-weight="900"`
  on the same row, and a second `<rect>` one row above says the panel is two
  rows deep. `grep -o '<rect[^>]*fill="#ffcc00"[^>]*/>'` on a `--plate-shot`
  `.svg` is how the before/after numbers in `docs/plate-size.png` were got.

## Facades: variety, and building profiles

- **A seed picks WHICH mix of facades; `--variety` picks HOW MUCH mixing.** It
  is a district grain (`world::grain_for`): the lattice, colour family, roof
  shape and plot split are keyed on `World::style_key` / `district` instead of
  on the plot as it falls. `--variety 1` is the default and is proven
  cell-for-cell identical to the old generator by a test — keep that test, it
  is what makes the knob safe to touch.
- **`world.rs::profile` is where a building's outline comes from.** The world is
  a height field, so an outline can only be made out of the cells WITHIN a plot;
  `profile` gives a tall plot one of six vertical profiles off the ring index.
  Two things in it are not decoration: `Cell::arch` comes out of the same
  decision as the height, so a spire is a spire in outline and not only in
  texture; and a plot with a courtyard is never offered a spire, crown or mast,
  because it has no middle cells to stand one on and was otherwise textured as
  a spire while coming out flat. A test asserts both.
- The mix is the design. About a third of tall stock stays flat on purpose and
  everything under 20 units is flat or nearly so — the old comment's worry
  about a field of ziggurats is legitimate, and the answer to it is the range,
  not the absence of profiles. Judge from the elevated vista
  (`docs/silhouettes.png`), which is where a skyline is.

## Evidence frames: how the committed pictures are made

- `.svg` out of `--vista` / `--capture` / `--plate-shot`, then
  `google-chrome --headless --disable-gpu --no-sandbox --window-size=W,H
  --screenshot=out.png file://.../frame.svg`, then ImageMagick `magick` (not
  `convert`, deprecated in IMv7) to crop, label and `-append` the comparison.
  `docs/plates.png`, `docs/plate-look.png`, `docs/plate-size.png`,
  `docs/variety.png`, `docs/silhouettes.png`, `docs/interiors.png`,
  `docs/doorway-street.png` and `docs/lift.png` were all built that way.
  `docs/lift.png` has a committed recipe: `--lift --out DIR`, then each of the
  six `.svg` through headless Chrome at `1980,1090`, then
  `magick FRAME -resize 900x -bordercolor black -border 6 -background black
  -fill '#8fd3c8' -pointsize 22 -font "$(fc-match -f '%{file}' 'DejaVu Sans')"
  -gravity northwest label:"CAPTION" -gravity center -append` for each, then
  `+append` in pairs and `-append` the three rows.
- **Give headless Chrome the frame's real height.** An SVG frame is
  `cols * 11` by `rows * 18` px; `--window-size=1980,700` on a 60-row frame
  silently CROPS the bottom third and the picture looks wrong in ways that
  send you back into the renderer. 180x60 wants `1980,1090`.
- **`magick ... -depth 8 -resize 1400x -strip`** before committing: the raw
  composites come out 16-bit and 19 MiB.
- **A before-and-after is only worth anything if it is the same frame.**
  `docs/plate-look.png` works because `--plate-shot` picked step 1196 on both
  builds, so the two crops are the same two cars in the same place; check the
  `<title>` in the two `.svg`s before cropping, and if the steps differ, say so
  rather than presenting two different frames as a comparison.
- **A committed comparison goes stale when the generator changes.** The plate
  frames had to be regenerated after the building profiles landed. If you change
  `world.rs` or `render.rs`, re-shoot every picture in `docs/` that shows the
  thing you changed.
- `docs/interiors.png` is stale as of the floor/wall/ceiling hue-separation
  work above — its colours predate `floor_hue` and `CEIL_HUE`. Re-shoot it
  with `--doorway` next time interiors are touched; it was not re-shot here
  because there was no committed recipe for its exact crop/label/append, only
  the general method above.

## Input: what a terminal can and cannot tell you

- **The whole problem is that a terminal without the kitty keyboard protocol
  sends presses only.** Two consequences, and they need separate fixes:
  the keyboard goes *silent* for the entire OS autorepeat delay (250–660 ms)
  after the first press with the finger still down; and only ONE key repeats, so
  a second key silently kills the first. Lengthening a hold window cannot fix
  either — a window long enough to cover the silence also outlasts letting go.
- The fixes, all in `src/term/input.rs` and `Camera::glide`: **measure** the
  autorepeat delay off the player's own first hold and expect the silence it
  makes; let a key whose repeat cadence proves a finger is down survive losing
  that repeat *only when another key was pressed on top of it* (the one signal
  that separates "I let go" from "I pressed something else"), bounded to
  `LATCH_MAX`; and give the camera a short glide so movement is a velocity.
- **`tmux 3.7b` ships `extended-keys off`**, so every tmux user lands on the
  fallback path. `tmux set -g extended-keys on` fixes it. The binary detects
  this and says so on screen, and again on exit.
- **Test input as a model, not through a terminal.** `Keyboard::press` /
  `expire` take the state machine apart from the byte reader, so the tests in
  `term/input.rs` replay a synthetic autorepeat stream against it directly. For
  end-to-end *feel*, drive the real binary through a `pty` and read the HUD:
  `yaw` (2 dp) and `eye` (1 dp) have far better resolution than position and are
  the honest probes. Two traps found the hard way: a replay harness must not
  play events past the moment it probes, and a pty reader that runs a regex over
  a growing buffer stalls itself and shows up as a fake stutter in the program.

## The self-playing mode

- `--demo` / `--wander` is an attract mode that drives the same input bitmask a
  keyboard produces and reads the same `World` the renderer does — so it
  exercises the real camera, collision and projection. Any key hands control
  back, `M` hands it over again.
- Two failures it has to avoid, both fixed and both easy to reintroduce:
  a descent must run until the eye has actually **landed** (the eye eases toward
  its target, so a timer ends it early and strands the camera in mid-air with
  collision off), and it must only *start* over a **street** cell
  (`cell.cross != 255`) — a block's interior courtyard is open ground too, and
  landing in one drops the walk into a pocket it cannot get out of. A forced
  turn also runs until there is road ahead, not for a fixed time, or it rocks
  side to side in a corner for ever.

## Recording a film

- **`--film` is the recording mode and it is the only one.** `--vista` searches
  for a dramatic sightline and MOVES the camera to it, so consecutive `--vista`
  frames teleport, and it never touches the weather — `--weather rain` is
  silently ignored on that path. `--capture` writes one frame per script
  segment, so it is a contact sheet. Approximating the vista deck with
  `--eye 6` is not the view `V` gives; `EYE_VISTA` is 34.0. Anything that wants
  a moving picture wants `--film`.
- **It is the autopilot's mechanism with a script instead of a policy.**
  `--demo` already drove the engine correctly — the same `camera::key` bitmask,
  live camera, collision, projection, population — but it chooses for itself, so
  it cannot be asked for a shot. `--film` keeps the mechanism, takes the script
  (`src/film.rs`), and reuses `--capture`'s `write_frame` untouched. `vista` in
  a script calls the same `toggle_vista` the `V` key calls; `weather` calls the
  same setter `--weather` does. Nothing sets an eye height, a pitch or a
  position behind the engine's back, and that is the property to keep.
- One tick is one frame and `--fps` (default 30) is both the engine `dt` and the
  playback rate, so a walk on screen lasts as long as the walk in the script.
  `walk` is `WALK_SPEED` = 3.2 units/s — a 20-second walk covers 64 units, which
  is the arithmetic to check a film against if it ever looks sped up.
- **ffmpeg here reads the `.svg` frames directly** — it is built
  `--enable-librsvg` (`ffmpeg -decoders | grep svg`). No chrome-per-frame step,
  which at ~1.5 s a frame would be minutes for a 30-second film. 960 frames at
  180x60 (1980x1080) encode in about 80 s. `--film` prints the exact command.
- **Rain is a STREET-level feature and a skyline hold is dry.** `RAIN_CEILING`
  is 12.0 and `EYE_VISTA` is 34.0, so above the rooftops there is nothing to
  draw. Measured: on the built-in reel in `--weather rain`, drops appear on
  frames 1–662 and stop the moment the eye clears the ceiling, and come straight
  back on a `sink`. This is the engine, not the recorder. Do not "fix" it in the
  recorder.
- **Proving rain is drawn, rather than assuming the flag worked.** The drop
  colours in `render.rs::rain` are fixed and appear nowhere else in the frame:
  `#3eb9cc` `#34828d` `#2a5055` near/mid/far and `#2b656e` `#1f4a51` for the
  streak tails. Grep the written `.svg`s for them, and shoot the SAME script
  with `--weather clear` as the control — that run must score zero. On the
  built-in reel: 142,133 drop runs over 668 of 960 frames wet, 0 over 960 dry.

## Documentation and brand

- **Licence: PolyForm Noncommercial 1.0.0, not all-rights-reserved.** `LICENSE`
  holds the license text byte-for-byte from `polyformproject.org` (fetched via
  the GitHub API's git-blob endpoint, not typed from memory or paraphrased) —
  never hand-edit the body; PolyForm's own terms require removing every mention
  of "PolyForm" from a text that has been changed. The plain-English summary
  above the `---` separator is explicitly a non-binding note, not a licence
  term. `Cargo.toml` uses `license = "PolyForm-Noncommercial-1.0.0"` (the exact
  SPDX id — check `spdx/license-list-data`'s `json/details/` if it is ever
  bumped), not `license-file`.
- **The README is a shop window, not a manual.** Logo, thanks, summary, key
  details, one-command run, controls, pictures, examples, links, licence — and
  nothing else. Anything longer than a paragraph belongs on a page in `docs/`
  linked from the "Deeper" table. Keep it under ~120 lines.
- **`./play` is the one-command entry point** and it is what the README tells
  people to run: it finds or offers to install a Rust toolchain, builds release
  only when `src/` or `Cargo.toml` is newer than the binary, and `exec`s the
  binary with every argument passed through. Test it from a clean tree
  (`git archive` into a tmpdir) after touching it, non-TTY included.
- **The brand mark is generated, never hand-edited.** `docs/brand/generate.py`
  is the only source of truth for `asciiworldengine-logo.svg` and
  `asciiworldengine-icon.svg`; re-render `docs/brand/preview/*.png` in the same
  commit and LOOK at them. The accent `#34f4ee` is `hsl(178, 90%, 58%)`, the
  `NEON_GRID` frame hue from `palette.rs` — if the palette's signature hue ever
  moves, the mark moves with it. See `docs/brand/README.md`.
- **This repo's own history is not what gets published.** The
  `Alchemy86/AsciiWorldEngine` mirror carries a SINGLE fresh commit of the
  current tree, by design — push a squashed tree there, never this repo's real
  history. `tools/web/asciicity.wasm` is our own `build-wasm.sh` output and is
  the only `.wasm` in the tree.
- **Front-page pictures must be captures of the current build.** `city-street`,
  `city-vista`, `city-interior` and `city-rain` in `docs/frames/` are all seed
  90210 at 180x60 so they read as one city; re-shoot all four together. The
  before/after composites in `docs/` (`silhouettes`, `variety`, `plates`,
  `plate-*`, `doorway-street`, `interiors`, `life-and-feel`) are historical
  evidence for their own doc pages and are deliberately NOT re-shot.

## Maintaining this file

Keep entries short, factual and project-intrinsic: things a future session
would otherwise have to rediscover. Prefer a pointer to the authoritative file,
command or doc over copying detail that the codebase already carries. Delete
entries that stop being true.
