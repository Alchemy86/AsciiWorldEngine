//! Brightness as characters.
//!
//! A terminal cell is not a pixel: a `@` fills most of its box and a `.` fills
//! almost none of it, so two cells painted the same colour read as two
//! different brightnesses. A ramp is a run of glyphs ordered by how much of
//! their cell they ink, densest first, and picking a glyph off it by
//! brightness is how an ASCII picture carries shading at all.

/// The classic ramp, densest first. A fully lit surface is `@`, a dark one
/// is blank.
pub const CLASSIC: &[u8; 12] = b"@%#&8ZX*+:. ";

/// **How much of its cell a glyph inks**, as a percentage.
///
/// The numbers are the percentage of the box a glyph covers in an ordinary
/// monospace face, eyeballed to the nearest percent or two. Nothing downstream
/// needs better than that: they order a ramp and scale a shaded background,
/// and both are quantised before they reach a screen.
pub(crate) static INK: [u8; 96] = {
    let mut t = [34u8; 96]; // letters and digits, the common case
    t[(b' ' - 32) as usize] = 0;
    t[(b'.' - 32) as usize] = 8;
    t[(b',' - 32) as usize] = 9;
    t[(b'\'' - 32) as usize] = 6;
    t[(b'`' - 32) as usize] = 6;
    t[(b'"' - 32) as usize] = 10;
    t[(b':' - 32) as usize] = 15;
    t[(b';' - 32) as usize] = 17;
    t[(b'_' - 32) as usize] = 11;
    t[(b'-' - 32) as usize] = 11;
    t[(b'~' - 32) as usize] = 14;
    t[(b'^' - 32) as usize] = 13;
    t[(b'=' - 32) as usize] = 21;
    t[(b'+' - 32) as usize] = 23;
    t[(b'*' - 32) as usize] = 27;
    t[(b'|' - 32) as usize] = 18;
    t[(b'/' - 32) as usize] = 19;
    t[(b'\\' - 32) as usize] = 19;
    t[(b'(' - 32) as usize] = 18;
    t[(b')' - 32) as usize] = 18;
    t[(b'[' - 32) as usize] = 22;
    t[(b']' - 32) as usize] = 22;
    t[(b'{' - 32) as usize] = 23;
    t[(b'}' - 32) as usize] = 23;
    t[(b'<' - 32) as usize] = 20;
    t[(b'>' - 32) as usize] = 20;
    t[(b'o' - 32) as usize] = 30;
    t[(b'0' - 32) as usize] = 42;
    t[(b'8' - 32) as usize] = 46;
    t[(b'$' - 32) as usize] = 48;
    t[(b'%' - 32) as usize] = 44;
    t[(b'&' - 32) as usize] = 47;
    t[(b'#' - 32) as usize] = 54;
    t[(b'@' - 32) as usize] = 62;
    t[(b'X' - 32) as usize] = 40;
    t[(b'Z' - 32) as usize] = 38;
    t[(b'H' - 32) as usize] = 40;
    t
};

/// How much of its cell `ch` inks, as a percentage. Anything outside
/// printable ASCII counts as blank.
#[inline]
pub fn coverage(ch: u8) -> u8 {
    if (32..128).contains(&ch) { INK[(ch - 32) as usize] } else { 0 }
}

/// The index into a densest-first ramp of `len` glyphs for brightness `b`
/// (0 dark, 1 fully lit), nudged by `jitter` steps before rounding. A jitter
/// is how a surface breaks up the bands a smooth gradient would otherwise
/// draw; pass 0 for none.
#[inline]
pub fn index(len: usize, b: f32, jitter: f32) -> usize {
    ((len - 1) as f32 * (1.0 - b) + jitter)
        .round()
        .clamp(0.0, (len - 1) as f32) as usize
}

/// The glyph of a densest-first `ramp` for brightness `b` in 0..1.
#[inline]
pub fn glyph(ramp: &[u8], b: f32) -> u8 {
    ramp[index(ramp.len(), b, 0.0)]
}

/// Any set of characters made into a ramp: ordered densest first by
/// `coverage`, ties kept in the order given. So a program can choose the
/// characters its picture is made of and still have them shade correctly.
pub fn by_coverage(glyphs: &[u8]) -> Vec<u8> {
    let mut v = glyphs.to_vec();
    v.sort_by(|a, b| coverage(*b).cmp(&coverage(*a)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classic_ramp_runs_from_full_to_blank() {
        assert_eq!(coverage(CLASSIC[0]), CLASSIC.iter().map(|&g| coverage(g)).max().unwrap());
        assert_eq!(glyph(CLASSIC, 1.0), b'@');
        assert_eq!(glyph(CLASSIC, 0.0), b' ');
    }

    #[test]
    fn any_character_set_becomes_a_ramp() {
        assert_eq!(by_coverage(b".@:"), b"@:.".to_vec());
    }
}
