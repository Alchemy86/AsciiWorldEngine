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
| cast | 0.077 | 0.085 | 0.130 |
| render | 0.360 | 0.361 | 0.401 |
| paint | 0.131 | 0.133 | 0.148 |
| **total** | **0.577** | | |

That is a **1,730 fps ceiling** at 10,800 cells, with 223 visible cells kept
per frame by the occlusion cull. Peak resident memory over the run was
**3.2 MB** — there is no world grid to hold, so walking further costs nothing.

Indoors the same run gives **0.540 ms/frame**: a room cell is an array index
where a city cell is five hashes, and a wall two paces away ends a raycast
column on its first step.

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
