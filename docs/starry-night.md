# The Starry Night, alive

The second program on the engine, and the proof that the engine is one: it
uses `ascii-engine` and nothing else — not the city, and not an image crate.

```bash
cargo run --release -p starry-night                 # watch it in this terminal
cargo run --release -p starry-night -- --shade      # with shaded cells
cargo run --release -p starry-night -- --image my.jpg
```

Keys: `B` shaded cells on and off, `R` pixel it in again, `P` write the frame
to disk, `Q` or `Esc` to quit.

## The picture

The painting (1889) is in the public domain. On first run the program fetches
Wikimedia Commons' scan of the Google Art Project image, 960 px wide, with
`curl` (a 60-second cap, two retries, then a 1280 px fallback) into
`~/.cache/ascii-engine/starry-night.jpg`, and reads it from there afterwards.
Nothing is written into the repository. With no network, download it
yourself and pass `--image PATH`; a baseline JPEG or a binary PPM will do.

The JPEG is decoded by `src/jpeg.rs`, a baseline decoder written for this:
Huffman, 8-bit sequential DCT, any chroma subsampling, restart markers.
Progressive JPEGs are refused with a message saying so. Against an
independent decoder on the 500, 960 and 1280 px scans it differs by 1-2
counts in red and green and about 5 in blue on average — the difference is
chroma upsampling (replication here, interpolation there) — and the picture
is averaged down to terminal cells before any of that could show.

## The motion

`src/art.rs` is the whole of it, and it is three things:

- **It pixels in.** Each cell has its own arrival time, a well-mixed hash of
  its position spread over 3.6 seconds, and arrives as a hot, flickering
  character that cools into the painting's colour.
- **It flows.** Each cell samples the painting through a slowly drifting sum
  of sine waves — about two cells of reach once the picture has formed, a
  quarter of that while it is forming — so the swirls ripple.
- **It shimmers.** The glyph comes from the brightness the cell sees through
  that flow, so as the paint slides under a cell its character changes. Where
  the paint has a strong edge the glyph follows the brush stroke
  (`/ \ | -`) instead of its density.

Two things decide whether it reads as the painting at all. The picture is
averaged down to about two samples a cell before anything samples it, or the
brush strokes alias into noise; and a cell's brightness is judged partly
against its neighbourhood (`LOCAL_CONTRAST`), which is what turns the sky's
light and dark bands into different characters. Colour is lifted by a gain
rather than toward white, because a glyph inks a third of its cell at best.

## Recording it

```bash
cargo run --release -p starry-night -- --capture out/starry --frames 420
ffmpeg -framerate 30 -i out/starry/starry-%05d.svg -c:v libx264 -pix_fmt yuv420p starry.mp4
```

`--capture` steps an exact clock — frame `n` is `n / fps` seconds in — and
writes each frame through the engine's `capture::write_frame`. A captured
cell is 11 x 18 px rather than a terminal's 1:2, and the picture is fitted to
that shape (`--cell-aspect` overrides either).
