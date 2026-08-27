//! AsciiWorldEngine — the native terminal binary.
//!
//!   cargo run --release                 walk the city
//!   cargo run --release -- --bench      per-frame cost, honestly split
//!   cargo run --release -- --vista      one skyline frame -> .svg / .txt
//!   cargo run --release -- --capture    a scripted walk -> frames on disk
//!   cargo run --release -- --film       record a walk, every frame -> ffmpeg
//!
//! Nothing here decides anything about the picture. It reads keys, hands the
//! engine a `dt` and a bitmask, and paints the buffer that comes back.

use std::io::Write;
use std::time::{Duration, Instant};

use asciicity::camera::key;
use asciicity::rng::Rng;
use asciicity::term::{terminal_size, Keyboard, RawTerm};
use asciicity::world::World;
use asciicity::palette::PlateSource;
use asciicity::{grid_to_ansi, grid_to_svg, grid_to_text, Camera, Engine};

/// A terminal character is about twice as tall as it is wide. That ratio is
/// the whole of what the projection needs to know about the display.
const CELL_W: f32 = 1.0;
const CELL_H: f32 = 2.0;

struct Args {
    cols: Option<usize>,
    rows: Option<usize>,
    seed: u32,
    steps: usize,
    yaw: Option<f32>,
    pitch: f32,
    eye: Option<f32>,
    out: String,
    name: String,
    frames: usize,
    weather: asciicity::entities::Weather,
    /// Registrations for the traffic, in the order they were given. Empty
    /// means nobody supplied a list and the seed generates one.
    plates: Vec<String>,
    plates_on: bool,
    /// How much the facade generator may vary between neighbours, 0..1.
    /// One is the look this city has always had.
    variety: f32,
    /// `--film`: the script to play, and how many ticks a second it plays at.
    /// One tick is one frame, so `fps` is both the engine's `dt` and the rate
    /// the film is meant to be played back at — which is what makes a walk on
    /// screen last as long as the walk in the script.
    script: Option<String>,
    fps: f32,
    /// Bench from INSIDE a room rather than on the street. Both numbers are
    /// worth having and they are not the same number.
    indoors: bool,
    /// A fixed camera position for `--vista`, instead of the sightline search.
    /// What makes two settings of `--variety` comparable: the same seed under
    /// two settings is not quite the same city, so a search would stand in two
    /// different places and the pictures would not be of the same thing.
    at: Option<(f32, f32)>,
}

fn main() {
    let mut a = Args {
        cols: None,
        rows: None,
        seed: 0xACC17,
        steps: 260,
        yaw: None,
        pitch: 0.0,
        eye: None,
        out: "frames".into(),
        name: "vista".into(),
        frames: 400,
        weather: asciicity::entities::Weather::Clear,
        plates: Vec::new(),
        plates_on: true,
        variety: 1.0,
        script: None,
        fps: 30.0,
        indoors: false,
        at: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = "play";
    let mut demo = false;
    let mut i = 0;
    while i < argv.len() {
        let s = argv[i].as_str();
        let next = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_default()
        };
        match s {
            "--help" | "-h" => return help(),
            "--bench" => mode = "bench",
            "--demo" | "--wander" => demo = true,
            "--vista" => mode = "vista",
            "--capture" => mode = "capture",
            "--film" => mode = "film",
            "--script" => a.script = Some(next(&mut i)),
            "--print-script" => {
                print!("{}", asciicity::film::DEFAULT_SCRIPT);
                return;
            }
            "--fps" => a.fps = next(&mut i).parse().unwrap_or(a.fps),
            "--plate-shot" => mode = "plate-shot",
            "--doorway" => mode = "doorway",
            "--indoors" => a.indoors = true,
            "--cols" => a.cols = next(&mut i).parse().ok(),
            "--rows" => a.rows = next(&mut i).parse().ok(),
            "--seed" => a.seed = next(&mut i).parse().unwrap_or(a.seed),
            "--steps" => a.steps = next(&mut i).parse().unwrap_or(a.steps),
            "--frames" => a.frames = next(&mut i).parse().unwrap_or(a.frames),
            "--yaw" => a.yaw = next(&mut i).parse().ok(),
            "--pitch" => a.pitch = next(&mut i).parse().unwrap_or(0.0),
            "--eye" => a.eye = next(&mut i).parse().ok(),
            "--out" => a.out = next(&mut i),
            "--name" => a.name = next(&mut i),
            // A list on the command line, and a list in a file, because a
            // real list is long and you want to point at it. Both flags may be
            // given, and either may be given more than once; the entries all
            // land in one pool.
            "--plates" => {
                for part in next(&mut i).split(',') {
                    a.plates.push(part.to_string());
                }
            }
            "--plates-file" => {
                let path = next(&mut i);
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        for line in text.lines() {
                            let line = line.split('#').next().unwrap_or("").trim();
                            if !line.is_empty() {
                                a.plates.push(line.to_string());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("cannot read {path}: {e}");
                        std::process::exit(2);
                    }
                }
            }
            "--no-plates" => a.plates_on = false,
            "--variety" => a.variety = next(&mut i).parse().unwrap_or(a.variety).clamp(0.0, 1.0),
            "--at" => {
                let v = next(&mut i);
                let mut p = v.split(',').filter_map(|t| t.trim().parse::<f32>().ok());
                a.at = match (p.next(), p.next()) {
                    (Some(x), Some(z)) => Some((x, z)),
                    _ => {
                        eprintln!("--at wants X,Z");
                        std::process::exit(2);
                    }
                };
            }
            "--weather" => {
                a.weather = match next(&mut i).as_str() {
                    "rain" => asciicity::entities::Weather::Rain,
                    "downpour" | "storm" => asciicity::entities::Weather::Downpour,
                    _ => asciicity::entities::Weather::Clear,
                }
            }
            other => {
                eprintln!("unknown flag {other} (try --help)");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    match mode {
        "bench" => bench(&a),
        "vista" => still(&a),
        "capture" => capture(&a),
        "film" => film(&a),
        "plate-shot" => plate_shot(&a),
        "doorway" => doorway(&a),
        _ => play(&a, demo),
    }
}

fn help() {
    println!(
        "AsciiWorldEngine — a walkable ASCII city. World, movement, projection and\n\
         renderer are all Rust; this binary only reads keys and paints bytes.\n\n\
         USAGE\n  \
           asciicity                 walk the city (sizes itself to the terminal)\n  \
           asciicity --demo          the city walks itself; any key takes over\n  \
           asciicity --bench         per-frame cost: sim / cast / render / paint\n  \
           asciicity --vista         one skyline frame -> .svg and .txt\n  \
           asciicity --capture       a scripted walk -> frames on disk\n  \
           asciicity --film          record a walk: EVERY frame -> ffmpeg\n  \
           asciicity --plate-shot    near / middle / far plate evidence frames\n  \
           asciicity --doorway       approach / threshold / inside / window /\n      \
             back out — evidence frames for the doors and the rooms\n\n\
         KEYS\n  \
           W A S D / arrows   walk and strafe        J K   turn\n  \
           R F                look up / down         space or shift   sprint\n  \
           E C / PgUp PgDn    rise / sink            V     street <-> elevated vista\n  \
           T                  weather: clear / rain / downpour\n  \
           Tab                lock a walk on, so you can look around while moving\n  \
           M                  hand over to the autopilot (any other key takes back)\n  \
           P                  write the frame to disk\n  \
           Q or Esc           quit\n\n\
         RECORDING A FILM\n  \
           --film             play a script and write a numbered frame for\n      \
             EVERY tick, ready for ffmpeg. It drives the engine the way you\n      \
             do — the same key bitmask, live camera, live weather, live\n      \
             traffic — so `vista` in a script PRESSES V and the eye rises to\n      \
             the skyline, it does not set an eye height. Unlike --vista, the\n      \
             camera never jumps and --weather is honoured.\n  \
           --script FILE      the script to play (`-` reads stdin). With none\n      \
             given it plays a built-in reel: walk, look up at the towers,\n      \
             press V, hold on the skyline.\n  \
           --print-script     print that reel so you can edit it:\n      \
               asciicity --print-script > myfilm.txt\n  \
           --fps N            ticks a second, default 30. One tick is one\n      \
             frame, so played back at --fps the film runs at the speed it was\n      \
             walked. --name and --out set where the frames land.\n\n  \
           A script is one beat per line: a duration, then whatever is held\n  \
           down for it.\n\n      \
               2s   wait\n      \
               7s   walk                 # 3.2 units/s, a walk not a rush\n      \
               4s   walk look-up\n      \
               1s   vista wait           # press V\n      \
               9s   wait                 # hold on the skyline\n\n  \
           Durations: 4s seconds (the default), 250ms, or 120f exact frames.\n  \
           Held: walk back sprint wait · strafe-left strafe-right\n      \
             turn-left turn-right · look-up look-down level · rise sink\n  \
           Once, at the start of a beat: vista · weather clear|rain|downpour\n  \
           Blank lines and #-comments are ignored.\n\n\
         FLAGS\n  \
           --cols N --rows N --seed N --steps N --frames N\n  \
           --yaw RAD --pitch RAD --eye UNITS --out DIR --name NAME\n  \
           --at X,Z           stand here for --vista instead of searching for\n      \
             a long sightline. Two runs that differ in --variety or --seed are\n      \
             only comparable if they are pictures of the same place.\n  \
           --weather clear|rain|downpour\n  \
           --indoors          bench from inside a room instead of on the\n      \
             street. Both are real frames and they do not cost the same.\n\n\
         FACADE VARIETY\n  \
           --variety 0..1     how much the generator may vary between\n      \
             NEIGHBOURING plots. A --seed has always chosen WHICH mix of\n      \
             facades you get; this chooses HOW MUCH mixing there is.\n      \
             1 (the default) is the look this city has always had: every\n      \
             plot picks its own window lattice, colour family, roof shape\n      \
             and plot split. Turn it down and those choices are shared\n      \
             across a district — at 0, an 8-by-8 block district reads as\n      \
             one big regular grid. Building heights are left alone at\n      \
             every setting; it is pattern and colour that go uniform.\n\n\
         REGISTRATION PLATES\n  \
           Every car on the road carries one: bold black on plate yellow at\n  \
           the back, bold black on white at the front, bordered and with a\n  \
           margin so the bodywork does not crowd it, and two rows deep on a\n  \
           car with the height to carry them. Close up it is real text; at\n  \
           middle distance it degrades to a plate-shaped panel rather than to\n  \
           text that might read as some other registration; far off there is\n  \
           nothing. A car keeps its plate for as long as it is on screen, and\n  \
           the same --seed always hands the same cars the same plates.\n\n  \
           --plates \"AB12 CDE,K9 PAW,1 RG\"\n      \
             a comma-separated list. May be given more than once.\n  \
           --plates-file FILE\n      \
             one registration per line; blank lines and #-comments skipped.\n      \
             Both flags may be used together and the entries all pool.\n  \
           --no-plates\n      \
             do not draw plates at all.\n\n  \
           Entries are folded to upper case and anything a plate cannot carry\n  \
           is dropped, so \"ab12-cde\" and \"AB12 CDE\" are the same plate. A\n  \
           plate is cut to 10 characters.\n\n  \
           WITH NO LIST GIVEN the traffic carries the committed default in\n  \
           src/registrations.txt — real registrations, not generated\n  \
           placeholders. Edit that file to change the stock; no flag needed.\n"
    );
}

fn make(a: &Args, cols: usize, rows: usize) -> Engine {
    let mut e = Engine::with_variety(cols, rows, CELL_W, CELL_H, a.seed, a.variety);
    e.set_weather(a.weather);
    e.set_plates_on(a.plates_on);
    if !a.plates.is_empty() {
        let (plates, dropped) = asciicity::palette::Plates::from_list(&a.plates);
        if dropped > 0 {
            eprintln!("{dropped} plate(s) had nothing usable in them and were skipped");
        }
        eprintln!("{} registration(s) on the road", plates.len());
        e.set_plates(plates);
    }
    e
}

/// One line saying where the plates came from, for the modes that write to a
/// terminal rather than take one over. Generated plates are said to be
/// generated: they are plausible patterns, not anybody's registration.
fn plate_note(a: &Args, eng: &Engine) -> String {
    if !a.plates_on {
        return "plates off (--no-plates)".into();
    }
    match eng.pop.plates.source {
        PlateSource::Generated => format!(
            "{} plates GENERATED from seed {} — plausible patterns, not real \
registrations. Pass --plates or --plates-file for your own.",
            eng.pop.plates.len(),
            a.seed
        ),
        PlateSource::Default => format!(
            "{} default plates (src/registrations.txt) — pass --plates or \
--plates-file for your own.",
            eng.pop.plates.len()
        ),
        PlateSource::Supplied => format!("{} supplied plates", eng.pop.plates.len()),
    }
}

// --- interactive ---------------------------------------------------------
fn play(a: &Args, demo: bool) {
    let term = match RawTerm::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot take the terminal: {e}");
            std::process::exit(1);
        }
    };
    let mut kb = Keyboard::new(term.kitty);
    let (mut tc, mut tr) = terminal_size();
    let (mut cols, mut rows) = (a.cols.unwrap_or(tc), a.rows.unwrap_or(tr.saturating_sub(1)).max(8));
    let mut eng = make(a, cols, rows);
    // How long the last input keeps commanding the camera after it stops
    // arriving. With real key releases that is zero — everything starts and
    // stops with your finger. Without them the keyboard goes silent for the
    // whole autorepeat delay while your finger is still down, and this is what
    // carries the camera across that hole. The width of the hole is measured
    // off the player's own keyboard rather than guessed, so it is re-read every
    // frame. See `term/input.rs`.
    eng.cam.glide = kb.glide_needed();
    let mut ansi = String::new();
    let mut out = std::io::stdout();
    let mut last = Instant::now();
    let mut paint_us = 0.0f32;
    let mut fps_t = Instant::now();
    let mut fps_n = 0u32;
    let mut fps = 0.0f32;
    let mut note = String::new();
    let mut auto_walk = false;
    let mut pilot = if demo { Some(Autopilot::new(a.seed as u64)) } else { None };
    // Shown until the player touches anything, or long enough to have read it.
    let mut advice = if term.kitty { String::new() } else { fallback_advice() };
    let started = Instant::now();

    loop {
        kb.poll();
        if kb.tapped(3) || kb.tapped(27) || kb.tapped(b'q' as u32) {
            break;
        }
        // Any key at all takes the wheel back off the autopilot. `M` hands it
        // over again.
        if kb.any_key() {
            if kb.tapped(b'm' as u32) {
                pilot = Some(pilot.take().unwrap_or_else(|| Autopilot::new(a.seed as u64)));
                if pilot.is_some() {
                    eng.cam.halt();
                }
            } else if pilot.is_some() {
                pilot = None;
                eng.cam.halt();
                note = "  you have the controls".into();
            }
            advice.clear();
        }
        if !advice.is_empty() && started.elapsed() > Duration::from_secs(14) {
            advice.clear();
        }
        if kb.tapped(b'v' as u32) {
            eng.cam.toggle_vista();
        }
        if kb.tapped(9) {
            auto_walk = !auto_walk;
        }
        if kb.tapped(b't' as u32) {
            note = format!("  weather: {}", eng.cycle_weather());
        }
        if kb.tapped(b'p' as u32) {
            let stem = write_frame(&eng, &a.out, "snapshot", "AsciiWorldEngine — snapshot");
            note = format!("  wrote {stem}.svg");
        }

        let (nc, nr) = terminal_size();
        if (nc, nr) != (tc, tr) {
            tc = nc;
            tr = nr;
            cols = a.cols.unwrap_or(tc);
            rows = a.rows.unwrap_or(tr.saturating_sub(1)).max(8);
            eng.resize(cols, rows, CELL_W, CELL_H);
            let _ = out.write_all(b"\x1b[2J");
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;
        eng.cam.glide = kb.glide_needed();
        if let Some(p) = pilot.as_mut() {
            if p.wants_weather_change(dt) {
                eng.cycle_weather();
            }
        }
        let bits = match pilot.as_mut() {
            Some(p) => p.drive(&eng.world, &eng.cam, dt),
            None => {
                let mut b = kb.bits();
                // A locked walk is the one thing that works the same in both
                // input modes: it needs no key held, so looking around while
                // moving costs nothing and cannot be taken away by a terminal
                // that will not report releases.
                if auto_walk {
                    if b & key::BACK != 0 {
                        auto_walk = false;
                    } else {
                        b |= key::FWD;
                    }
                }
                b
            }
        };
        eng.step(dt, bits, 0.0, 0.0);
        eng.render();

        let t = Instant::now();
        grid_to_ansi(&eng.grid, &mut ansi);
        let s = &eng.stats;
        let mode = match pilot.as_ref() {
            Some(p) => format!("demo: {}", p.label()),
            None if auto_walk => "walk locked (Tab)".into(),
            None if term.kitty => "key-hold".into(),
            None if kb.latched() => "autorepeat · held".into(),
            None => "autorepeat".into(),
        };
        // Where you are is part of the state, so the HUD says it. Indoors
        // that is the room's own name and whatever is within reach of you —
        // both read off the world model, not off the picture.
        let where_now = match eng.room() {
            Some(r) => match eng.interaction() {
                Some((f, _)) => format!("{} · {} {}", r.label_str(), f.kind.verb(), f.kind.label()),
                None => r.label_str().to_string(),
            },
            None => eng.weather_name().to_string(),
        };
        let hud = format!(
            "\x1b[{};1H\x1b[0m\x1b[2K\x1b[38;2;120;160;200m\
             {:.1},{:.1} yaw {:+.2} eye {:.1}  \x1b[38;2;150;220;170m\
             sim {:.2} cast {:.2} render {:.2} paint {:.2} ms  {:.0} fps\
             \x1b[38;2;90;110;140m  {} hits · {}x{} · {} · {}{}\x1b[0m",
            rows + 1,
            eng.cam.x, eng.cam.z, eng.cam.yaw, eng.cam.eye,
            s.sim_us / 1000.0, s.cast_us / 1000.0, s.render_us / 1000.0, paint_us / 1000.0,
            fps, eng.hit_count(), cols, rows, where_now, mode,
            note,
        );
        let _ = out.write_all(ansi.as_bytes());
        let _ = out.write_all(hud.as_bytes());
        if !advice.is_empty() {
            let _ = out.write_all(advice.as_bytes());
        }
        let _ = out.flush();
        paint_us = t.elapsed().as_secs_f32() * 1e6;

        fps_n += 1;
        if fps_t.elapsed() > Duration::from_millis(500) {
            fps = fps_n as f32 / fps_t.elapsed().as_secs_f32();
            fps_t = Instant::now();
            fps_n = 0;
            note.clear();
        }
        let spent = now.elapsed();
        if spent < Duration::from_millis(16) {
            std::thread::sleep(Duration::from_millis(16) - spent);
        }
    }
    let degraded = !term.kitty;
    drop(term);
    println!("walked to {:.0},{:.0}. {:.0} fps at {cols}x{rows}.", eng.cam.x, eng.cam.z, fps);
    if degraded {
        // The alternate screen took the on-screen notice with it when we left,
        // so leave the remedy behind where it can be copied and pasted.
        println!();
        println!("This terminal did not report key releases, so the controls ran in fallback mode.");
        for line in remedy_lines() {
            println!("  {line}");
        }
    }
}

/// Does this terminal report key releases, and if not, what should be done
/// about it? The answer differs depending on whether tmux is in the way,
/// because inside tmux it is tmux that is swallowing them, not the terminal.
fn remedy_lines() -> Vec<String> {
    let mut v = Vec::new();
    if std::env::var_os("TMUX").is_some() {
        v.push("tmux is swallowing them. Turn them on, then restart this program:".to_string());
        v.push("    tmux set -g extended-keys on".to_string());
        v.push("(add it to ~/.tmux.conf to keep it), or run outside tmux in a".to_string());
        v.push("terminal that speaks the kitty keyboard protocol: WezTerm, kitty,".to_string());
        v.push("ghostty, foot, or a recent Alacritty.".to_string());
    } else {
        v.push("Run it in a terminal that speaks the kitty keyboard protocol:".to_string());
        v.push("WezTerm, kitty, ghostty, foot, or a recent Alacritty.".to_string());
        v.push("If you are in a multiplexer, it may be the one dropping them —".to_string());
        v.push("under tmux: tmux set -g extended-keys on".to_string());
    }
    v
}

/// The panel that says, on screen and unprompted, why the controls are not the
/// good ones and exactly how to get the good ones. Nobody should have to guess
/// that their terminal is the problem.
fn fallback_advice() -> String {
    let mut body = vec![
        "CONTROLS ARE IN FALLBACK MODE".to_string(),
        String::new(),
        "This terminal does not report key RELEASES, so holding a key is".to_string(),
        "inferred rather than known.".to_string(),
        String::new(),
    ];
    body.extend(remedy_lines());
    body.push(String::new());
    body.push("Until then: movement glides so a hold does not stutter, and".to_string());
    body.push("Tab locks a walk on so you can look around while moving.".to_string());
    body.push(String::new());
    body.push("Any key dismisses this.".to_string());

    let w = body.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 2;
    let mut s = String::new();
    // Draw it at a fixed spot near the top-left, over whatever is behind it.
    let (r0, c0) = (2usize, 4usize);
    s.push_str(&format!("\x1b[{};{}H\x1b[38;2;255;210;120m┌{}┐", r0, c0, "─".repeat(w)));
    for (i, line) in body.iter().enumerate() {
        let pad = w - 1 - line.chars().count();
        let colour = if i == 0 { "\x1b[1;38;2;255;235;180m" } else { "\x1b[38;2;225;200;150m" };
        s.push_str(&format!(
            "\x1b[{};{}H\x1b[38;2;255;210;120m│ {colour}{line}{}\x1b[38;2;255;210;120m│",
            r0 + 1 + i,
            c0,
            " ".repeat(pad)
        ));
    }
    s.push_str(&format!(
        "\x1b[{};{}H\x1b[38;2;255;210;120m└{}┘\x1b[0m",
        r0 + 1 + body.len(),
        c0,
        "─".repeat(w)
    ));
    s
}


// --- the autopilot -------------------------------------------------------
/// A city that walks itself.
///
/// It drives the SAME input bitmask a player's keyboard produces and reads the
/// SAME world the renderer does, so it goes through every line of the camera,
/// collision and projection code that you do. Nothing about the picture knows
/// it is not a person at the keys — which is the only way an attract mode is
/// worth anything as a demo of the real thing.
///
/// Three things run at once and on their own clocks, because a wanderer that
/// walks, *then* turns, *then* looks reads as a script. A person walks and
/// turns and looks up all at the same time.
struct Autopilot {
    rng: Rng,
    /// What the feet are doing, and for how long.
    step: Step,
    step_left: f32,
    /// What the head is doing, and for how long.
    gaze: Gaze,
    gaze_left: f32,
    /// Where in the street-to-skyline cycle we are.
    alt: Alt,
    alt_left: f32,
    /// Seconds until it changes the weather. An attract mode that never shows
    /// the rain is not showing you the city.
    weather_left: f32,
    /// Seconds left of looking round a room before heading for the door.
    /// Reset every frame it is outdoors, so it is always full on walking in.
    indoor_left: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    /// Walk, optionally leaning into a turn — a curve, not a corner.
    Walk { sprint: bool, turn: i8 },
    /// Swing on the spot. `until_clear` keeps swinging until there is somewhere
    /// worth walking, which is what stops it dithering in a corner.
    Pivot { right: bool, until_clear: bool },
    /// Stand and take it in.
    Stand,
}

#[derive(Clone, Copy, PartialEq)]
enum Gaze {
    Level,
    Up,
    Down,
}

#[derive(Clone, Copy, PartialEq)]
enum Alt {
    Street,
    Rising,
    High,
    Sinking,
}

/// How far ahead the autopilot looks before committing to keep walking.
const LOOKAHEAD: f32 = 34.0;
/// Closer than this and it must turn or it will walk into a facade.
const TOO_CLOSE: f32 = 3.2;
/// A forced turn keeps going until there is at least this much road ahead.
const OPEN_ENOUGH: f32 = 11.0;

impl Autopilot {
    fn new(seed: u64) -> Self {
        Autopilot {
            rng: Rng::new(seed ^ 0x5EED_10AD),
            step: Step::Walk { sprint: false, turn: 0 },
            step_left: 2.0,
            gaze: Gaze::Level,
            gaze_left: 1.5,
            alt: Alt::Street,
            alt_left: 40.0,
            weather_left: 30.0,
            indoor_left: 2.5,
        }
    }

    /// Uniform in `[lo, hi)`.
    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rng.f32()
    }

    /// Clear distance from the camera along a heading, in world units, capped
    /// at `LOOKAHEAD`. Stepped at half a cell so it cannot tunnel a corner.
    fn clearance(world: &World, cam: &Camera, yaw: f32) -> f32 {
        let (dx, dz) = (yaw.sin(), -yaw.cos());
        let mut d = 0.5f32;
        while d < LOOKAHEAD {
            if world.solid((cam.x + dx * d).floor() as i32, (cam.z + dz * d).floor() as i32) {
                return d;
            }
            d += 0.5;
        }
        LOOKAHEAD
    }

    /// Is it time to change the weather? Asked once a frame; the caller owns
    /// the engine, so it does the changing.
    fn wants_weather_change(&mut self, dt: f32) -> bool {
        self.weather_left -= dt;
        if self.weather_left > 0.0 {
            return false;
        }
        self.weather_left = self.between(38.0, 75.0);
        true
    }

    /// One tick. Returns the key bitmask a player would be holding.
    fn drive(&mut self, world: &World, cam: &Camera, dt: f32) -> u32 {
        // **Indoors, the only plan is the way out.** An attract mode that walks
        // into a shop is a good attract mode; one that gets stuck in the back
        // of it turning in circles is the one failure this thing cannot have,
        // and the wall-clearance wander has no idea a doorway is special. So
        // while it is inside, it steers at the door it came in by and walks —
        // which shows the room for a couple of seconds on the way through and
        // cannot wedge, because the door is the one cell it is aiming at.
        if let Some(room) = world.interior() {
            self.indoor_left -= dt;
            self.alt = Alt::Street;
            self.alt_left = self.alt_left.max(6.0);
            if self.indoor_left > 0.0 {
                // Long enough to look at the place before turning round.
                return key::TURN_R;
            }
            let (fwd, turn) = room.way_out(cam.x, cam.z, cam.yaw);
            return if fwd { key::FWD } else { 0 }
                | match turn {
                    1 => key::TURN_R,
                    -1 => key::TURN_L,
                    _ => 0,
                };
        }
        self.indoor_left = self.between(1.6, 3.4);
        self.step_left -= dt;
        self.gaze_left -= dt;
        self.alt_left -= dt;

        // --- altitude: street level, with an occasional trip to the skyline.
        // Above the rooftops there is nothing to collide with, so coming down
        // is the one move that can end badly: land on a plot and you are inside
        // a building with the collision switched back on. So the descent is
        // only ever STARTED over open ground, and once started the camera holds
        // still until it has landed.
        // A descent runs until it has LANDED, not until its timer says so: the
        // eye eases toward its target, so how long the fall takes is not
        // knowable in advance, and ending it early leaves the camera hanging in
        // mid-air over the street for the rest of the run. This has to come
        // before the transition below, or the timer wins.
        let ground = asciicity::camera::EYE_STREET;
        if self.alt == Alt::Sinking && (cam.eye > ground + 0.05 || cam.eye_target > ground + 0.01) {
            self.alt_left = self.alt_left.max(0.05);
        }
        if self.alt_left <= 0.0 {
            // Not merely open — on a STREET. A block's interior courtyard is
            // open ground too, and landing in one drops the walk into a pocket
            // with no way out of it.
            let c = world.cell(cam.x.floor() as i32, cam.z.floor() as i32);
            let over_ground = c.height == 0 && c.cross != 255;
            let (next, secs) = match self.alt {
                Alt::Street => (Alt::Rising, self.between(2.2, 3.4)),
                Alt::Rising => (Alt::High, self.between(9.0, 16.0)),
                Alt::High if !over_ground => (Alt::High, self.between(0.8, 1.8)),
                Alt::High => (Alt::Sinking, self.between(2.4, 3.6)),
                Alt::Sinking => (Alt::Street, self.between(45.0, 95.0)),
            };
            self.alt = next;
            self.alt_left = secs;
        }

        // --- feet.
        let ahead = Self::clearance(world, cam, cam.yaw);
        // A pivot that was forced by a wall keeps going until it has actually
        // found somewhere to walk. Turning for a fixed time instead is how a
        // wanderer ends up rocking side to side in a corner for ever, which is
        // the one failure an attract mode cannot have.
        if let Step::Pivot { until_clear: true, .. } = self.step {
            if ahead > OPEN_ENOUGH {
                self.step_left = 0.0;
            } else {
                self.step_left = self.step_left.max(0.05);
            }
        }
        let boxed_in = ahead < TOO_CLOSE && !cam.airborne();
        if boxed_in && !matches!(self.step, Step::Pivot { until_clear: true, .. }) {
            // Turn toward whichever way is more open.
            let left = Self::clearance(world, cam, cam.yaw - core::f32::consts::FRAC_PI_2);
            let right = Self::clearance(world, cam, cam.yaw + core::f32::consts::FRAC_PI_2);
            self.step = Step::Pivot { right: right >= left, until_clear: true };
            // A full turn takes about 2.9 s; cap it so a genuinely sealed spot
            // cannot spin us for ever.
            self.step_left = 3.2;
            // Look where you are going.
            self.gaze = Gaze::Level;
            self.gaze_left = self.between(1.2, 2.2);
        } else if self.step_left <= 0.0 {
            let roll = self.rng.below(100);
            self.step = if cam.airborne() {
                // Up here there is nothing to walk into, so cruise and turn.
                Step::Walk {
                    sprint: roll < 45,
                    turn: if roll < 30 { -1 } else if roll < 60 { 1 } else { 0 },
                }
            } else if roll < 10 {
                Step::Stand
            } else if roll < 24 {
                Step::Pivot { right: roll & 1 == 0, until_clear: false }
            } else {
                Step::Walk {
                    sprint: roll > 82,
                    // A gentle lean most of the time; a straight run the rest.
                    turn: if roll < 34 { -1 } else if roll < 44 { 1 } else { 0 },
                }
            };
            self.step_left = match self.step {
                Step::Stand => self.between(1.0, 2.4),
                Step::Pivot { .. } => self.between(0.35, 0.9),
                Step::Walk { .. } => self.between(2.0, 5.5),
            };
        }

        // --- head. Looking up at a tower you are walking past is most of what
        // makes this read as somebody sightseeing rather than a camera on rails.
        if self.gaze_left <= 0.0 {
            let roll = self.rng.below(100);
            self.gaze = if cam.airborne() {
                if roll < 55 { Gaze::Down } else { Gaze::Level }
            } else if roll < 26 {
                Gaze::Up
            } else if roll < 34 {
                Gaze::Down
            } else {
                Gaze::Level
            };
            self.gaze_left = self.between(0.9, 2.6);
        }

        // Come straight down. Drifting during a descent is how you land on a
        // roof you were not over when you started.
        if self.alt == Alt::Sinking {
            self.step = Step::Stand;
            self.step_left = self.step_left.max(0.1);
        }

        let mut k = match self.step {
            Step::Stand => 0,
            Step::Pivot { right, .. } => {
                if right { key::TURN_R } else { key::TURN_L }
            }
            Step::Walk { sprint, turn } => {
                let mut b = key::FWD;
                if sprint {
                    b |= key::SPRINT;
                }
                if turn < 0 {
                    b |= key::TURN_L;
                } else if turn > 0 {
                    b |= key::TURN_R;
                }
                b
            }
        };
        k |= match self.gaze {
            Gaze::Up => key::LOOK_UP,
            Gaze::Down => key::LOOK_DOWN,
            // Bring the horizon back level rather than leaving it wherever the
            // last look left it.
            Gaze::Level => {
                if cam.pitch > 0.03 {
                    key::LOOK_DOWN
                } else if cam.pitch < -0.03 {
                    key::LOOK_UP
                } else {
                    0
                }
            }
        };
        k |= match self.alt {
            Alt::Rising => key::RISE,
            Alt::Sinking => key::SINK,
            _ => 0,
        };
        k
    }

    /// What the HUD says it is doing, so the mode is never a mystery.
    fn label(&self) -> &'static str {
        match (self.alt, self.step) {
            (Alt::Rising, _) => "rising",
            (Alt::Sinking, _) => "descending",
            (Alt::High, _) => "over the rooftops",
            (_, Step::Stand) => "taking it in",
            (_, Step::Pivot { .. }) => "turning",
            (_, Step::Walk { sprint: true, .. }) => "running",
            (_, Step::Walk { .. }) => "walking",
        }
    }
}

// --- stills --------------------------------------------------------------
/// Walk to a long sightline and write one frame. This is the frame to look at
/// when judging whether the picture is right.
fn still(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(80);
    let mut eng = make(a, cols, rows);
    if let Some(y) = a.yaw {
        eng.cam.yaw = y;
    }
    eng.cam.pitch = a.pitch;
    if let Some(e) = a.eye {
        eng.cam.eye = e;
        eng.cam.eye_target = e;
    }
    // Sprint down the avenue and keep the position with the longest sightline
    // seen along the way. A frame taken at an arbitrary step is usually one
    // facade filling the view; the picture worth judging is the one down a
    // long street, with depth in it.
    let mut best = (f32::NEG_INFINITY, eng.cam.x, eng.cam.z, eng.cam.yaw);
    for i in 0..if a.at.is_some() { 0 } else { a.steps } {
        eng.step(1.0 / 60.0, key::FWD | key::SPRINT, 0.0, 0.0);
        if i % 6 != 0 {
            continue;
        }
        eng.render();
        // Score the middle half of the frame. Two things make the picture:
        // the nearest facades should be a good way out (a skyline, not one wall
        // filling the view), and the frame should actually be FULL of city
        // rather than half black. Reward both, and prefer a sightline around
        // 75 units — far enough to see a skyline, near enough that the
        // storefront band is still a band.
        let lo = cols / 4;
        let hi = cols - cols / 4;
        let mut sum = 0.0;
        let mut covered = 0.0;
        for x in lo..hi {
            let d = eng.rays.column(x).first().map(|h| h.dist).unwrap_or(200.0);
            sum += d.min(200.0);
            if d < 150.0 {
                covered += 1.0;
            }
        }
        let n = (hi - lo) as f32;
        let mean = sum / n;
        let score = (covered / n) * (1.0 - ((mean - 75.0) / 75.0).abs()).max(0.0);
        if score > best.0 {
            best = (score, eng.cam.x, eng.cam.z, eng.cam.yaw);
        }
    }
    let (px, pz) = a.at.unwrap_or((best.1, best.2));
    eng.cam.x = px;
    eng.cam.z = pz;
    eng.cam.yaw = a.yaw.unwrap_or(best.3);
    eng.step(0.0, 0, 0.0, 0.0);
    eng.render();
    eprintln!("  sightline score {:.2} (coverage x closeness to a 75-unit sightline)", best.0);
    eprintln!("  {}", plate_note(a, &eng));
    let title = format!(
        "AsciiWorldEngine — {cols}x{rows}, {:.0},{:.0} yaw {:.2} eye {:.1}",
        eng.cam.x, eng.cam.z, eng.cam.yaw, eng.cam.eye
    );
    let stem = write_frame(&eng, &a.out, &a.name, &title);
    eprintln!("wrote {stem}.svg and {stem}.txt");
    eprintln!(
        "  sim {:.3} ms · cast {:.3} ms · render {:.3} ms · {} hits",
        eng.stats.sim_us / 1000.0,
        eng.stats.cast_us / 1000.0,
        eng.stats.render_us / 1000.0,
        eng.hit_count()
    );
}

fn write_frame(eng: &Engine, dir: &str, name: &str, title: &str) -> String {
    let _ = std::fs::create_dir_all(dir);
    let stem = format!("{dir}/{name}");
    let _ = std::fs::write(format!("{stem}.svg"), grid_to_svg(&eng.grid, title));
    let _ = std::fs::write(format!("{stem}.txt"), grid_to_text(&eng.grid));
    stem
}

/// A scripted walk: settle, walk, turn, sprint, then rise to the vista. The
/// same script every time, so two builds' captures are comparable.
fn capture(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(80);
    let mut eng = make(a, cols, rows);
    let script: &[(usize, u32, &str)] = &[
        (60, 0, "settle"),
        (120, key::FWD, "walk"),
        (70, key::TURN_R, "turn"),
        (180, key::FWD | key::SPRINT, "sprint"),
        (90, key::STRAFE_R, "strafe"),
        (120, key::RISE | key::FWD, "rise"),
    ];
    eprintln!("{}", plate_note(a, &eng));
    let mut n = 0usize;
    for &(steps, keys, label) in script {
        for _ in 0..steps {
            eng.step(1.0 / 60.0, keys, 0.0, 0.0);
            n += 1;
        }
        eng.render();
        let title = format!(
            "AsciiWorldEngine — frame {n:04} {label} — {:.0},{:.0} eye {:.1}",
            eng.cam.x, eng.cam.z, eng.cam.eye
        );
        let stem = write_frame(&eng, &a.out, &format!("walk-{n:04}-{label}"), &title);
        eprintln!("{stem}.svg");
    }
}

/// **Recording mode: the engine plays itself and every frame is written.**
///
/// This is `--capture` grown up, and it is deliberately not a camera path.
/// `--vista` searches for a dramatic sightline and puts the camera there, which
/// is exactly right for ONE picture and teleports between consecutive ones; it
/// also never touches the weather, so a film made out of it can never rain.
/// `--capture` walks a real route but writes one frame per segment, so it is a
/// contact sheet, not a film. And the autopilot (`--demo`) drives the engine
/// properly but chooses for itself, so it cannot be asked for a shot.
///
/// What this takes from each: the autopilot's MECHANISM — everything reaches
/// the engine as the `camera::key` bitmask a keyboard produces, so the camera,
/// the collision, the projection and the population are all live — and
/// `--capture`'s frame writer, unchanged. What it adds is a script somebody can
/// edit (`film.rs`) and a frame per tick.
///
/// The two things a script can do that are not a held key are done the way the
/// keyboard does them: `vista` calls the same `toggle_vista` the `V` key calls,
/// so the eye EASES up to `EYE_VISTA` on the camera's own time constant — about
/// half a second, and framerate-independent, so `--fps 60` gives the same rise
/// in the same wall-clock time with twice the frames in it — rather than
/// cutting there; and `weather` calls the same setter `--weather` does. Nothing
/// here sets an eye height, a pitch or a position behind the engine's back.
fn film(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(60);
    let text = match a.script.as_deref() {
        None => asciicity::film::DEFAULT_SCRIPT.to_string(),
        Some("-") => {
            let mut buf = String::new();
            if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
                eprintln!("cannot read the script from stdin: {e}");
                std::process::exit(2);
            }
            buf
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("cannot read {path}: {e}");
                eprintln!("`--print-script > myfilm.txt` writes a starting point to edit.");
                std::process::exit(2);
            }
        },
    };
    let script = match asciicity::film::parse(&text, a.fps) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    let mut eng = make(a, cols, rows);
    // A film starts where a session starts unless it is told otherwise, and
    // `--at` / `--yaw` are how it is told. Nothing else about the pose is set
    // from out here.
    if let Some((x, z)) = a.at {
        eng.cam.x = x;
        eng.cam.z = z;
    }
    if let Some(y) = a.yaw {
        eng.cam.yaw = y;
    }
    let dt = 1.0 / a.fps;
    let total = script.ticks();
    eprintln!(
        "{cols}x{rows} ({}x{} px a frame) · seed {} · weather {} · {:.0} fps · \
{} beats, {total} frames, {:.1}s",
        cols * 11,
        rows * 18,
        a.seed,
        eng.weather_name(),
        a.fps,
        script.beats.len(),
        total as f32 / a.fps
    );
    eprintln!("{}", plate_note(a, &eng));
    let (sx, sz) = (eng.cam.x, eng.cam.z);

    let mut n = 0usize;
    for (bi, beat) in script.beats.iter().enumerate() {
        // The presses, at the top of the beat, through the same calls the keys
        // make.
        if let Some(w) = beat.weather {
            eng.set_weather(w);
        }
        if beat.vista {
            eng.cam.toggle_vista();
        }
        let first = n + 1;
        for _ in 0..beat.ticks {
            let keys = beat.keys | if beat.level { asciicity::film::level_bits(eng.cam.pitch) } else { 0 };
            eng.step(dt, keys, 0.0, 0.0);
            eng.render();
            n += 1;
            let title = format!(
                "AsciiWorldEngine — film {n:06} · {:.2}s · beat {} `{}` — {:.1},{:.1} \
yaw {:+.2} eye {:.1} · {}",
                n as f32 / a.fps,
                bi + 1,
                beat.label,
                eng.cam.x,
                eng.cam.z,
                eng.cam.yaw,
                eng.cam.eye,
                eng.weather_name()
            );
            write_frame(&eng, &a.out, &format!("{}-{n:06}", a.name), &title);
        }
        // One line a beat, not one a frame: a per-frame log buries the run.
        eprintln!(
            "  beat {:>2}  frames {first:06}-{n:06}  {:<28}  {:.0},{:.0} eye {:.1}  {}",
            bi + 1,
            beat.label,
            eng.cam.x,
            eng.cam.z,
            eng.cam.eye,
            eng.weather_name()
        );
    }

    let walked = ((eng.cam.x - sx).powi(2) + (eng.cam.z - sz).powi(2)).sqrt();
    eprintln!(
        "\nwrote {n} frames to {}/{}-%06d.svg (+ .txt)\n  \
covered {walked:.0} units in {:.1}s — walking is {:.1} units/s, sprinting {:.1}",
        a.out,
        a.name,
        n as f32 / a.fps,
        asciicity::camera::WALK_SPEED,
        asciicity::camera::SPRINT_SPEED
    );
    // Frames left behind by a longer previous run would be silently picked up
    // by ffmpeg and tacked onto the end of this film, so say so rather than let
    // it happen.
    let stale: Vec<String> = std::fs::read_dir(&a.out)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| {
            f.strip_prefix(&format!("{}-", a.name))
                .and_then(|r| r.split('.').next())
                .and_then(|d| d.parse::<usize>().ok())
                .is_some_and(|k| k > n)
        })
        .collect();
    if !stale.is_empty() {
        eprintln!(
            "  WARNING: {} frame(s) from a longer earlier run are still in {} and \
ffmpeg would append them.\n  rm {}/{}-*",
            stale.len(),
            a.out,
            a.out,
            a.name
        );
    }
    eprintln!(
        "\n  ffmpeg -y -framerate {:.0} -start_number 1 -i {}/{}-%06d.svg \\\n    \
-c:v libx264 -pix_fmt yuv420p -crf 18 film.mp4",
        a.fps, a.out, a.name
    );
}

/// Evidence frames for the plates: one close enough to read, one at the
/// distance where a plate has stopped being text, and one far enough away that
/// there is no plate at all.
///
/// `--vista` and `--capture` cannot do this — both pick a frame on the shape of
/// the CITY (a long sightline, a scripted walk), and where the traffic happens
/// to be at that moment is luck. This drives the same sim and scores each frame
/// on the plates themselves, read straight off the grid's background plane:
/// a plate is the only thing in the whole frame that paints one, so the width
/// of a run of coloured background IS how legible a plate is on that frame.
/// Evidence frames for the doors and the rooms behind them: walking up to an
/// entrance, standing in the room, looking out of its window, and back on the
/// pavement.
///
/// `--vista` and `--capture` cannot do this and it is worth saying why, because
/// the same argument put `--plate-shot` here. Both of those pick their frame on
/// the shape of the CITY — a long sightline, a scripted walk — and whether an
/// entrance happens to be in view is luck. This picks its frames on the shape of
/// a DOORWAY: it finds one, walks in through it with the same keys a player
/// uses, and shoots at the four moments that show the feature.
fn doorway(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(60);
    let mut eng = make(a, cols, rows);

    // The nearest entrance to the spawn. `door_near` is a box scan and belongs
    // to the tools; nothing in the frame path ever looks for a door.
    let Some((dx, dz, face)) = eng.world.door_near(eng.cam.x, eng.cam.z, 160) else {
        eprintln!("no entrance within 160 cells of the spawn for seed {}", a.seed);
        std::process::exit(1);
    };
    let (ix, iz) = asciicity::interior::INWARD[face as usize];
    // forward = (sin yaw, -cos yaw), so this is the yaw that faces inward.
    let yaw_in = (ix as f32).atan2(-(iz as f32));
    eprintln!(
        "entrance at {dx},{dz} facing {}, {}",
        ["-X", "+X", "-Z", "+Z"][face as usize],
        plate_note(a, &eng)
    );

    let shot = |eng: &mut Engine, name: &str, what: &str| {
        eng.render();
        let title = format!(
            "AsciiWorldEngine — {what} — {:.0},{:.0} yaw {:.2}{}",
            eng.cam.x,
            eng.cam.z,
            eng.cam.yaw,
            match eng.room() {
                Some(r) => format!(" — {} — ceiling {:.1}", r.label_str(), r.ceiling),
                None => String::new(),
            }
        );
        let stem = write_frame(eng, &a.out, name, &title);
        eprintln!("{stem}.svg  {what}{}", if eng.world.indoors() { "  [indoors]" } else { "" });
    };

    // Stand back on the pavement, looking straight at the entrance.
    let stand = |eng: &mut Engine, back: f32| {
        eng.cam.x = dx as f32 + 0.5 - ix as f32 * back;
        eng.cam.z = dz as f32 + 0.5 - iz as f32 * back;
        eng.cam.yaw = yaw_in;
        eng.cam.pitch = 0.0;
        eng.cam.halt();
        eng.step(0.0, 0, 0.0, 0.0);
    };

    stand(&mut eng, 11.0);
    shot(&mut eng, "door-1-approach", "approaching the door");
    stand(&mut eng, 3.2);
    shot(&mut eng, "door-2-threshold", "at the threshold");

    // Walk in, with the keys a player would use, and keep going until we are
    // well clear of the doorway.
    // Stop a few paces in, where a room is a room. Walking until something
    // stops you leaves the camera against the back wall, which is a picture of
    // a wall and not a picture of a room.
    let mut entered = 0usize;
    for i in 0..900 {
        eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
        if eng.world.indoors() {
            entered += 1;
            let ahead = (
                (eng.cam.x + ix as f32 * 2.5).floor() as i32,
                (eng.cam.z + iz as f32 * 2.5).floor() as i32,
            );
            let room_ends = eng.room().is_some_and(|r| !r.open(ahead.0, ahead.1));
            if entered > 110 || (entered > 30 && room_ends) {
                break;
            }
        }
        if i == 899 {
            eprintln!("walked 900 frames at the door without getting in");
            std::process::exit(1);
        }
    }
    // Face the longest clear run in the room, the same way `camera::spawn`
    // faces the longest clear run down a street. A shot taken from wherever the
    // walk stopped is usually a picture of the back of a rack.
    if let Some(r) = eng.room() {
        let (px, pz) = (eng.cam.x, eng.cam.z);
        let mut best = (-1i32, eng.cam.yaw);
        for k in 0..16 {
            let yaw = k as f32 * core::f32::consts::TAU / 16.0;
            let (fx, fz) = (yaw.sin(), -yaw.cos());
            let mut run = 0;
            while run < 40 {
                let (gx, gz) = (
                    (px + fx * (run as f32 + 1.0) * 0.5).floor() as i32,
                    (pz + fz * (run as f32 + 1.0) * 0.5).floor() as i32,
                );
                if !r.open(gx, gz) {
                    break;
                }
                run += 1;
            }
            if run > best.0 {
                best = (run, yaw);
            }
        }
        eng.cam.yaw = best.1;
        eng.cam.halt();
    }
    eng.step(0.0, 0, 0.0, 0.0);
    if let Some(r) = eng.room() {
        // The plan, as a plan. A room read back as a picture is what makes a
        // layout obviously right or obviously wrong; a table of numbers is
        // not.
        eprintln!("  plan ('#' wall, '=' glazing, 'o' furniture, '+' you, '.' floor):");
        for gz in r.z0..r.z0 + r.wz {
            let mut line = String::from("    ");
            for gx in r.x0..r.x0 + r.wx {
                let here = eng.cam.x.floor() as i32 == gx && eng.cam.z.floor() as i32 == gz;
                let c = r.at(gx, gz).unwrap();
                line.push(if here {
                    '+'
                } else if c.door != 0 {
                    'D'
                } else if c.height == 0 {
                    '.'
                } else if c.win == asciicity::interior::fit::WINDOW {
                    '='
                } else if c.win == asciicity::interior::fit::WALL {
                    '#'
                } else {
                    'o'
                });
            }
            eprintln!("{line}");
        }
        eprintln!(
            "  inside: {:?} \"{}\" — {}x{} cells, ceiling {:.2}, floor {}, {} windows, {} fixtures",
            r.room,
            r.label_str(),
            r.wx,
            r.wz,
            r.ceiling,
            r.floor,
            r.windows.len(),
            r.props.len()
        );
    }
    shot(&mut eng, "door-3-inside", "inside the room");

    // Now the window. Stand a set distance BACK from the glazed street wall and
    // face it, so the frame carries the room as well as what is beyond it: the
    // sills, the piers between the bays, the ceiling overhead, and the city
    // outside. Pressed against the glass you get a picture of a street, which
    // proves the mechanism and shows none of the point of it.
    if let Some(r) = eng.room() {
        let back = 6.0f32.min((r.wx.min(r.wz) - 4) as f32);
        // The middle of the glazed run, so a pier is not filling the view.
        let (mx, mz) = (
            (r.x0 as f32 + (r.wx - 1) as f32 * 0.5) + 0.5,
            (r.z0 as f32 + (r.wz - 1) as f32 * 0.5) + 0.5,
        );
        let (dxf, dzf) = (dx as f32 + 0.5, dz as f32 + 0.5);
        // Along the wall take the room's middle; away from it, `back` in.
        eng.cam.x = if ix != 0 { dxf + ix as f32 * back } else { mx };
        eng.cam.z = if iz != 0 { dzf + iz as f32 * back } else { mz };
    }
    eng.cam.yaw = yaw_in + core::f32::consts::PI;
    eng.cam.halt();
    eng.step(0.0, 0, 0.0, 0.0);
    shot(&mut eng, "door-4-window", "looking out of the window");

    // And back out on to the pavement.
    for _ in 0..900 {
        eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
        if !eng.world.indoors() {
            break;
        }
    }
    for _ in 0..40 {
        eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
    }
    eng.cam.yaw = yaw_in + core::f32::consts::PI;
    eng.cam.halt();
    eng.step(0.0, 0, 0.0, 0.0);
    shot(&mut eng, "door-5-back-out", "back out on the street");
    if eng.world.indoors() {
        eprintln!("WARNING: never found the way back out");
    }
}

fn plate_shot(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(60);
    let mut eng = make(a, cols, rows);
    eprintln!("{}", plate_note(a, &eng));
    if !a.plates_on {
        eprintln!("nothing to shoot with --no-plates");
        return;
    }

    // best score, the frame's text, the frame's svg, a caption
    let mut shots: [(f32, String, String, String); 3] = Default::default();
    let steps = a.steps.max(2400);
    let mut pilot = Autopilot::new(a.seed as u64 ^ 0x9E37);
    // The shortest supplied registration decides where "readable" starts: a
    // panel narrower than that plus its two margin cells cannot be carrying
    // one, which is what separates the near band from the middle one.
    let readable_at = eng.pop.plates.readable_width();
    for i in 0..steps {
        // Walk for a while, then stand at the kerb and let the traffic come to
        // you — a car that passes at arm's length is where a plate is at its
        // most legible, and standing still is far likelier to produce one than
        // chasing traffic is. The pilot keeps ticking either way so it does not
        // wedge itself against a wall.
        let driving = pilot.drive(&eng.world, &eng.cam, 1.0 / 60.0);
        let keys = if (i / 150) % 2 == 0 { driving } else { 0 };
        eng.step(1.0 / 60.0, keys, 0.0, 0.0);
        // Plates are a street-level feature; the vista is not what is being
        // judged here, so hold the camera down on the pavement.
        eng.cam.eye = asciicity::camera::EYE_STREET;
        eng.cam.eye_target = asciicity::camera::EYE_STREET;
        eng.cam.pitch = 0.0;
        if i % 2 != 0 {
            continue;
        }
        eng.render();

        // Every run of plate on the frame, by width — and by how many
        // characters of REGISTRATION it is actually carrying, which is not the
        // same question: a two-letter private registration sits on a plate as
        // wide as an eight-character one, because that is what a private plate
        // looks like.
        //
        // A plate is made of the same coloured glyphs on black as everything
        // else now, so it is found by the mark the renderer left rather than by
        // looking for the one painted rectangle on the frame. A cell that is
        // one of the plate's own body characters is the plate; anything else on
        // it is the registration.
        let body = [
            asciicity::palette::PLATE_RULE,
            asciicity::palette::PLATE_CORNER,
            asciicity::palette::PLATE_UPRIGHT,
            asciicity::palette::PLATE_CAP_L,
            asciicity::palette::PLATE_CAP_R,
        ];
        let mut widest = 0usize;
        let mut carried = 0usize;
        let mut smudges = 0usize;
        for y in 0..eng.grid.rows {
            let mut x = 0usize;
            while x < eng.grid.cols {
                if !eng.grid.is_plate(x as i32, y as i32) {
                    x += 1;
                    continue;
                }
                let start = x;
                let mut glyphs = 0usize;
                while x < eng.grid.cols && eng.grid.is_plate(x as i32, y as i32) {
                    let c = eng.grid.ch[y * eng.grid.cols + x];
                    if c != b' ' && c != 0 && !body.contains(&c) {
                        glyphs += 1;
                    }
                    x += 1;
                }
                let n = x - start;
                widest = widest.max(n);
                carried = carried.max(glyphs);
                if n >= 3 && n < readable_at {
                    smudges += 1;
                }
            }
        }
        // How much traffic is out past the distance a plate survives to, and
        // how close the nearest car in shot is.
        let (fx, fz) = eng.cam.forward();
        let (rx, rz) = eng.cam.right();
        let mut far_traffic = 0usize;
        let mut nearest = f32::INFINITY;
        let mut tallest = 0.0f32;
        for v in &eng.pop.vehs {
            let (tx, tz) = (v.x - eng.cam.x, v.z - eng.cam.z);
            let d = tx * fx + tz * fz;
            if d <= 0.5 {
                continue;
            }
            if (55.0..90.0).contains(&d) {
                far_traffic += 1;
            }
            // Roughly centred, so the shot is of a car rather than of one
            // clipped against the frame edge, and wholly on screen — a car
            // right under the camera's nose is enormous and entirely off the
            // bottom of the frame, which is not a picture of anything.
            let top = eng.proj.row_of(1.05, d).ceil();
            let bot = eng.proj.row_of(0.0, d).floor();
            if ((tx * rx + tz * rz) / d).abs() < 0.45
                && top >= 0.0
                && bot < eng.grid.rows as f32
            {
                nearest = nearest.min(d);
                tallest = tallest.max(bot - top);
            }
        }

        // How much of the middle half of the frame is looking down a street
        // rather than at a facade.
        let (lo, hi) = (cols / 4, cols - cols / 4);
        let open = (lo..hi)
            .filter(|&x| eng.rays.column(x).first().map(|h| h.dist).unwrap_or(200.0) > 45.0)
            .count();

        let scores = [
            // near: the longest registration the walk ever gets on screen as
            // real characters, and then the closest car it gets it on — the
            // panel stops growing once it reaches plate proportion, but the CAR
            // keeps growing, and a plate on a car you can see is the thing
            // being judged.
            // Aim at a car about nine rows tall: that is the distance at which
            // the sprite's own half-width cap stops squashing it, so the car
            // is as big as it gets while still being the shape of a car.
            if carried > 0 {
                carried as f32 * 1000.0 + (100.0 - (tallest - 9.0).abs())
            } else {
                0.0
            },
            // middle: plate-shaped panels, and nothing on the frame big
            // enough to be read as text.
            if widest < readable_at { smudges as f32 } else { 0.0 },
            // far: traffic in shot, no plate anywhere on it, and a street open
            // enough to see down — traffic past 55 units behind a wall proves
            // nothing about plates.
            if widest == 0 { far_traffic as f32 * open as f32 } else { 0.0 },
        ];
        let caps = [
            format!(
                "near — a {carried}-character registration on a {widest}-cell plate; \
biggest car wholly in shot {tallest:.0} rows tall at {nearest:.1} units"
            ),
            format!(
                "middle — {smudges} empty plates, widest {widest} cells \
(a registration needs {readable_at})"
            ),
            format!(
                "far — {far_traffic} vehicles past 55 units down an open street \
({open} of {} middle columns clear), no plate drawn",
                hi - lo
            ),
        ];
        for k in 0..3 {
            if scores[k] > shots[k].0 {
                let title = format!(
                    "AsciiWorldEngine — plates, {} — {cols}x{rows} step {i}",
                    ["near", "middle", "far"][k]
                );
                shots[k] = (
                    scores[k],
                    grid_to_text(&eng.grid),
                    grid_to_svg(&eng.grid, &title),
                    caps[k].clone(),
                );
            }
        }
    }

    let _ = std::fs::create_dir_all(&a.out);
    // A registration is SET across its panel — the characters spread to span
    // the plate rather than clustering in the middle of it — so "verbatim" now
    // means any of the three even pitches it can be set at, not just the tight
    // one. `RT08 AAR` on a near car reads `R T 0 8   A A R`, and that is the
    // registration, not something else.
    //
    // Checked against what is actually ON THE ROAD — `eng.pop.plates`, not
    // `a.plates` — so the check runs whether the list came from `--plates`,
    // from a file, or (the common case) from nobody having passed either.
    let wanted: Vec<(String, Vec<String>)> = eng
        .pop
        .plates
        .all()
        .iter()
        .map(|q| {
            let mut buf = [b' '; asciicity::palette::PLATE_SET_MAX];
            let forms = (0..asciicity::palette::PLATE_SETTINGS)
                .map(|s| {
                    let n = q.set_into(s, &mut buf);
                    String::from_utf8_lossy(&buf[..n]).into_owned()
                })
                .collect();
            (q.as_str().to_string(), forms)
        })
        .collect();
    for (k, name) in ["near", "middle", "far"].iter().enumerate() {
        if shots[k].0 <= 0.0 && k != 2 {
            eprintln!("{name}: nothing scored — is there traffic on this seed?");
            continue;
        }
        let stem = format!("{}/{}-{}", a.out, a.name, name);
        let _ = std::fs::write(format!("{stem}.txt"), &shots[k].1);
        let _ = std::fs::write(format!("{stem}.svg"), &shots[k].2);
        // The honest legibility check: the frame's own characters are the
        // characters a reader sees, so a registration that appears verbatim in
        // the text dump is a registration you can read off the picture.
        let found: Vec<&String> = wanted
            .iter()
            .filter(|(_, forms)| forms.iter().any(|f| shots[k].1.contains(f.as_str())))
            .map(|(name, _)| name)
            .collect();
        eprintln!("wrote {stem}.svg / .txt — {}", shots[k].3);
        if !found.is_empty() {
            eprintln!("  readable registrations on this frame: {found:?}");
        } else if k == 0 && !wanted.is_empty() {
            eprintln!("  no supplied registration appears verbatim in the frame text");
        }
    }
}

// --- measurement ---------------------------------------------------------
/// Honest per-frame cost. Simulation, raycast, render and paint measured
/// separately over a real moving camera, not a still frame — a still frame
/// flatters the cull.
fn bench(a: &Args) {
    let cols = a.cols.unwrap_or(180);
    let rows = a.rows.unwrap_or(60);
    let mut eng = make(a, cols, rows);
    if let Some(e) = a.eye {
        eng.cam.eye = e;
        eng.cam.eye_target = e;
        eng.cam.pitch = a.pitch;
    }
    // Indoors is a different frame with a different cost and both are worth
    // having. Walk in through a real door with the real keys — there is no
    // back way into a room, and a bench that got there by one would not be
    // measuring the engine anybody plays.
    if a.indoors {
        let Some((dx, dz, face)) = eng.world.door_near(eng.cam.x, eng.cam.z, 160) else {
            eprintln!("no entrance within 160 cells of the spawn for seed {}", a.seed);
            std::process::exit(1);
        };
        let (ix, iz) = asciicity::interior::INWARD[face as usize];
        eng.cam.x = (dx - ix) as f32 + 0.5;
        eng.cam.z = (dz - iz) as f32 + 0.5;
        eng.cam.yaw = (ix as f32).atan2(-(iz as f32));
        eng.cam.halt();
        for _ in 0..240 {
            eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
            if eng.world.indoors() {
                break;
            }
        }
        if !eng.world.indoors() {
            eprintln!("could not get inside to bench");
            std::process::exit(1);
        }
        for _ in 0..60 {
            eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
        }
    }
    let mut ansi = String::new();
    let n = a.frames;
    let mut sim = Vec::with_capacity(n);
    let mut cast = Vec::with_capacity(n);
    let mut rend = Vec::with_capacity(n);
    let mut paint = Vec::with_capacity(n);
    let mut hits = 0usize;
    let mut inside = 0usize;
    // Warm up: let the population settle and the camera get out of the spawn.
    for _ in 0..60 {
        eng.step(1.0 / 60.0, key::FWD, 0.0, 0.0);
        eng.render();
    }
    let wall = Instant::now();
    for i in 0..n {
        // Keep it moving and turning, so no frame gets a free ride.
        let keys = key::FWD | if (i / 40) % 2 == 0 { key::TURN_R } else { key::SPRINT };
        eng.step(1.0 / 60.0, keys, 0.0, 0.0);
        eng.render();
        let t = Instant::now();
        grid_to_ansi(&eng.grid, &mut ansi);
        paint.push(t.elapsed().as_secs_f32() * 1000.0);
        sim.push(eng.stats.sim_us / 1000.0);
        cast.push(eng.stats.cast_us / 1000.0);
        rend.push(eng.stats.render_us / 1000.0);
        hits += eng.hit_count();
        if eng.world.indoors() {
            inside += 1;
        }
    }
    let wall_ms = wall.elapsed().as_secs_f32() * 1000.0;
    println!(
        "asciicity — AsciiWorldEngine, {cols}x{rows} = {} cells, {n} frames, eye {:.1}\n  {}\n",
        cols * rows, eng.cam.eye, plate_note(a, &eng)
    );
    println!("  stage      mean      p50      p95");
    for (label, v) in [("sim", &sim), ("cast", &cast), ("render", &rend), ("paint", &paint)] {
        let mut s = v.clone();
        s.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let mean: f32 = v.iter().sum::<f32>() / v.len() as f32;
        println!("  {label:<8} {mean:>7.3}  {:>7.3}  {:>7.3}", s[s.len() / 2], s[s.len() * 95 / 100]);
    }
    let eng_mean: f32 = sim.iter().chain(&cast).chain(&rend).sum::<f32>() / n as f32;
    let paint_mean: f32 = paint.iter().sum::<f32>() / n as f32;
    println!(
        "\n  engine (sim+cast+render) {:.3} ms/frame\n  + ANSI paint             {:.3} ms/frame\n  \
         = {:.3} ms/frame total, {:.0} fps ceiling\n  {:.0} visible cells kept per frame by the occlusion cull",
        eng_mean, paint_mean, eng_mean + paint_mean, 1000.0 / (eng_mean + paint_mean),
        hits as f32 / n as f32
    );
    println!("  wall clock over the run: {:.1} ms for {n} frames", wall_ms);
    if a.indoors || inside > 0 {
        println!(
            "  {inside} of {n} frames were INDOORS ({}%){}",
            100 * inside / n,
            match eng.room() {
                Some(r) => format!(" — {}, {}x{}, ceiling {:.1}", r.label_str(), r.wx, r.wz, r.ceiling),
                None => String::new(),
            }
        );
    }
}
