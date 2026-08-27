# The city walking itself

```bash
cargo run --release -- --demo     # or --wander
```

`--demo` is the attract mode: it paints to the terminal exactly as playing
does, but drives itself — wandering streets, turning at junctions when it runs
out of road, looking up at the towers, and every minute or so rising to the
rooftops and coming back down. It loops until you stop it.

**Any key hands you the controls**; `M` hands them back; `Q` quits.

It drives the same input bitmask your keyboard produces and reads the same
world the renderer does, so it is the real thing playing itself, not a
recording. Same camera, same collision, same projection, same population.

The only difference from [`--film`](film.md) is where the decisions come from:
the autopilot chooses for itself, a film script is told. A forced turn runs
until there is road ahead rather than for a fixed time, or it rocks side to
side in a corner for ever.
