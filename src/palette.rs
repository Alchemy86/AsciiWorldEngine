//! Colour, texture and the facade tables — the vocabulary the renderer draws
//! from. Every hue, every falloff and every dither threshold this city is
//! painted with is in this one file, so the whole look can be read in one
//! sitting and changed in one place.

use crate::rng::noise;

/// `hsl(h deg, s%, l%)` -> 8-bit RGB.
///
/// Colour is *always* a floor plus a range: `hsl(h, s, base + range*b)`.
/// Brightness never multiplies a colour toward black — that is why distant
/// towers stay vivid instead of fading to grey.
#[inline]
pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = (s.clamp(0.0, 100.0)) / 100.0;
    let l = (l.clamp(0.0, 100.0)) / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        (((r + m) * 255.0) + 0.5) as u8,
        (((g + m) * 255.0) + 0.5) as u8,
        (((b + m) * 255.0) + 0.5) as u8,
    ]
}

/// 8x8 ordered (Bayer) dither, values `(m + 0.5)/64`.
///
/// ORDERED, not random. This is the single most visible way to get the look
/// wrong: an ordered matrix makes the lit panes on one storey line up with the
/// lit panes on the next, so windows read as rows rather than as speckle.
pub static DITHER: [f32; 64] = build_dither();

const fn build_dither() -> [f32; 64] {
    // Recursive Bayer construction, unrolled for const evaluation.
    let mut m = [[0u32; 8]; 8];
    let mut n = 1usize;
    while n < 8 {
        let mut y = 0;
        while y < n {
            let mut x = 0;
            while x < n {
                let v = 4 * m[y][x];
                m[y][x] = v;
                m[y][x + n] = v + 2;
                m[y + n][x] = v + 3;
                m[y + n][x + n] = v + 1;
                x += 1;
            }
            y += 1;
        }
        n *= 2;
    }
    let mut out = [0.0f32; 64];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            out[y * 8 + x] = (m[y][x] as f32 + 0.5) / 64.0;
            x += 1;
        }
        y += 1;
    }
    out
}

/// Snap world-units-per-character to a power of two so a surface's pattern
/// holds still instead of shimmering as you walk toward it.
#[inline]
pub fn quant(scale: f32) -> f32 {
    let q = (2.0 * scale).max(1.0);
    if q < 2.0 { 1.0 } else if q < 4.0 { 2.0 } else if q < 8.0 { 4.0 }
    else if q < 16.0 { 8.0 } else if q < 32.0 { 16.0 } else { 32.0 }
}

/// The same power-of-two world grid, continued BELOW one unit.
///
/// `quant` floors at one world unit, and outdoors that is exactly right:
/// nothing is ever nearer than the far side of a forecourt, so a character
/// always covers a unit or more and a finer grid would only shimmer. Indoors a
/// wall is an arm's length away and one world unit is sixty screen columns —
/// at that floor a room is textured in sixty-column blocks. Continuing the
/// ladder down keeps the "snapped to a power of two, so it holds still"
/// property and gives a surface you can stand next to something to look at.
pub fn quant_fine(scale: f32) -> f32 {
    let q = 2.0 * scale;
    if q >= 1.0 {
        quant(scale)
    } else if q >= 0.5 {
        0.5
    } else if q >= 0.25 {
        0.25
    } else if q >= 0.125 {
        0.125
    } else {
        0.0625
    }
}

/// Ordered surface texture at world coordinate `(u, v)`.
#[inline]
pub fn surf_tex(u: f32, v: f32, scale: f32, ou: i32, ov: i32) -> f32 {
    let q = quant(scale);
    let iu = ((2.0 * u / q).floor() as i32).wrapping_add(ou) & 7;
    let iv = ((2.0 * v / q).floor() as i32).wrapping_add(ov) & 7;
    DITHER[((iv << 3) | iu) as usize]
}

/// Vertical bulge down a wall span: brightest around mid-height.
#[inline]
pub fn v_profile(i: i32, span: i32) -> f32 {
    let t = if span > 0 { i as f32 / span as f32 } else { 0.0 };
    0.7 + 0.3 * (core::f32::consts::PI * t).sin()
}

/// brightness -> glyph. A fully lit near surface is `@`, a dark one is blank.
pub const WALL_RAMP: &[u8; 12] = b"@%#&8ZX*+:. ";

// --- facade styles -------------------------------------------------------
/// Six facade styles, one per procedurally laid-out building: a hue quartet
/// plus a pattern index. The pattern picks the storey pitch, the window glyph
/// run, the mullion glyph and the frame character.
pub struct Facade {
    pub frame_hue: f32,
    pub accent_hue: f32,
    pub glass_hue: f32,
    pub light_hue: f32,
    pub pattern: usize,
}

pub static FACADES: [Facade; 6] = [
    Facade { frame_hue: 178.0, accent_hue: 292.0, glass_hue: 205.0, light_hue: 48.0,  pattern: 0 }, // NEON_GRID
    Facade { frame_hue: 38.0,  accent_hue: 12.0,  glass_hue: 222.0, light_hue: 54.0,  pattern: 1 }, // AMBER_BANDS
    Facade { frame_hue: 276.0, accent_hue: 188.0, glass_hue: 238.0, light_hue: 325.0, pattern: 2 }, // VIOLET_PANES
    Facade { frame_hue: 148.0, accent_hue: 48.0,  glass_hue: 192.0, light_hue: 164.0, pattern: 3 }, // MINT_ARCADE
    Facade { frame_hue: 215.0, accent_hue: 28.0,  glass_hue: 232.0, light_hue: 190.0, pattern: 4 }, // COBALT_STRIP
    Facade { frame_hue: 4.0,   accent_hue: 45.0,  glass_hue: 218.0, light_hue: 18.0,  pattern: 5 }, // RED_TERMINAL
];

pub const FLOOR_PITCH: [i32; 6] = [5, 3, 6, 4, 2, 5];
pub const PANE_LIT: [f32; 6] = [0.38, 0.28, 0.46, 0.34, 0.25, 0.42];
pub const PANE_ON: [&[u8]; 6] = [b"0", b"@", b"[]", b"o", b"-", b"8"];
pub const PANE_OFF: [&[u8]; 6] = [b":", b":", b".", b":", b"_", b"#"];
pub const EDGE_CH: [u8; 6] = [b'|', b'|', b'[', b'{', b'|', b'|'];
pub const LEDGE_CH: [u8; 6] = [b'=', b'=', b'=', b'=', b'=', b'#'];

/// Building names stamped down the storefront signage row, one character per
/// half world unit — that is what makes the street band read as shopfront text.
pub const SIGN_NAMES: [&str; 8] =
    ["NOVA", "ORBIT", "CINDER", "STATIC", "LUMEN", "VECTOR", "EMBER", "SIGNAL"];
pub const SIGN_TYPES: [&str; 8] =
    ["SUPPLY", "CAFE", "OFFICES", "CLINIC", "WORKS", "HOUSE", "LAUNDRY", "ARCADE"];

/// Stable per-building identity: same block + same plan id -> same facade.
pub struct Building {
    pub style: &'static Facade,
    pub label: [u8; 16],
    pub label_len: usize,
    /// How much of `label` is the building's NAME, before the shop type runs
    /// straight into it. The storefront wants the whole thing; anything that
    /// wants to say what the building is CALLED — the room you walk into, for
    /// one — wants only this much.
    pub name_len: usize,
}

/// `grain` is the world's district grain (`world::grain_for`): zero means every
/// building picks its own facade, as it always has; above zero a district
/// shares one. The building's NAME stays keyed on the building itself either
/// way — a uniform district is one that repeats a pattern, not one where every
/// shop has the same name.
pub fn building_of(gx: i32, gz: i32, plan_id: u16, block: i32, grain: i32) -> Building {
    let bx = gx.div_euclid(block);
    let bz = gz.div_euclid(block);
    let (kx, kz) = if grain <= 0 {
        (bx, bz)
    } else {
        (bx.div_euclid(grain), bz.div_euclid(grain))
    };
    let p = plan_id as i32;
    let ps = if grain <= 0 { p } else { 0 };
    let a = noise(kx * 131 + ps * 17, kz * 197 + ps * 41);
    let b = noise(bz * 313 + p * 7, bx * 89 + p * 53);
    let style = &FACADES[((a * 6.0) as usize) % 6];
    let name = SIGN_NAMES[((b * 8.0) as usize) % 8];
    let kind = SIGN_TYPES[((b * 64.0) as usize) % 8];
    let mut label = [0u8; 16];
    let mut n = 0;
    for &c in name.as_bytes().iter().chain(kind.as_bytes()) {
        if n < 16 { label[n] = c; n += 1; }
    }
    Building { style, label, label_len: n, name_len: name.len().min(n) }
}

// --- billboards ----------------------------------------------------------
/// A 3x5 stroke font. Tall buildings carry one lit word (or one big letter) on
/// a framed panel partway up the facade.
fn glyph3x5(c: u8) -> [&'static str; 5] {
    match c {
        b'A' => [".#.", "#.#", "###", "#.#", "#.#"],
        b'B' => ["##.", "#.#", "##.", "#.#", "##."],
        b'C' => [".##", "#..", "#..", "#..", ".##"],
        b'D' => ["##.", "#.#", "#.#", "#.#", "##."],
        b'E' => ["###", "#..", "##.", "#..", "###"],
        b'I' => ["###", ".#.", ".#.", ".#.", "###"],
        b'L' => ["#..", "#..", "#..", "#..", "###"],
        b'M' => ["#.#", "###", "###", "#.#", "#.#"],
        b'N' => ["#.#", "###", "###", "###", "#.#"],
        b'O' => [".#.", "#.#", "#.#", "#.#", ".#."],
        b'T' => ["###", ".#.", ".#.", ".#.", ".#."],
        b'V' => ["#.#", "#.#", "#.#", "#.#", ".#."],
        b'Y' => ["#.#", "#.#", ".#.", ".#.", ".#."],
        _ => ["...", "...", "...", "...", "..."],
    }
}

pub const SIGN_WORDS: [&str; 8] = ["NOVA", "DATA", "BYTE", "OMNI", "CITY", "LIVE", "VOID", "NEON"];

/// Rows of the rendered sign grid: `k` selects the word, `narrow` picks the
/// one-big-initial variant.
///
/// Built once and shared. Rendering the stroke font per wall hit allocated ten
/// strings for every billboard on screen, which is invisible at street level
/// and the single biggest cost from an elevated vista where hundreds of
/// facades are in frame at once.
pub fn sign_rows(k: usize, narrow: bool) -> &'static [String; 5] {
    static TABLE: std::sync::OnceLock<Vec<[String; 5]>> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut v = Vec::with_capacity(16);
        for narrow in [false, true] {
            for word in SIGN_WORDS {
                let w = word.as_bytes();
                let mut rows: [String; 5] = Default::default();
                if narrow {
                    for (r, row) in rows.iter_mut().enumerate() {
                        *row = format!("......{}......", glyph3x5(w[0])[r]);
                    }
                } else {
                    for (i, &c) in w.iter().enumerate() {
                        let g = glyph3x5(c);
                        for (r, row) in rows.iter_mut().enumerate() {
                            if i > 0 { row.push('.'); }
                            row.push_str(g[r]);
                        }
                    }
                }
                v.push(rows);
            }
        }
        v
    });
    &t[if narrow { 8 } else { 0 } + k % 8]
}

// --- registration plates -------------------------------------------------
/// The most a plate can carry. A current UK registration is 8 characters with
/// its space ("AB12 CDE"); a private one is usually shorter and occasionally
/// longer, so there is headroom.
pub const PLATE_MAX: usize = 10;

/// How many even pitches a registration can be set at — see `Plate::settings`.
pub const PLATE_SETTINGS: usize = 3;

/// The most cells a set registration can ever cover: every character one apart
/// and the group gap opened. Nothing wider is a single registration.
pub const PLATE_SET_MAX: usize = 2 * PLATE_MAX;

/// One registration, held as fixed bytes rather than a `String` so the
/// renderer's inner loop touches no heap.
#[derive(Clone, Copy)]
pub struct Plate {
    text: [u8; PLATE_MAX],
    len: u8,
}

impl Plate {
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.text[..self.len as usize]
    }

    pub fn as_str(&self) -> &str {
        // Normalisation keeps only A-Z, 0-9 and space, so this is always ASCII.
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    /// The widths this registration can be SET at, tight to wide.
    ///
    /// At one glyph per cell a character cannot be drawn any larger, so the
    /// only way for a registration to fill more of its plate is to SPAN it.
    /// These are the three even pitches it can be set at — even, because a
    /// registration with one pair of characters touching and the rest apart
    /// reads as a typo:
    ///
    /// * `0` — every character touching its neighbour. `RT08 AAR`.
    /// * `1` — the registration's own space opened out on both sides, so the
    ///   group gap is wider than the character gap, the way a real plate's is.
    ///   `RT08   AAR`.
    /// * `2` — every character set one cell apart, the group gap wider still.
    ///   `R T 0 8   A A R`.
    ///
    /// One cell between characters is the most on offer. Two is two words, and
    /// a reader stops seeing one registration.
    ///
    /// The caller picks whichever of the three lands closest to the width its
    /// panel wants to be, which is what closes the gap between the characters
    /// and the yellow.
    pub fn settings(&self) -> [usize; PLATE_SETTINGS] {
        let n = self.len as usize;
        let (word, plain) = self.slots();
        [n, n + word, n + word + plain]
    }

    /// How many of the gaps between adjacent characters touch the
    /// registration's own space, and how many do not.
    fn slots(&self) -> (usize, usize) {
        let t = self.as_bytes();
        let mut word = 0;
        for i in 1..t.len() {
            if t[i - 1] == b' ' || t[i] == b' ' {
                word += 1;
            }
        }
        (word, t.len().saturating_sub(1) - word)
    }

    /// Write this registration into `out`, set at `s`. Returns how many cells
    /// it covers, which is always `self.settings()[s]`.
    ///
    /// The caller has already sized the panel to this, so nothing is centred
    /// and nothing is padded: what comes back is the field of the plate.
    pub fn set_into(&self, s: usize, out: &mut [u8]) -> usize {
        let t = self.as_bytes();
        debug_assert!(out.len() >= PLATE_SET_MAX, "a buffer set_into can always fill");
        let mut w = 0;
        for i in 0..t.len() {
            if i > 0 {
                let touches_space = t[i - 1] == b' ' || t[i] == b' ';
                if s == 2 || (s == 1 && touches_space) {
                    out[w] = b' ';
                    w += 1;
                }
            }
            out[w] = t[i];
            w += 1;
        }
        w
    }

    /// Uppercase, keep only characters a plate can actually carry, collapse
    /// runs of spaces, trim, and cut to `PLATE_MAX`. Anything left empty is
    /// not a plate and the caller drops it.
    pub fn parse(raw: &str) -> Option<Plate> {
        let mut text = [b' '; PLATE_MAX];
        let mut len = 0usize;
        let mut pending_space = false;
        for ch in raw.chars() {
            let c = ch.to_ascii_uppercase() as u32;
            let c = match c {
                0x41..=0x5A | 0x30..=0x39 => c as u8,
                // A hyphen or a dot in a written registration is a space.
                0x20 | 0x2D | 0x2E | 0x5F => b' ',
                _ => continue,
            };
            if c == b' ' {
                pending_space = len > 0;
                continue;
            }
            if pending_space && len < PLATE_MAX {
                text[len] = b' ';
                len += 1;
                pending_space = false;
            }
            if len >= PLATE_MAX {
                break;
            }
            text[len] = c;
            len += 1;
        }
        if len == 0 {
            None
        } else {
            Some(Plate { text, len: len as u8 })
        }
    }
}

/// Letters a DVLA registration is built from: no `I` and no `Q`, which is why
/// a generated plate reads as a real one rather than as random noise.
const PLATE_LETTERS: &[u8; 24] = b"ABCDEFGHJKLMNOPRSTUVWXYZ";
/// Age identifiers in the current-style format: March plates run 02..25,
/// September plates 51..75.
const PLATE_AGES: [u8; 2] = [0, 50];

/// The committed default registrations: real plates the operator has for
/// sale, shipped as data rather than as a string literal in this file, so
/// the stock can change without anybody touching code. One registration per
/// line, same format `--plates-file` reads — see `registrations.txt` itself.
const DEFAULT_REGISTRATIONS: &str = include_str!("registrations.txt");

/// Where a `Plates` set came from. Only used to phrase the note a run prints
/// about its own plates, so a generated placeholder is never mistaken for a
/// real registration and a real one is never reported as if it were typed in
/// on the command line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlateSource {
    /// Plausible-looking patterns derived from `--seed`. Not real.
    Generated,
    /// `registrations.txt`, committed to the repo: real registrations, until
    /// the operator overrides them.
    Default,
    /// `--plates` / `--plates-file`: the operator's own list.
    Supplied,
}

/// The registrations the traffic carries.
///
/// Either the operator's own list — `--plates` or `--plates-file` — or, with
/// neither given, the committed default in `registrations.txt`. `source`
/// says which, and the binary says so on screen and in `--help`.
pub struct Plates {
    list: Vec<Plate>,
    pub source: PlateSource,
}

impl Plates {
    /// A supplied list. Returns how many entries were unusable alongside the
    /// set, so the caller can say so rather than silently swallowing them.
    pub fn from_list<S: AsRef<str>>(raw: &[S]) -> (Plates, usize) {
        let mut list = Vec::with_capacity(raw.len());
        let mut dropped = 0;
        for r in raw {
            match Plate::parse(r.as_ref()) {
                Some(p) => list.push(p),
                None => dropped += 1,
            }
        }
        if list.is_empty() {
            return (Plates::from_seed(0xACC17, 128), dropped);
        }
        (Plates { list, source: PlateSource::Supplied }, dropped)
    }

    /// The list every run carries with no `--plates`, `--plates-file` or
    /// `--no-plates` given: the committed registrations in
    /// `registrations.txt`. Falls back to a generated set only if that file
    /// were ever left with nothing usable in it, so the traffic is never
    /// blank.
    pub fn default_set() -> Plates {
        let lines: Vec<&str> = DEFAULT_REGISTRATIONS
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .collect();
        let (mut plates, _dropped) = Plates::from_list(&lines);
        if plates.source == PlateSource::Supplied {
            plates.source = PlateSource::Default;
        }
        plates
    }

    /// `n` plausible current-style registrations — `LLNN LLL` — derived from
    /// the seed, so the same seed always produces the same set. These are
    /// PATTERNS, not real registrations.
    pub fn from_seed(seed: u64, n: usize) -> Plates {
        let mut rng = crate::rng::Rng::new(seed ^ 0x_50_1A_7E_5);
        let mut list = Vec::with_capacity(n);
        for _ in 0..n {
            let mut text = [b' '; PLATE_MAX];
            text[0] = PLATE_LETTERS[rng.below(24) as usize];
            text[1] = PLATE_LETTERS[rng.below(24) as usize];
            let age = PLATE_AGES[(rng.below(2)) as usize] as u32 + 2 + rng.below(24);
            text[2] = b'0' + (age / 10) as u8;
            text[3] = b'0' + (age % 10) as u8;
            text[4] = b' ';
            for slot in text.iter_mut().skip(5).take(3) {
                *slot = PLATE_LETTERS[rng.below(24) as usize];
            }
            list.push(Plate { text, len: 8 });
        }
        Plates { list, source: PlateSource::Generated }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Every registration actually on the road, in list order. Nothing on
    /// the draw path touches this — `get` is what a frame draws from — it is
    /// for tooling that wants to check what a set really carries, whichever
    /// of `default_set` / `from_list` / `from_seed` built it.
    pub fn all(&self) -> &[Plate] {
        &self.list
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The shortest registration in the set. Below that many characters plus
    /// `PLATE_PAD`, a panel cannot be carrying one.
    pub fn shortest(&self) -> u8 {
        self.list.iter().map(|p| p.len).min().unwrap_or(0)
    }

    /// The narrowest panel on a frame that could be carrying a registration at
    /// all: the shortest one in the set plus the edge and margin cells every
    /// drawn plate spends. Anything narrower is the honest middle-distance
    /// smudge, and `--plate-shot` splits the near band from the middle one on
    /// exactly this line.
    pub fn readable_width(&self) -> usize {
        self.shortest().max(2) as usize + PLATE_PAD
    }

    /// The plate a vehicle's own key resolves to. The key is drawn once, at
    /// spawn, from the seeded population generator, so a vehicle keeps the same
    /// registration for as long as it is on screen and the same seed always
    /// hands the same cars the same plates.
    #[inline]
    pub fn get(&self, key: u16) -> &Plate {
        &self.list[key as usize % self.list.len()]
    }
}

impl Default for Plates {
    fn default() -> Self {
        Plates::default_set()
    }
}

/// **A plate is drawn out of characters, like everything else on this screen.**
///
/// It used to be a filled rectangle: a background colour painted behind black
/// ink. It read correctly and it was the wrong medium — every other thing in
/// this city is a coloured glyph on black, and one painted block in the middle
/// of it looks pasted on rather than drawn. So these are now the colour of the
/// GLYPHS the plate's body and edge are made of, not of a panel behind them,
/// and nothing about a plate paints a background any more.
///
/// **One colour, front and rear**, and that is a change from the painted panel.
/// A rear plate in this country is yellow and a front plate white, which is
/// what a panel could say — black ink on a white field is still a plate. Drawn
/// out of characters on a black screen it is not: a white frame reads as
/// generic interface furniture, and it collides with the near-white the
/// registration is set in, so the body and the characters stop separating. The
/// yellow is what makes a plate recognisable as an object before anybody reads
/// a character off it, so the yellow is what both ends get.
///
/// The yellow is the real one. BS AU 145d — the standard a road-legal plate is
/// made to — is a fully saturated amber-yellow, and it is the SATURATION that
/// makes a plate recognisable as an object before anybody reads a character
/// off it. `[247, 214, 24]` was a shade off it, greener and impure, and read
/// as a highlighter pen; `[255, 204, 0]` is the plate.
pub const PLATE_BODY: [u8; 3] = [255, 204, 0];
/// The registration itself: **ordinary characters**, near-white, no colour of
/// its own. On a painted panel the ink was pure black because the panel was
/// there to give it contrast; on black it has to be the bright thing, and the
/// plate's own colour is spent on the body around it so the two do not compete
/// for the same job.
pub const PLATE_INK: [u8; 3] = [238, 240, 246];

/// The characters a plate's body and edge are drawn with.
///
/// `#` for the rules that run along a plate's top and bottom, `+` at their
/// corners, `|` for the uprights at each end of the row the registration sits
/// on, and `[` `]` when the plate is only one row tall and a bare upright would
/// not read as an end.
///
/// **None of them is a character the bodywork is drawn with, and that is the
/// whole of why they are these characters.** A vehicle is `-`, `=`, `:` and
/// `o`; the first try used `=` for the rules and the plate's top and bottom
/// dissolved straight into the back of the car — the same failure the old
/// painted panel had before it was given a dark edge, arriving again by a
/// different route. `#` is also the densest glyph in common use, so a run of
/// them is the nearest thing to a bar of solid yellow that can be drawn out of
/// characters, which is what a plate's body should read as.
pub const PLATE_RULE: u8 = b'#';
pub const PLATE_CORNER: u8 = b'+';
pub const PLATE_UPRIGHT: u8 = b'|';
pub const PLATE_CAP_L: u8 = b'[';
pub const PLATE_CAP_R: u8 = b']';

/// Cells a drawn plate spends on something other than the registration: one
/// character of its own body at each end of the row the registration is on.
///
/// It was four while a plate was a painted panel — a dark edge cell and a clear
/// margin cell at each end, because the yellow ran straight into the bodywork
/// otherwise and the characters needed air. A drawn upright does both of those
/// jobs itself, so the margin is gone and the plate is two cells tighter, which
/// is two more cells of registration at the distance where that decides whether
/// it can be read at all.
pub const PLATE_PAD: usize = 2;
