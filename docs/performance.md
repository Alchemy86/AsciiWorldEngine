# What a frame costs

```bash
cargo run --release -- --bench                 # on the street
cargo run --release -- --bench --indoors       # from inside a room
```

`--bench` reports **sim / cast / render / paint** separately, so a regression
can be attributed to a stage rather than to the frame as a whole. Terminal
*paint* cost measured through a pty is I/O backpressure, not the engine's cost;
`--bench` times the ANSI encode, which is the honest engine-side figure.

## Measured on the current build

600 frames at 180×60, Intel Core Ultra 7 265, release profile:

| stage | mean (ms) | p50 | p95 |
|---|---|---|---|
| sim | 0.009 | 0.009 | 0.010 |
| cast | 0.077 | 0.085 | 0.133 |
| render | 0.362 | 0.360 | 0.406 |
| paint | 0.136 | 0.136 | 0.153 |
| **total** | **0.584** | | |

That is a **1,710 fps ceiling** at 10,800 cells, with 223 visible cells kept
per frame by the occlusion cull — the same 223 as before the lift, because the
street is the same street. Peak resident memory over the run was **3.2 MB** —
there is no world grid to hold, so walking further costs nothing.

Indoors the same run gives **0.544 ms/frame**: a room cell is an array index
where a city cell is five hashes, and a wall two paces away ends a raycast
column on its first step. In a moving lift it is **0.611** — see the lift
below.

## Read absolute numbers with suspicion

`--bench` absolute numbers move with machine load by 30% or more, so a figure
quoted from a document is worth nothing on its own. Every delta below was
measured by building the before and the after and running them **interleaved in
one session**, which is the only way the difference means anything.

To get a "before": `git archive <ref> | tar -x -C <tmpdir>` and build there.

## What each piece cost when it landed

Street furniture, the sky and weather — 5 interleaved runs each, 180×60:

| | sim | cast | render | paint | total |
|---|---|---|---|---|---|
| before street furniture + sky + weather | 0.009 | 0.066 | 0.311 | 0.116 | 0.502 |
| with them, weather **clear** (the default) | 0.009 | 0.068 | 0.340 | 0.127 | 0.544 |
| weather **rain** | 0.012 | 0.069 | 0.358 | 0.133 | 0.572 |
| weather **downpour** | 0.015 | 0.069 | 0.389 | 0.135 | 0.608 |

So: **+8% of a frame with weather off, +14% in rain and +21% in a downpour**.
Clear is the default, so nobody pays for weather unasked.

Plates and the building profiles — 6 interleaved runs each, same machine and
session:

| | sim | cast | render | paint | total |
|---|---|---|---|---|---|
| before all three | 0.009 | 0.068 | 0.340 | 0.128 | 0.545 |
| with them | 0.009 | 0.075 | 0.347 | 0.130 | 0.561 |
| with them, `--no-plates` | 0.009 | 0.075 | 0.346 | 0.128 | 0.558 |

The cast column is the profiles: an occlusion cull has more varied heights to
work against. From the elevated vista, where the roofs are, it is unchanged.
Plates themselves are **+0.003 ms of a 0.546 ms frame**, and `--no-plates`
costs nothing at all.

Dressing the panel as a plate — the border, the margin, the second row, the
true yellow and the bold — added **nothing measurable on top**: six 600-frame
runs of each build back to back gave 0.588 ms/frame before against 0.586 after,
a 0.002 ms difference inside a 0.022 ms run-to-run spread. Sizing the panel to
its registration and setting the characters across it added nothing either: six
interleaved 400-frame runs gave **0.585 before against 0.583 after**, and the
plate feature itself measured **+0.009 ms before against +0.005 ms after**
against `--no-plates` on the same runs — both inside the spread.

Doors, interiors and see-through windows — 8 interleaved runs of 400 frames
each, same machine and session, 180×60:

| | sim | cast | render | paint | total |
|---|---|---|---|---|---|
| before doors and interiors | 0.009 | 0.076 | 0.362 | 0.133 | 0.577 |
| with them, on the STREET | 0.009 | 0.080 | 0.379 | 0.133 | 0.596 |
| with them, INDOORS | 0.001 | 0.029 | 0.403 | 0.070 | 0.504 |

Paired over ten back-to-back runs the street delta is **+0.019 ms/frame
(+3.4%)**, against a run-to-run spread of 0.007 ms on the same build.
**Indoors is cheaper than the street** — 0.504 against 0.596 — and that is not
a rounding artefact, for the reason above.

Of that delta, `cast` is real new geometry: an entrance bay is a notch in a
facade and an occlusion cull has less to work with. Drawing the doorway itself
and carving the bays were each measured at **nothing** by disabling them in
turn; what had cost +0.037 was `facade`/`ground_glyph` losing their inlining,
and `#[inline(always)]` is what took it back.

The lift — 10 interleaved runs of 400 frames each, same machine and session,
180×60, against the build immediately before it:

| | sim | cast | render | paint | total |
|---|---|---|---|---|---|
| before the lift, on the STREET | 0.009 | 0.082 | 0.364 | 0.131 | 0.587 |
| with it, on the STREET | 0.009 | 0.083 | 0.367 | 0.137 | 0.596 |
| before the lift, INDOORS | 0.001 | 0.030 | 0.397 | 0.093 | 0.522 |
| with it, INDOORS | 0.001 | 0.031 | 0.399 | 0.097 | 0.527 |
| with it, in a MOVING LIFT | 0.001 | 0.082 | 0.420 | 0.108 | 0.611 |

Paired per interleaved run: the street delta is **+0.009 ms/frame (+1.5%)**,
indoors **+0.005**, and riding a lift is **+0.016 over the street**.

Those deltas are small enough to be worth a control, so there is one: the same
build run in *both* slots of the same alternating pattern, ten pairs, comes out
at **+0.0004 ms/frame** with a per-pair range of ±0.006. The harness has no
slot bias, so the +0.009 is real measurement and not an artefact of going
second.

Real measurement, but **not real work**. Three fifths of it (+0.005) is in
`paint`, which is `grid_to_ansi` over the packed grid — a stage the lift does
not touch, encoding *byte-identical* output: `--vista` on six seeds and three
scripted `--capture` walks come out the same to the byte as the build before
it. `Cell` is still 12 bytes and `raycast::Hit` still 24; the only hot struct
that grew is `Camera`, 56 to 60, for the `ground` field, and that is one struct
touched once a frame. What moved is where the linker put things. It is the cost
of the binary being bigger, not of the street doing anything new — which is
also why it was +0.001 when the same feature was measured against an earlier
base.

Riding a lift is **0.611 ms/frame**. A shaft wall runs the whole height of the
building and the well has no ceiling over it, so more of the frame is wall and
less of it is sky. Reproduce with `--bench`, `--bench --indoors` and
`--bench --lift-bench` — and note that absolute numbers here move 30% or more
with what else is running on the machine, so only the interleaved pairs mean
anything.
