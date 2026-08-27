# Tools

`plates-example.txt` is sample data for the binary's `--plates-file` flag;
`serve.py` is a static file server for the optional browser build (`web/`,
produced by `./build-wasm.sh` from the repo root — see the root
[README](../README.md)).

The rest of this file is the commands behind the committed evidence frames in
[`../docs/frames/`](../docs/frames/), so a picture can be re-shot exactly.

## Building silhouettes

There is no flag for this — it is how the world generates. `world.rs::profile`
gives every tall plot one of six vertical profiles (flat, stepped, tapered,
crowned, spired, masted) out of the cells *within* the plot, because the world
is a height field and that is the only place an outline can come from.
`Cell::arch` falls out of the same decision, so a spire is a spire in outline
and not only in texture. The picture worth judging it from is the elevated
vista:

```bash
cargo run --release -- --vista --seed 703703 --eye 40 --pitch -0.10 \
    --at 4090,4062 --yaw 0 --no-plates --out docs/frames --name skyline-after
```

[`../docs/silhouettes.png`](../docs/silhouettes.png) is the committed before
and after, from the rooftop and from the street.

## Facade variety

`--variety 0..1` scales how much the facade generator is allowed to vary
between NEIGHBOURING plots. A `--seed` picks which mix of facades you get;
this picks how much mixing there is. It belongs to the native binary.

```bash
cargo run --release -- --variety 1      # the default: the look this city has always had
cargo run --release -- --variety 0.55   # choices shared across a 2x2 block district
cargo run --release -- --variety 0      # one 8x8 block district, one look

# the same view under two settings, which is the only fair comparison:
# --at fixes the camera, because the same seed at two settings is not
# quite the same city and a searched viewpoint would move.
cargo run --release -- --vista --seed 703703 --eye 34 --pitch -0.32 \
    --at 4090,4062 --yaw 0 --variety 0 --out docs/frames --name variety-0.0
```

At `1` every plot picks its own window lattice, colour family, roof shape and
plot split. Below that they are shared across a district: 1 block at 0.65, 2 at
0.45, 3 at 0.28, 5 at 0.12 and 8 at 0. Building **heights** are untouched at
every setting — the knob makes pattern and colour uniform, not the skyline.
[`../docs/variety.png`](../docs/variety.png) is the committed comparison.

## Registration plates

Every car on the road carries a registration plate, and the list is yours to
supply.

```bash
# a comma-separated list, straight on the command line
cargo run --release -- --plates "AB12 CDE,K9 PAW,1 RG"

# a file, one registration per line — what you want for a real list
cargo run --release -- --plates-file tools/plates-example.txt

# both together; either flag may also be repeated. All the entries pool.
cargo run --release -- --plates "BOSS 1" --plates-file tools/plates-example.txt

# do not draw plates at all
cargo run --release -- --no-plates

# write the three evidence frames: near, middle and far
cargo run --release -- --plate-shot --plates-file tools/plates-example.txt \
    --out docs/frames --name plates
```

[`plates-example.txt`](plates-example.txt) is the file format: one registration
per line, blank lines and `#`-comments skipped. Entries are folded to upper
case and anything a plate cannot carry — punctuation other than a separator —
is dropped, so `ab12-cde` and `AB12 CDE` are the same plate. A plate is cut to
10 characters.

**With no list given**, the traffic carries registrations **generated from the
`--seed`**. They are plausible-looking current-style patterns so the feature is
visible out of the box; they are **not real registrations** and are not claimed
to belong to anybody. Every mode that reports its plates says which of the two
it has.

A car keeps its plate for as long as it is on the road, and the same seed hands
the same cars the same plates. Close up a plate is real readable text — bold
black on plate yellow at the rear, bold black on white at the front, bordered
and with a margin, and two rows deep on a car tall enough to carry them; at
middle distance it is a plate-shaped panel carrying **no characters at all**,
because a half-drawn plate reads as a different registration; past about 70
units there is nothing.

`--plate-shot` drives the same simulation the game does, scores every frame on
the plates themselves — read off the grid's background plane, which nothing but
a plate ever paints — and writes the best frame for each of the three bands as
`.svg` and `.txt`. The near band scores on how many CHARACTERS a panel is
carrying rather than on how wide it is: since the panel took real plate
proportions the two are no longer the same question, and a two-letter private
registration now sits on a panel as wide as an eight-character one — which is
what a private plate looks like. It also prints which registrations from your
list appear verbatim in the frame's own characters, which is the honest
legibility check: the frame's characters are the characters a reader sees.
[`../docs/plates.png`](../docs/plates.png) is the committed evidence set and
[`../docs/plate-look.png`](../docs/plate-look.png) the before-and-after on the
panel's own look.

