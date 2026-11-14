//! **Living Starry Night** — the second program on the engine.
//!
//! Van Gogh's The Starry Night as coloured ASCII in the terminal: it pixels
//! in a scatter at a time, then flows like wet paint, the characters
//! shimmering as the brightness under them moves. Built on `ascii-engine`
//! alone — grid, glyph ramp, colour, ANSI output, raw terminal, frame clock
//! and frame capture are all the engine's; the painting, the motion and a
//! small JPEG decoder are this program's.
//!
//! ```text
//! cargo run --release -p starry-night
//! cargo run --release -p starry-night -- --capture out/starry --frames 360
//! ```

mod art;
mod fetch;
mod image;
mod jpeg;

use std::io::Write;
use std::time::{Duration, Instant};

use ascii_engine::frame::Pacer;
use ascii_engine::output::grid_to_ansi;
use ascii_engine::term::{terminal_size, KeyMap, Keyboard, RawTerm};
use ascii_engine::{capture, Grid};

use art::Scene;
use image::Image;

const KEYS: KeyMap = KeyMap {
    one_shot: &[3, 27, b'q' as u32, b'b' as u32, b'r' as u32, b'p' as u32],
    opposed: &[],
};

struct Args {
    image: Option<String>,
    cols: Option<usize>,
    rows: Option<usize>,
    capture: Option<String>,
    frames: usize,
    fps: f32,
    shade: bool,
    seed: i32,
    cell_aspect: Option<f32>,
}

/// A captured frame's cell is 11 x 18 px (`output::grid_to_svg`), not the
/// terminal's usual 1:2, and the picture is fitted to whichever it is shown on.
const SVG_CELL_ASPECT: f32 = 18.0 / 11.0;

fn help() {
    println!(
        "Living Starry Night — Van Gogh's painting as flowing coloured ASCII.\n\n\
         USAGE\n  \
         starry-night                 watch it in this terminal\n  \
         starry-night --capture DIR   write every frame to DIR as .svg + .txt\n\n\
         FLAGS\n  \
         --image PATH    the painting, as a baseline JPEG or binary PPM. Without\n                  \
         it the program fetches Wikimedia Commons' public-domain scan once\n                  \
         into ~/.cache/ascii-engine and reads it from there after.\n  \
         --shade         paint each cell's background a dimmed copy of its ink\n  \
         --fps N         frames a second (default 30)\n  \
         --cols N --rows N   the grid size (default: the terminal; 160x60 for\n                  \
         --capture)\n  \
         --frames N      how many frames --capture writes (default 360)\n  \
         --seed N        which scatter the painting pixels in with\n  \
         --cell-aspect R a cell's height over its width (default 2, or 18/11\n                  \
         for --capture, the shape of a captured frame's cell)\n\n\
         KEYS\n  \
         B  shaded cells on / off      R  pixel it in again\n  \
         P  write this frame to disk   Q or Esc  quit"
    );
}

fn parse() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        image: None,
        cols: None,
        rows: None,
        capture: None,
        frames: 360,
        fps: 30.0,
        shade: false,
        seed: 1889,
        cell_aspect: None,
    };
    let mut i = 0;
    let next = |i: &mut usize| -> String {
        *i += 1;
        argv.get(*i).cloned().unwrap_or_default()
    };
    while i < argv.len() {
        match argv[i].as_str() {
            "--help" | "-h" => {
                help();
                std::process::exit(0);
            }
            "--image" => a.image = Some(next(&mut i)),
            "--cols" => a.cols = next(&mut i).parse().ok(),
            "--rows" => a.rows = next(&mut i).parse().ok(),
            "--capture" => a.capture = Some(next(&mut i)),
            "--frames" => a.frames = next(&mut i).parse().unwrap_or(a.frames),
            "--fps" => a.fps = next(&mut i).parse::<f32>().unwrap_or(a.fps).clamp(1.0, 120.0),
            "--shade" => a.shade = true,
            "--seed" => a.seed = next(&mut i).parse().unwrap_or(a.seed),
            "--cell-aspect" => a.cell_aspect = next(&mut i).parse().ok(),
            other => {
                eprintln!("unknown argument {other}; --help lists them");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    a
}

fn load(a: &Args) -> Image {
    let path = match &a.image {
        Some(p) => p.clone(),
        None => match fetch::painting() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
    };
    match Image::load(&path) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let a = parse();
    let painting = load(&a);
    let mut scene = Scene::new(&painting, a.seed);
    scene.cell_aspect = a.cell_aspect.unwrap_or(if a.capture.is_some() { SVG_CELL_ASPECT } else { 2.0 });
    match &a.capture {
        Some(dir) => record(&a, &mut scene, dir),
        None => watch(&a, &mut scene),
    }
}

/// Every frame to disk at an exact clock: frame `n` is `n / fps` seconds in,
/// however long it took to draw, so the result plays back at the speed it
/// would have run.
fn record(a: &Args, scene: &mut Scene, dir: &str) {
    let (cols, rows) = (a.cols.unwrap_or(160), a.rows.unwrap_or(60));
    let mut g = Grid::new(cols, rows);
    g.shade = a.shade;
    for n in 0..a.frames {
        g.clear();
        scene.draw(&mut g, n as f32 / a.fps);
        let title = format!("Living Starry Night — frame {n:05}");
        capture::write_frame(&g, dir, &format!("starry-{n:05}"), &title);
    }
    eprintln!(
        "wrote {} frames ({cols}x{rows}, {:.1} s at {} fps) to {dir}/starry-%05d.svg (+ .txt)\n\
         assemble with:\n  ffmpeg -y -framerate {} -i {dir}/starry-%05d.svg -c:v libx264 \
         -pix_fmt yuv420p -crf 18 starry-night.mp4",
        a.frames,
        a.frames as f32 / a.fps,
        a.fps,
        a.fps
    );
}

fn watch(a: &Args, scene: &mut Scene) {
    let term = match RawTerm::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot take the terminal: {e}");
            std::process::exit(1);
        }
    };
    let mut kb = Keyboard::new(term.kitty, KEYS);
    let size = |a: &Args| {
        let (tc, tr) = terminal_size();
        (a.cols.unwrap_or(tc), a.rows.unwrap_or(tr.saturating_sub(1)).max(4))
    };
    let (mut cols, mut rows) = size(a);
    let mut g = Grid::new(cols, rows);
    g.shade = a.shade;
    let mut pacer = Pacer::new(Duration::from_secs_f32(1.0 / a.fps), 0.1);
    let mut started = Instant::now();
    let mut ansi = String::new();
    let mut out = std::io::stdout();
    let mut note = String::new();

    loop {
        kb.poll();
        if kb.tapped(3) || kb.tapped(27) || kb.tapped(b'q' as u32) {
            break;
        }
        if kb.tapped(b'b' as u32) {
            g.shade = !g.shade;
        }
        if kb.tapped(b'r' as u32) {
            started = Instant::now();
        }
        if kb.tapped(b'p' as u32) {
            let stem = capture::write_frame(&g, "out", "starry-snapshot", "Living Starry Night — snapshot");
            note = format!("  wrote {stem}.svg");
        }
        let (c, r) = size(a);
        if (c, r) != (cols, rows) {
            (cols, rows) = (c, r);
            g.resize(cols, rows);
            let _ = out.write_all(b"\x1b[2J");
        }

        pacer.begin();
        g.clear();
        scene.draw(&mut g, started.elapsed().as_secs_f32());
        grid_to_ansi(&g, &mut ansi);
        let hud = format!(
            "\x1b[{};1H\x1b[0m\x1b[2K\x1b[38;2;120;150;210m The Starry Night, 1889 \
             \x1b[38;2;90;110;140m· {:.0} fps · {}x{} · B shade {} · R again · P save · Q quit{}\x1b[0m",
            rows + 1,
            pacer.fps(),
            cols,
            rows,
            if g.shade { "on" } else { "off" },
            note,
        );
        let _ = out.write_all(ansi.as_bytes());
        let _ = out.write_all(hud.as_bytes());
        let _ = out.flush();
        if pacer.count_frame() && started.elapsed() > Duration::from_secs(3) {
            note.clear();
        }
        pacer.rest();
    }
    let fps = pacer.fps();
    drop(term);
    println!("{fps:.0} fps at {cols}x{rows}.");
}
