#!/usr/bin/env python3
"""Generate the AsciiWorldEngine brand SVGs.

Every letterform is hand-drawn here as a stroked skeleton path — **no font is
embedded, subset or traced**, so there is no third-party licence in these
files.  The panel, the alphabet, the tight tracking, the accent full stop and
the grey wide-tracked tagline are the house style shared with the sibling
projects; the *motif* and the *accent colour* are this engine's own.

Run from anywhere:  python3 docs/brand/generate.py
It rewrites `asciiworldengine-logo.svg` and `asciiworldengine-icon.svg`
deterministically.  Edit this file, never the SVGs by hand.
"""

import os

OUT = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------------------
# Palette.  Panel, border, wordmark white and tagline grey are the house
# values, unchanged — that is what makes this a sibling mark rather than a
# different project.  The ACCENT is this engine's own: `hsl(178, 90%, 58%)`,
# the `NEON_GRID` frame hue out of `src/palette.rs`, which is the teal that
# dominates every capture in `docs/frames/`.  The mark and the product agree.
BG = "#0d1117"      # near-black panel (matches GitHub dark, works on light)
EDGE = "#30363d"    # faint panel border so the card reads on pure black too
FG = "#f0f3f6"      # wordmark white
GREY = "#8b949e"    # tagline grey
CYAN = "#34f4ee"    # hsl(178, 90%, 58%) — the engine's own neon

TAGLINE = "A WALKABLE CITY IN YOUR TERMINAL"

# ---------------------------------------------------------------------------
# Stroke-skeleton capital letters.  Cap height 100, stroke 26 (half-stroke 13);
# every endpoint is inset 13 so round caps land on the ink edge.  The value is
# (advance width, list of path data).
#
# These are the shapes the sibling marks already draw, unchanged, so the
# wordmarks are visibly the same alphabet.  K is new here and is drawn to the
# same rules: same cap height, same inset, same stroke.
S = 13
GLYPHS = {
    'T': (72, ["M13 13 H59", "M36 13 V87"]),
    'E': (56, ["M43 13 H13 V87 H43", "M13 50 H36"]),
    'R': (68, ["M13 87 V13 H36 A19 19 0 0 1 36 51 H13", "M37 54 L54 87"]),
    'M': (86, ["M13 87 V13 L43 57 L73 13 V87"]),
    'I': (26, ["M13 13 V87"]),
    'N': (64, ["M13 87 V13 L51 87 V13"]),
    'A': (76, ["M11 87 L38 15 L65 87", "M23 62 H53"]),
    'L': (54, ["M13 13 V87 H41"]),
    'G': (70, ["M57 13 H13 V87 H57 V56 H42"]),
    'B': (66, ["M13 13 V87", "M13 13 H35 A18 18 0 0 1 35 49 H13",
               "M13 49 H37 A19 19 0 0 1 37 87 H13"]),
    'O': (64, ["M32 13 A19 37 0 1 0 32 87 A19 37 0 1 0 32 13"]),
    'U': (64, ["M13 13 V68 A19 19 0 0 0 51 68 V13"]),
    'Y': (64, ["M13 13 L32 47 L51 13", "M32 47 V87"]),
    'S': (72, ["M57 13 H36 A23 18.5 0 0 0 36 50 A23 18.5 0 0 1 36 87 H15"]),
    'C': (60, ["M42 18 A19 37 0 1 0 42 82"]),
    'D': (68, ["M13 13 V87", "M13 13 H31 A24 37 0 0 1 31 87 H13"]),
    'P': (62, ["M13 87 V13", "M13 13 H32 A17 18 0 0 1 32 49 H13"]),
    'V': (64, ["M13 13 L32 87 L51 13"]),
    'W': (96, ["M13 13 L33 87 L48 34 L63 87 L83 13"]),
    'H': (66, ["M13 13 V87", "M53 13 V87", "M13 50 H53"]),
    # --- new here, same construction: one upright and two diagonals meeting
    # it on the centre line, both inset 13 like every other terminal.
    'K': (64, ["M13 13 V87", "M53 13 L20 50 L55 87"]),
    ' ': (30, []),
}
TRACK = 8  # tight letter spacing


def word_width(text, track=TRACK, widths=None):
    w = 0
    for i, ch in enumerate(text):
        w += (widths.get(ch) if widths and ch in widths else GLYPHS[ch][0])
        if i < len(text) - 1:
            w += track
    return w


def draw_word(text, x, y, scale, color, track=TRACK):
    """SVG for `text` with the letter grid's top-left at (x, y)."""
    parts = []
    cx = 0.0
    for ch in text:
        w, paths = GLYPHS[ch]
        for d in paths:
            parts.append(
                f'<path transform="translate({x + cx * scale:.1f} {y:.1f}) '
                f'scale({scale:.4f})" d="{d}" fill="none" stroke="{color}" '
                f'stroke-width="26" stroke-linecap="round" '
                f'stroke-linejoin="round"/>')
        cx += w + track
    return "\n".join(parts)


def svg(width, height, body, comment):
    return (f"<!-- {comment}\n"
            "     Hand-authored for AsciiWorldEngine.  No font embedded, "
            "subset or traced;\n     letterforms are original stroked paths. "
            "Regenerate with docs/brand/generate.py -->\n"
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} '
            f'{height}" width="{width}" height="{height}" role="img">\n'
            f"{body}\n</svg>\n")


def panel(w, h, rx=24):
    return (f'<rect x="1" y="1" width="{w - 2}" height="{h - 2}" rx="{rx}" '
            f'fill="{BG}" stroke="{EDGE}" stroke-width="2"/>')


def write(name, content):
    path = os.path.join(OUT, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as handle:
        handle.write(content)
    print(f"wrote {name} ({len(content)} bytes)")


# ---------------------------------------------------------------------------
# THE MARK.
#
# The house composition: one heavy geometric wordmark, tight-tracked, white on
# a near-black panel; the motif REPLACING one letter; an accent-colour full
# stop closing the word; the tagline underneath, lighter, grey, wide-tracked.
#
# The motif is a **setback tower** standing where the second I of ASCII would
# be — the letter is a vertical bar already, and this engine's world generator
# builds towers out of exactly this shape: a wide base, two setback rings, a
# crown, and a mast on top (`world.rs`, the six building profiles).  It is
# drawn with a hard flat top and square shoulders against an alphabet of round
# terminals, so it reads as architecture standing in a line of letters rather
# than as another glyph.  Its storeys are struck out in the panel colour, the
# way a lit facade reads in the engine: rows, not speckle.
#
# Nothing here is borrowed: the whole vocabulary is a tower, a skyline and a
# full stop.
TOWER_W = 66        # the plot the tower stands on, wider than the I it replaces
TOWER_RISE = 20     # the mast carries past cap height; the base sits on it
STOREY = 11         # storey pitch, in glyph units
SETBACKS = ((0, 100, 44), (8, 44, 20), (19, 20, 2))  # (inset, y_from, y_to)


def draw_tower(x, y0, scale, accent=CYAN, ground=BG):
    """The setback tower filling the plot, ink-left-edge at x.

    Glyph space is y=0 at cap top, y=100 at the baseline; the tower is built
    upwards from the baseline and the mast carries past the cap.
    """
    def px(u):
        return x + u * scale

    def py(u):
        return y0 + u * scale

    parts = []
    for inset, y_from, y_to in SETBACKS:
        parts.append(
            f'<rect x="{px(inset):.1f}" y="{py(y_to):.1f}" '
            f'width="{(TOWER_W - 2 * inset) * scale:.1f}" '
            f'height="{(y_from - y_to) * scale:.1f}" fill="{accent}"/>')
    # the mast, on the middle of the crown
    parts.append(
        f'<rect x="{px(TOWER_W / 2 - 4):.1f}" y="{py(-TOWER_RISE):.1f}" '
        f'width="{8 * scale:.1f}" height="{(2 + TOWER_RISE) * scale:.1f}" '
        f'fill="{accent}"/>')
    # storeys: struck out in the panel colour so the tower reads as a facade
    y = 100 - STOREY
    while y > 6:
        inset = next(i for i, a, b in SETBACKS if b <= y < a)
        parts.append(
            f'<rect x="{px(inset):.1f}" y="{py(y):.1f}" '
            f'width="{(TOWER_W - 2 * inset) * scale:.1f}" '
            f'height="{3 * scale:.1f}" fill="{ground}"/>')
        y -= STOREY
    return "\n".join(parts)


# The skyline strip under the wordmark: the same six profiles the generator
# actually draws, in a row, on one baseline.  Heights in glyph units.
SKYLINE = (6, 18, 11, 30, 22, 8, 40, 15, 26, 10, 34, 19, 7, 44, 13, 24,
           9, 32, 17, 5, 28, 38, 12, 21, 8, 30, 16, 46, 10, 25, 14, 6,
           20, 9)


def skyline(x, baseline, width, unit, colour=CYAN, gap_ratio=0.22):
    n = len(SKYLINE)
    pitch = width / n
    w = pitch * (1 - gap_ratio)
    parts = []
    for i, h in enumerate(SKYLINE):
        hh = h * unit
        parts.append(f'<rect x="{x + i * pitch:.1f}" y="{baseline - hh:.1f}" '
                     f'width="{w:.1f}" height="{hh:.1f}" fill="{colour}"/>')
    return "\n".join(parts)


def logo():
    W, H = 1400, 420
    text = "ASCIIWORLDENGINE"
    TOWER_AT = 4           # the second I of ASCII
    DOT_R = 16             # the full stop, bottom-aligned with the letter ink
    widths = {}
    scale = 1.10
    # advance table with the tower's plot substituted for that one letter
    adv = [GLYPHS[c][0] for c in text]
    adv[TOWER_AT] = TOWER_W
    total = (sum(adv) + TRACK * len(text) + 2 * DOT_R) * scale
    x0 = (W - total) / 2
    y0 = 84
    parts = [panel(W, H)]
    cx = 0.0
    for i, ch in enumerate(text):
        if i == TOWER_AT:
            parts.append(draw_tower(x0 + cx * scale, y0, scale))
        else:
            parts.append(draw_word(ch, x0 + cx * scale, y0, scale, FG))
        cx += adv[i] + TRACK
    parts.append(
        f'<circle cx="{x0 + (cx + DOT_R) * scale:.1f}" '
        f'cy="{y0 + (100 - DOT_R) * scale:.1f}" r="{DOT_R * scale:.1f}" '
        f'fill="{CYAN}"/>')
    parts.append(skyline(x0, 284, total, 1.30))
    tw = word_width(TAGLINE, track=14) * 0.30
    parts.append(draw_word(TAGLINE, (W - tw) / 2, 316, 0.30, GREY, track=14))
    write("asciiworldengine-logo.svg",
          svg(W, H, "\n".join(parts),
              "AsciiWorldEngine logo — the wordmark, the tower, the full stop"))


def icon():
    """A city block, square, at avatar and favicon size.

    The wordmark's motif distilled: the setback tower with a neighbour either
    side, standing on a pavement.  Every coordinate is a multiple of 8 inside
    the 128 box, so at a 16 px favicon each tower lands on whole device pixels
    and the block stays three towers instead of blurring into one bar.
    """
    IW = 128
    body = [panel(IW, IW, rx=28)]
    #        x,   w,  top   — left neighbour, the tower, right neighbour
    for x, w, top in ((24, 24, 64), (56, 24, 40), (88, 16, 56)):
        body.append(f'<rect x="{x}" y="{top}" width="{w}" '
                    f'height="{104 - top}" fill="{CYAN}"/>')
    body.append(f'<rect x="64" y="24" width="8" height="24" fill="{CYAN}"/>')
    body.append(f'<rect x="16" y="104" width="96" height="8" fill="{CYAN}"/>')
    write("asciiworldengine-icon.svg",
          svg(IW, IW, "\n".join(body),
              "AsciiWorldEngine icon — the tower and its neighbours"))


if __name__ == "__main__":
    logo()
    icon()
