//! Ways of getting a grid out: 24-bit ANSI for a terminal, plain text and SVG
//! for committed evidence frames. No dependencies — these are just string
//! builders over the same grid the frontends paint.

use crate::grid::Grid;
use std::fmt::Write as _;

/// 24-bit ANSI, one escape sequence only where the colour actually changes.
/// A 180x80 frame is ~14k cells; emitting a colour per cell would be ~280 KB
/// of escape codes a frame and no terminal keeps up with that.
///
/// The background plane is written the same way, run by run: `SGR 48;2` only
/// where the background colour actually changes, and `SGR 49` back to the
/// terminal's own black wherever it goes black. A frame drawn with shading off
/// never writes a background at all, so `has_bg` is false and not one extra
/// byte is emitted — which is why the plain look costs what it always did.
///
/// With shading on the background IS most of the frame, and the cost is the
/// escape sequences, not the compare. That is why `color::shade` quantises:
/// backgrounds that agree to within four counts of 255 come out byte-identical
/// and collapse into one run.
pub fn grid_to_ansi(g: &Grid, out: &mut String) {
    out.clear();
    // Two bodies rather than a test per cell: this is the paint loop, and a
    // frame with no background on it must cost exactly what it cost before
    // there was a background plane.
    if g.has_bg {
        ansi_rows::<true>(g, out);
    } else {
        ansi_rows::<false>(g, out);
    }
    out.push_str("\x1b[0m");
}

fn ansi_rows<const BG: bool>(g: &Grid, out: &mut String) {
    for y in 0..g.rows {
        let _ = write!(out, "\x1b[{};1H", y + 1);
        let mut last: i32 = -1;
        let mut last_bg: i32 = 0;
        for x in 0..g.cols {
            let i = y * g.cols + x;
            let (r, gg, b) = (g.rgb[i * 3], g.rgb[i * 3 + 1], g.rgb[i * 3 + 2]);
            let key = ((r as i32) << 16) | ((gg as i32) << 8) | b as i32;
            if key != last {
                let _ = write!(out, "\x1b[38;2;{r};{gg};{b}m");
                last = key;
            }
            if BG {
                let (br, bgg, bb) = (g.bg[i * 3], g.bg[i * 3 + 1], g.bg[i * 3 + 2]);
                let bkey = ((br as i32) << 16) | ((bgg as i32) << 8) | bb as i32;
                if bkey != last_bg {
                    if bkey == 0 {
                        out.push_str("\x1b[49m");
                    } else {
                        let _ = write!(out, "\x1b[48;2;{br};{bgg};{bb}m");
                    }
                    last_bg = bkey;
                }
            }
            out.push(if g.ch[i] == 0 { ' ' } else { g.ch[i] as char });
        }
        // `\x1b[K` clears with the CURRENT background, so a row that ended on
        // a shaded cell would paint the rest of the line that colour.
        if BG && last_bg != 0 {
            out.push_str("\x1b[49m");
        }
        let _ = write!(out, "\x1b[K");
    }
}

/// Plain characters, no colour — for committed evidence frames and for diffing
/// two runs against each other.
pub fn grid_to_text(g: &Grid) -> String {
    let mut s = String::with_capacity(g.rows * (g.cols + 1));
    for y in 0..g.rows {
        let start = s.len();
        for x in 0..g.cols {
            let c = g.ch[y * g.cols + x];
            s.push(if c == 0 { ' ' } else { c as char });
        }
        while s.len() > start && s.ends_with(' ') {
            s.pop();
        }
        s.push('\n');
    }
    s
}

/// Self-contained SVG — colour evidence that renders anywhere.
///
/// The cell is 11x18 — a 5.5:9 aspect, so the picture is not stretched
/// relative to the projection it was built for — and the type is set at 1.11x
/// the row height (10px glyphs on a 9px pitch). Glyphs that under-fill their
/// cell are what make an ASCII picture look washed out; `textLength` keeps a run
/// locked to the grid. The scanline and vignette are drawn over the top.
pub fn grid_to_svg(g: &Grid, title: &str) -> String {
    const CW: usize = 11;
    const CH: usize = 18;
    let w = g.cols * CW;
    let h = g.rows * CH;
    let pitch = CH as f32 * 4.0 / 9.0;
    let mut s = String::with_capacity(g.cols * g.rows * 8);
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
viewBox=\"0 0 {w} {h}\" font-family=\"ui-monospace,Consolas,'Courier New',monospace\" \
font-size=\"{:.1}\" font-weight=\"700\">\n",
        CH as f32 * 1.11
    );
    if !title.is_empty() {
        let _ = write!(s, "<title>{}</title>\n", esc(title));
    }
    let _ = write!(
        s,
        "<defs><pattern id=\"sl\" width=\"{w}\" height=\"{pitch:.2}\" patternUnits=\"userSpaceOnUse\">\
<rect width=\"{w}\" height=\"{:.2}\" fill=\"#fff\" opacity=\"0.012\"/></pattern>\
<radialGradient id=\"vg\" cx=\"50%\" cy=\"50%\" r=\"72%\">\
<stop offset=\"58%\" stop-color=\"#000\" stop-opacity=\"0\"/>\
<stop offset=\"82%\" stop-color=\"#000\" stop-opacity=\"0.08\"/>\
<stop offset=\"100%\" stop-color=\"#000\" stop-opacity=\"0.28\"/></radialGradient></defs>\n\
<rect width=\"{w}\" height=\"{h}\" fill=\"#000\"/>\n",
        pitch / 4.0
    );

    // The background plane first, under everything, so a cell's own glyph is
    // never sat on by its own background. Runs of one colour, and skipped
    // entirely on a frame drawn with shading off — which has no background.
    for y in 0..g.rows {
        if !g.has_bg {
            break;
        }
        let mut x = 0usize;
        while x < g.cols {
            let i = y * g.cols + x;
            let key = (g.bg[i * 3], g.bg[i * 3 + 1], g.bg[i * 3 + 2]);
            if key == (0, 0, 0) {
                x += 1;
                continue;
            }
            let start = x;
            while x < g.cols {
                let j = y * g.cols + x;
                if (g.bg[j * 3], g.bg[j * 3 + 1], g.bg[j * 3 + 2]) != key {
                    break;
                }
                x += 1;
            }
            let _ = write!(
                s,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#{:02x}{:02x}{:02x}\"/>\n",
                start * CW,
                y * CH,
                (x - start) * CW,
                CH,
                key.0,
                key.1,
                key.2
            );
        }
    }

    for y in 0..g.rows {
        let mut run = String::new();
        let mut run_color = String::new();
        let mut run_start = 0usize;
        for x in 0..g.cols {
            let i = y * g.cols + x;
            let c = format!("#{:02x}{:02x}{:02x}", g.rgb[i * 3], g.rgb[i * 3 + 1], g.rgb[i * 3 + 2]);
            if c != run_color || run.is_empty() {
                flush_run(&mut s, &run, &run_color, run_start, y, CW, CH);
                run.clear();
                run_color = c;
                run_start = x;
            }
            run.push(if g.ch[i] == 0 { ' ' } else { g.ch[i] as char });
        }
        flush_run(&mut s, &run, &run_color, run_start, y, CW, CH);
    }
    let _ = write!(s, "<rect width=\"{w}\" height=\"{h}\" fill=\"url(#sl)\"/>\n");
    let _ = write!(s, "<rect width=\"{w}\" height=\"{h}\" fill=\"url(#vg)\"/>\n");
    s.push_str("</svg>\n");
    s
}

/// One run of same-coloured characters.
///
/// There is no weight axis: every glyph is set at one weight. The background
/// plane is a surface colour, not a panel, so a cell having one says nothing
/// about what is written on it.
fn flush_run(
    s: &mut String,
    run: &str,
    colour: &str,
    start: usize,
    y: usize,
    cw: usize,
    ch: usize,
) {
    if run.trim().is_empty() {
        return;
    }
    let _ = write!(
        s,
        "<text x=\"{}\" y=\"{}\" textLength=\"{}\" lengthAdjust=\"spacing\" fill=\"{}\" \
xml:space=\"preserve\">{}</text>\n",
        start * cw,
        y * ch + ch - 3,
        run.chars().count() * cw,
        colour,
        esc(run)
    );
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
