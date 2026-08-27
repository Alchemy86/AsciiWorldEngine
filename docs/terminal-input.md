# Holding a key down

In a terminal that speaks the **kitty keyboard protocol** (kitty, ghostty,
foot, WezTerm, recent Alacritty) you get real press *and release* events: keys
are held exactly as long as your finger is on them, any number at once, and
there is nothing to solve.

Everywhere else — including **inside tmux unless `extended-keys` is on** — a
terminal only ever sends *presses*, padded out by the OS autorepeat. Two things
follow, and both make a walker feel broken:

* the keyboard goes silent for the whole autorepeat delay (250–660 ms) after
  your first press, with your finger still down; and
* only one key repeats at a time, so pressing a second key silently kills the
  first — you cannot walk and look around.

## What the fallback does

Not "make the hold window longer" — that cannot work, because a window that
covers the silence also outlasts letting go. Instead:

* the autorepeat delay is **measured off your own keyboard** on your first hold
  and then *expected*, so the silence it makes is ridden out rather than
  mistaken for a release;
* a key whose repeat cadence proves a finger is down **survives losing that
  repeat** to a second key, for a bounded couple of seconds — long enough to
  turn a corner or look up at a tower while still walking; and
* movement is a velocity with a short glide, so it starts and stops as a
  movement rather than a teleport.

If the protocol is missing, the game **says so on screen when it starts**, with
the exact fix for your situation, and prints it again on the way out. `Tab`
locks a walk on, which needs no key held at all and so works the same in both
modes. The HUD always says which mode you got.

The whole of it is in `src/term/input.rs`.
