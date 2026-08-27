# Recording a film

`--film` records the engine playing itself. It writes a numbered frame for
**every tick**, so the frames are a film rather than a contact sheet, and it
reaches the engine only through the `camera::key` bitmask a keyboard produces —
live camera, live collision, live weather, live traffic.

```bash
cargo run --release -- --film --weather rain --out frames --name film
ffmpeg -y -framerate 30 -start_number 1 -i frames/film-%06d.svg \
  -c:v libx264 -pix_fmt yuv420p -crf 18 film.mp4
```

That is the whole pipeline — `ffmpeg` reads the SVG frames directly if it was
built with librsvg (`ffmpeg -decoders | grep svg`), and `--film` prints the
command for you with your own paths in it. At the default 180×60 a frame is
1980×1080 px.

## The script

With no `--script` it plays a built-in reel: stand, walk down the avenue, look
up at the towers, `V`, hold on the skyline. `--print-script > myfilm.txt`
writes that reel out to edit. A script is one beat per line — a duration, then
whatever is held down for it:

```text
2s   wait
7s   walk                 # 3.2 units/s, the same walk as at the keyboard
4s   walk look-up         # up at the towers as we pass them
3s   walk level           # horizon back down
1s   vista wait           # press V
9s   wait                 # hold on the skyline
```

| | |
|---|---|
| duration | `4s` seconds (the default unit), `250ms`, or `120f` exact frames |
| held for the beat | `walk` `back` `sprint` `wait` · `strafe-left` `strafe-right` `turn-left` `turn-right` · `look-up` `look-down` `level` · `rise` `sink` |
| once, at the start of the beat | `vista` · `weather clear\|rain\|downpour` |

Blank lines and `#`-comments are ignored.

One tick is one frame, so at `--fps` playback the film runs at the speed it was
walked; nothing is sped up. `--seed`, `--weather`, `--variety`, `--plates` and
`--cols`/`--rows` all apply as they do to any other run, and `--at X,Z` /
`--yaw` set where the film starts. Everything else — `--script`, `--fps`,
`--out`, `--name` — is in `--help`.

## Why not the other capture modes

`vista` **in a script presses `V`**, so the eye rises to the observation deck
the way it does under your finger; it does not set an eye height. That is the
difference from `--vista`, which searches for a dramatic sightline and jumps
the camera to it — right for one picture, a teleport between two — and which
never applies the weather at all. `--capture` writes one frame per script
segment, so it is a contact sheet. Anything that wants a moving picture wants
`--film`.

Approximating the vista deck with `--eye 6` is not the view `V` gives;
`EYE_VISTA` is 34.0.
