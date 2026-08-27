# The shape of the engine

One library crate holds the whole engine. The binaries are thin: they read
keys, hand over an input bitmask and an elapsed time, ask for a frame, and
paint the flat buffer that comes back. Nothing about the picture is decided
outside `src/`.

```bash
cargo build --no-default-features   # the whole engine, standard library only
```

| | |
|---|---|
| `src/world.rs` | the city. No grid in memory — every cell is a pure function of its coordinate, so the city is unbounded and costs nothing to hold. Also `World::place`: which of the two we are in |
| `src/interior.rs` | insides. One room at a time, as a real grid, generated from the building it belongs to — doors, glazing, furniture, fixtures |
| `src/camera.rs` | walk, strafe, sprint, turn, look, eye height, collision |
| `src/project.rs` | field of view, horizon, and the screen-row maths |
| `src/raycast.rs` | one DDA per column, with the occlusion cull |
| `src/render.rs` | sky, ground, tower body, storefront, far skyline, roofs, population |
| `src/palette.rs` | colour, the ordered dither, the facade tables |
| `src/entities.rs` | pedestrians, traffic and their plates, and the weather |
| `src/output.rs` | 24-bit ANSI, plain text, SVG |
| `src/film.rs` | the film script: a duration and the keys held for it |
| `src/term/` | the terminal frontend (`tui` feature; the only dependency the crate has, `libc`) |
| `src/wasm.rs` | the optional browser target (`wasm` feature) |

## There is no world grid

`world.rs` computes every cell — height, ground surface, hue, saturation, lit
fraction, window lattice, architecture and the plan id that gives a building
its facade identity — as a **pure function of its global integer coordinate**.
Nothing is stored, so the city is unbounded in every direction and the process
holds a few megabytes whatever you do in it. You can walk for a week and never
leave it, and you can `--at 4090,4062` your way to a place you have never been
without generating anything on the way.

The layout is a **32-cell block**: the first 16 cells on each axis are the
built quadrant, the remaining 16 are the avenue. A straight avenue on an
infinite grid is an infinite corridor, and its vanishing point renders black
where there should be a skyline — which is why island towers stand in the
middle of the junctions, sized to leave clearance on both axes so they can
never box you in.

## Inside and outside are a mode, not a special case

`World::place` is either `Outdoors` — the city, still a pure function of
coordinate, still holding nothing — or `Indoors(Interior)`, one real grid of
one room. `World::cell` answers from whichever it is, so the raycaster, the
collision and the depth buffer all follow without knowing there are two of
them. The renderer picks its whole pass list off the same enum: sky / ground /
walls / props / population / rain out there, ceiling / floor / room walls /
fixtures in here.

See [Doors, rooms and windows](interiors.md).

## The wall pass marches near to far

Rendering far-to-near with only a depth buffer costs three times as much from
an elevated vista, where thousands of cells are in frame and almost all of
them are overdrawn. `render.rs::walls` marches near to far with a running
per-column `ybuf` instead, and the matching cull lives in `raycast.rs`.

## The projection

The raycaster hands out distances; turning a distance into a wall of a certain
height on a certain screen row is `project.rs`, and it lives inside the engine
rather than in a frontend so every frontend gets the same picture. Two things
there are load-bearing and easy to get wrong:

* **Pitch is a true camera rotation, not a horizon offset.** A horizon offset
  shears the picture; a rotation swings it.
* **The inverse — the world height seen at a screen row — carries the opposite
  pitch sign**, and ground depth takes the minus branch.

`row_span`, the per-row world-units-per-row derivative, feeds the texture
quantiser. Substituting the flat horizon value `1/proj_y` leaves the quantiser
one step too fine on distant walls and aliases the storeys away.

Vertical half-FOV is fixed at 0.35 rad (~40°); horizontal FOV follows from the
cell aspect, so a wide terminal widens the view rather than stretching it.

## Two sharp edges in the look

* Surface texture is an **8×8 ordered (Bayer) dither**, not hash noise. That is
  what lines the lit panes on one storey up with the lit panes on the next, so
  windows read as rows rather than as speckle.
* A building's bright vertical corner is drawn where the wall **segment**
  changes between screen **columns**, not at every sixth sub-column.

Both are the difference between a facade and vertical hash.

## The browser build (an optional extra)

```bash
rustup target add wasm32-unknown-unknown
./build-wasm.sh
python3 tools/serve.py        # then open /tools/web/
```

`--no-default-features --features wasm` drops the terminal frontend and with it
the only dependency the crate has, so the browser build is the engine and
nothing else: plain `extern "C"` exports and a view over linear memory, no
bindings generator. The page does four things — load the module, feed keys and
a `dt` in, paint the flat buffer that comes back, and size to the window. It
decides nothing about the picture.

The native terminal binary is the product. This is a target, not the
foundation.
