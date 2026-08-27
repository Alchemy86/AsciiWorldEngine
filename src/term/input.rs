//! Key state from a terminal, in the two modes a terminal can offer it.
//!
//! With the **kitty keyboard protocol** we get press *and* release, so a key is
//! held exactly as long as your finger is on it, any number of keys at once,
//! and there is nothing to solve.
//!
//! Without it — which is what you get inside tmux with `extended-keys off`, and
//! in plenty of terminals besides — all a terminal ever sends is a stream of
//! *presses* padded out by the OS autorepeat. Two things follow, and they are
//! the whole reason this file is more than a byte loop:
//!
//!   1. **There is a hole at the start of every hold.** The first press
//!      arrives, then nothing until autorepeat starts, which on a typical Linux
//!      desktop is 250–660 ms later. A key that counts as held for a fixed
//!      window after its last press therefore dies inside that hole and comes
//!      back when the repeats begin. Measured on this machine against the real
//!      binary: a 280 ms freeze, mid-stride, every single time you start
//!      walking.
//!   2. **Only one key repeats.** Press a second key while holding the first
//!      and the OS moves the repeat to the new one; the first goes silent even
//!      though your finger has not moved. So walking stops the moment you start
//!      turning, and no amount of tuning a hold window changes that — the
//!      bytes simply are not there.
//!
//! Lengthening the window is not the fix. It cannot be: to cover the hole it
//! would have to outlast the autorepeat delay, and then letting go of a key
//! would leave you walking for two thirds of a second. So the hole and the
//! second key are solved separately, and neither is solved here alone:
//!
//!   * **The hole** is bridged by the camera, not by the parser —
//!     `Camera::glide` keeps the last commanded velocity alive and then slides
//!     off it, so the gap reads as a walk carrying its own weight rather than a
//!     stop. The window here only has to outlast the autorepeat *period*.
//!   * **The second key** is solved by *latching*. Three or more presses whose
//!     gaps are all shorter than a human can tap is proof of autorepeat, which
//!     is proof that a finger is physically down. A key that has proved itself
//!     that way stays down while you are demonstrably still playing — while
//!     any key is arriving — instead of dying the moment the repeat moves
//!     elsewhere. That is what makes walking and looking at the same time
//!     possible at all on this path.
//!
//! Neither trick is needed with the kitty protocol, and neither is applied
//! there.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::camera::key;

/// How long a key counts as held after a press once its repeats have started.
/// A repeat is due within 25–50 ms on any system you will meet, so this only
/// has to cover a hiccup. Keeping it short is what makes letting go of a key
/// feel like letting go of a key.
const HOLD: Duration = Duration::from_millis(150);

/// How long a key counts as held after its FIRST press, before any repeat has
/// arrived — as a multiple of the measured autorepeat delay, plus a margin.
///
/// This is the whole trick for the hole at the start of a hold: the silence
/// after the first press is not evidence the key is up, it is *expected*, and
/// its width is known. So rather than let the key lapse and lean on the
/// camera's glide to disguise it, simply hold the key through the silence it
/// was always going to make. The cost is that a single deliberate tap walks you
/// a step rather than a twitch, which is the right trade in a game you play by
/// holding keys.
const FIRST_HOLD_SCALE: f32 = 1.25;
const FIRST_HOLD_MARGIN: Duration = Duration::from_millis(30);
/// Used until the player's own keyboard has been measured. Linux desktops ship
/// 250–660 ms; this machine reports 500.
const DELAY_GUESS: Duration = Duration::from_millis(520);

/// The longest gap between two presses that can still be autorepeat rather than
/// Every key the game reads as a one-shot press rather than as movement.
/// Ctrl-C, Tab and Esc, then `l m n p q t v x` and Enter. Keep it in step with
/// the `kb.tapped(...)` calls in `bin/asciicity.rs` — nothing checks that they
/// agree, and a key missing from here is a key that does nothing.
const ONE_SHOT: [u32; 12] = [
    3, 9, 13, 27, b'l' as u32, b'm' as u32, b'n' as u32, b'p' as u32, b'q' as u32,
    b't' as u32, b'v' as u32, b'x' as u32,
];

/// a fast human tap. Human double-taps do not get under ~110 ms; autorepeat
/// does not get over ~55 ms.
const REPEAT_GAP: Duration = Duration::from_millis(90);
/// Presses at that cadence needed before we will believe a finger is down.
const REPEAT_PROOF: u32 = 3;
/// Once a key has proved itself AND another key has been pressed on top of it,
/// it stays down this long — bounded, because the finger really might have come
/// off during the other key's repeat and there is no way to know. Long enough
/// to look up at a tower (0.8 s) or turn a corner (1.5 s) without stopping.
/// `Tab` is the unbounded answer for anyone who wants one.
const LATCH_MAX: Duration = Duration::from_millis(2200);
/// Plausible bounds on an OS autorepeat delay. Outside these it was a human.
const DELAY_MIN: Duration = Duration::from_millis(120);
const DELAY_MAX: Duration = Duration::from_millis(900);

/// Silence that ends a latched key. It has to outlast the *autorepeat delay*,
/// because when a second key takes the repeat the keyboard goes quiet for that
/// long before the new key's own repeats start.
const LATCH_QUIET: Duration = Duration::from_millis(800);

/// Seconds the last input keeps commanding the camera after it stops arriving,
/// on the fallback path. Short — its job is weight, not bridging a gap.
const GLIDE: f32 = 0.18;

pub const K_UP: u32 = 1001;
pub const K_DOWN: u32 = 1002;
pub const K_RIGHT: u32 = 1003;
pub const K_LEFT: u32 = 1004;
pub const K_PGUP: u32 = 1005;
pub const K_PGDN: u32 = 1006;
/// Stands in for "a shift key" when the terminal names one.
pub const K_SHIFT: u32 = b'#' as u32;

/// What we know about one key that is, or recently was, down.
#[derive(Clone, Copy)]
struct Held {
    last: Instant,
    /// Consecutive presses arriving at autorepeat cadence.
    streak: u32,
    /// Proved to be a physically held key, so it survives losing the repeat.
    latched: bool,
    /// Another key has been pressed since this one last spoke — which is the
    /// only evidence there is that this one lost its repeat rather than being
    /// released.
    buried: bool,
}

/// Keys that cancel each other. Pressing one clears the other's latch, so a
/// latched walk forward never fights a deliberate step back.
const OPPOSED: [(u32, u32); 5] = [
    (b'w' as u32, b's' as u32),
    (b'a' as u32, b'd' as u32),
    (b'j' as u32, b'k' as u32),
    (b'r' as u32, b'f' as u32),
    (b'e' as u32, b'c' as u32),
];

pub struct Keyboard {
    pub kitty: bool,
    held: HashMap<u32, Held>,
    pending: Vec<u8>,
    /// When anything at all last arrived. The latch leans on this.
    last_event: Option<Instant>,
    /// Did this poll see any key at all? Distinct from `taps` and `bits`,
    /// which only speak for keys the game binds — the autopilot hands over on
    /// *any* key, including ones that do nothing.
    fresh: bool,
    /// The OS autorepeat delay, once we have watched one hold and seen it.
    repeat_delay: Option<Duration>,
    /// The gap that is *claimed* to be the delay, waiting on a third press to
    /// confirm the key was being held rather than tapped twice.
    candidate: Option<Duration>,
    /// One-shot presses the application consumes itself (quit, vista, capture).
    pub taps: Vec<u32>,
}

impl Keyboard {
    pub fn new(kitty: bool) -> Self {
        Keyboard {
            kitty,
            held: HashMap::new(),
            pending: Vec::new(),
            last_event: None,
            fresh: false,
            repeat_delay: None,
            candidate: None,
            taps: Vec::new(),
        }
    }

    /// Drain whatever the terminal has for us. Never blocks.
    pub fn poll(&mut self) {
        self.taps.clear();
        self.fresh = false;
        let mut buf = [0u8; 512];
        loop {
            let n = unsafe {
                let mut p =
                    libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
                if libc::poll(&mut p, 1, 0) <= 0 {
                    break;
                }
                libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n <= 0 {
                break;
            }
            self.pending.extend_from_slice(&buf[..n as usize]);
        }
        self.parse();
        if !self.kitty {
            self.expire(Instant::now());
        }
    }

    /// Decide, with no releases to go on, which keys are still down.
    ///
    /// The hard case is telling "I let go of W" apart from "I am still holding
    /// W and have now also pressed K, so the OS moved the repeat to K and W has
    /// gone silent". Both look like a key that stopped repeating. The one
    /// signal that separates them arrives immediately and is decisive: in the
    /// second case **another key was pressed**, and in the first case nothing
    /// was. So a key that has proved itself held survives losing its repeat
    /// only when something else was pressed on top of it; on its own it dies on
    /// the ordinary short window, which is what keeps letting go feeling like
    /// letting go.
    fn expire(&mut self, now: Instant) {
        let Some(last_event) = self.last_event else { return };
        // Has the keyboard gone quiet for longer than a second key's autorepeat
        // delay? Then nobody is holding anything.
        let quiet = now.duration_since(last_event) >= LATCH_QUIET;
        let mut expired: Vec<u32> = Vec::new();
        for (&code, h) in self.held.iter() {
            let age = now.duration_since(h.last);
            if age < self.window(h) {
                continue;
            }
            if h.latched && h.buried && !quiet && age < LATCH_MAX {
                continue;
            }
            expired.push(code);
        }
        for code in expired {
            self.held.remove(&code);
        }
    }

    fn parse(&mut self) {
        let mut i = 0usize;
        let b = std::mem::take(&mut self.pending);
        while i < b.len() {
            if b[i] == 0x1b {
                // Need at least the introducer to decide; if the sequence is
                // still arriving, keep it for the next poll.
                if i + 1 >= b.len() {
                    break;
                }
                if b[i + 1] == b'[' || b[i + 1] == b'O' {
                    let mut j = i + 2;
                    while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                        j += 1;
                    }
                    if j >= b.len() {
                        break; // incomplete
                    }
                    let params: String = b[i + 2..j].iter().map(|&c| c as char).collect();
                    self.csi(&params, b[j]);
                    i = j + 1;
                    continue;
                }
                // A bare ESC. Treat as quit so there is always a way out.
                self.taps.push(27);
                i += 1;
                continue;
            }
            let c = b[i];
            i += 1;
            if c == 3 {
                self.taps.push(3); // ctrl-C
                continue;
            }
            // In the non-kitty path a printable byte is a press with no
            // release. An uppercase letter is that key WITH shift, and shift
            // has to expire on its own clock like anything else — latching it
            // on for the rest of the run is a sprint you cannot turn off.
            if c.is_ascii_uppercase() {
                self.press(K_SHIFT);
            }
            self.press(c.to_ascii_lowercase() as u32);
        }
        // Anything left is a partial escape sequence; keep it.
        if i < b.len() {
            self.pending.extend_from_slice(&b[i..]);
        }
    }

    fn csi(&mut self, params: &str, final_byte: u8) {
        let mut parts = params.trim_start_matches('?').split(';');
        let p0 = parts.next().unwrap_or("");
        let p1 = parts.next().unwrap_or("");
        let mut p1i = p1.split(':');
        let mods: u32 = p1i.next().unwrap_or("").parse().unwrap_or(1);
        let event: u32 = p1i.next().unwrap_or("").parse().unwrap_or(1);

        let code = match final_byte {
            b'u' => p0.split(':').next().unwrap_or("").parse::<u32>().unwrap_or(0),
            b'A' => K_UP,
            b'B' => K_DOWN,
            b'C' => K_RIGHT,
            b'D' => K_LEFT,
            b'~' => match p0.parse::<u32>().unwrap_or(0) {
                5 => K_PGUP,
                6 => K_PGDN,
                _ => 0,
            },
            _ => 0,
        };
        if code == 0 {
            return;
        }
        // Kitty reports the shift keys themselves; they are the sprint key.
        let code = match code {
            57441 | 57447 => K_SHIFT,
            c => c,
        };
        match event {
            3 => {
                self.held.remove(&code);
                self.last_event = Some(Instant::now());
            }
            _ => {
                // A modifier reported alongside another key is a press of that
                // modifier too — the only way sprint-with-arrows can work.
                if mods.saturating_sub(1) & 1 != 0 {
                    self.press(K_SHIFT);
                } else if code != K_SHIFT {
                    self.held.remove(&K_SHIFT);
                }
                self.press(code);
            }
        }
    }

    fn press(&mut self, code: u32) {
        let lower = char::from_u32(code)
            .map(|c| c.to_ascii_lowercase() as u32)
            .unwrap_or(code);
        let now = Instant::now();
        self.last_event = Some(now);
        self.fresh = true;

        // Pressing a key deliberately cancels whatever it opposes: a latched
        // walk must never survive you asking for the other direction.
        for (a, b) in OPPOSED {
            if lower == a {
                self.held.remove(&b);
            } else if lower == b {
                self.held.remove(&a);
            }
        }

        // Anything already down has now had a key pressed on top of it.
        for (&code, h) in self.held.iter_mut() {
            if code != lower {
                h.buried = true;
            }
        }
        let entry = self.held.entry(lower).or_insert(Held {
            last: now,
            streak: 0,
            latched: false,
            buried: false,
        });
        entry.buried = false;
        // Autorepeat cadence, sustained, is proof of a finger on the key. A
        // human tapping fast cannot reach it, so this never fires by accident.
        let gap = now.duration_since(entry.last);
        // How far into a repeat stream this key is. A press is a repeat if it
        // came within one repeat *period* of the last one — or, for the second
        // press of a hold only, within one plausible autorepeat *delay*, since
        // that first long gap is exactly what a hold looks like.
        let first_repeat = entry.streak == 1 && (DELAY_MIN..=DELAY_MAX).contains(&gap);
        if entry.streak == 0 || gap <= REPEAT_GAP {
            entry.streak += 1;
        } else if first_repeat {
            entry.streak = 2;
            // That gap IS the OS autorepeat delay, which is the exact width of
            // the silence a hold makes before its repeats start. Measuring it
            // beats guessing it. Believed only once a third press at repeat
            // cadence rules out somebody tapping twice at a similar spacing.
            self.candidate = Some(gap);
        } else {
            entry.streak = 1;
            self.candidate = None;
        }
        if entry.streak >= REPEAT_PROOF {
            if let Some(c) = self.candidate.take() {
                self.repeat_delay = Some(c);
            }
        }
        entry.last = now;
        if !self.kitty && entry.streak >= REPEAT_PROOF {
            entry.latched = true;
        }

        // One-shot actions are consumed on the press edge, not while held.
        //
        // **Every key the frontend reads with `tapped` has to be in here**, and
        // this list is the whole reason a key can be wired up, documented in
        // `--help`, covered by a passing test and still do nothing at all when
        // somebody actually plays: `M` and the act key's own HUD note had both
        // been missing from it. A key that only moves the camera goes through
        // `bits` instead and does not belong here.
        if ONE_SHOT.contains(&lower) {
            self.taps.push(lower);
        }
    }

    #[inline]
    fn down(&self, code: u32) -> bool {
        self.held.contains_key(&code)
    }

    /// The engine's input bitmask. This is the ONLY thing about input that
    /// crosses into the engine.
    pub fn bits(&self) -> u32 {
        let mut k = 0;
        if self.down(b'w' as u32) || self.down(K_UP) { k |= key::FWD; }
        if self.down(b's' as u32) || self.down(K_DOWN) { k |= key::BACK; }
        if self.down(b'a' as u32) { k |= key::STRAFE_L; }
        if self.down(b'd' as u32) { k |= key::STRAFE_R; }
        if self.down(b'j' as u32) || self.down(K_LEFT) { k |= key::TURN_L; }
        if self.down(b'k' as u32) || self.down(K_RIGHT) { k |= key::TURN_R; }
        if self.down(b'r' as u32) { k |= key::LOOK_UP; }
        if self.down(b'f' as u32) { k |= key::LOOK_DOWN; }
        if self.down(b' ' as u32) || self.down(K_SHIFT) { k |= key::SPRINT; }
        if self.down(b'e' as u32) || self.down(K_PGUP) { k |= key::RISE; }
        if self.down(b'c' as u32) || self.down(K_PGDN) { k |= key::SINK; }
        // Act on whatever is in reach. Edge-triggered inside the ENGINE, not
        // here, so every frontend — keyboard, film script, test — presses a
        // panel exactly once for one press.
        if self.down(b'x' as u32) || self.down(13) { k |= key::ACT; }
        k
    }

    pub fn tapped(&self, code: u32) -> bool {
        self.taps.contains(&code)
    }

    /// Did this poll see a key — any key, bound or not? What the autopilot
    /// hands over on.
    pub fn any_key(&self) -> bool {
        self.fresh
    }

    /// **How long the last input keeps commanding the camera after it stops
    /// arriving**, in seconds.
    ///
    /// With the hole now *expected* rather than papered over (see `window`),
    /// this no longer has a gap to bridge and can be short: it is here for
    /// weight, so that a walk starts and ends as a movement rather than a
    /// teleport, and so the boundary between "held" and "not held" is a slope
    /// instead of a cliff. Zero with the kitty protocol, where key state is
    /// known rather than inferred and nothing needs softening.
    pub fn glide_needed(&self) -> f32 {
        if self.kitty {
            0.0
        } else {
            GLIDE
        }
    }

    /// The measured autorepeat delay, once one hold has been watched.
    pub fn repeat_delay(&self) -> Option<Duration> {
        self.repeat_delay
    }

    /// How long a key that has just spoken stays down, given how many times it
    /// has spoken. Everything about the fallback turns on this one function.
    fn window(&self, h: &Held) -> Duration {
        if h.streak >= 2 {
            // Its repeats are running, so silence means a finger came off.
            HOLD
        } else {
            // Its first and only press. The autorepeat delay's worth of silence
            // is coming and means nothing; ride it out.
            let d = self.repeat_delay.unwrap_or(DELAY_GUESS);
            Duration::from_secs_f32(d.as_secs_f32() * FIRST_HOLD_SCALE) + FIRST_HOLD_MARGIN
        }
    }

    /// True once a key has proved, by its own repeat cadence, that it is being
    /// physically held. The HUD uses it to show that the fallback has taken
    /// hold rather than leaving you guessing.
    pub fn latched(&self) -> bool {
        self.held.values().any(|h| h.latched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Seen {
        bits: u32,
        latched: bool,
    }

    /// Replay a stream of (offset-from-start, key) presses against the state
    /// machine and report what it believes at a given moment. No terminal
    /// involved — this is the model, tested as a model.
    fn replay(events: &[(u64, u32)], at_ms: u64) -> Seen {
        let base = Instant::now();
        let mut kb = Keyboard::new(false);
        // Instant cannot be built from a number, so drive the clock the only
        // honest way: sleep. The streams are short.
        let until = |ms: u64| {
            while base.elapsed() < Duration::from_millis(ms) {
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        // Only what has actually happened by the probe time — replaying the
        // whole stream first and then asking about an earlier moment tests
        // nothing at all.
        for &(t, code) in events.iter().filter(|(t, _)| *t <= at_ms) {
            until(t);
            kb.press(code);
        }
        until(at_ms);
        kb.expire(Instant::now());
        Seen { bits: kb.bits(), latched: kb.latched() }
    }

    /// The stream a physically held `key` produces with no releases to report:
    /// one press, the OS autorepeat delay, then repeats. These are this
    /// machine's own settings (`xset q`: 500 ms delay, 30 ms period).
    fn autorepeat(key: u32, from: u64, to: u64) -> Vec<(u64, u32)> {
        let mut v = vec![(from, key)];
        let mut t = from + 500;
        while t < to {
            v.push((t, key));
            t += 30;
        }
        v
    }

    #[test]
    fn a_hold_walks_straight_through_the_autorepeat_hole() {
        // 300 ms in, the first press is long past and the repeats have not
        // begun — the terminal is saying nothing at all. Walking must continue
        // anyway, because that silence was expected and its width is known.
        let e = autorepeat(b'w' as u32, 0, 1200);
        for probe in [120u64, 300, 460, 700, 1000] {
            assert_eq!(
                replay(&e, probe).bits & key::FWD,
                key::FWD,
                "walking stopped {probe} ms into a held key"
            );
        }
    }

    #[test]
    fn walking_survives_a_second_key_stealing_the_repeat() {
        // Hold W until its repeats prove it, then hold K too. The OS moves the
        // repeat to K and W goes silent with the finger still on it. This is
        // the exact thing that made walking and looking impossible.
        let mut e = autorepeat(b'w' as u32, 0, 800);
        e.extend(autorepeat(b'k' as u32, 800, 2200));
        let seen = replay(&e, 1600);
        assert_eq!(seen.bits & key::TURN_R, key::TURN_R, "the new key must be down");
        assert_eq!(
            seen.bits & key::FWD,
            key::FWD,
            "the held key must NOT die when it loses the repeat"
        );
    }

    #[test]
    fn letting_go_mid_stream_stops_promptly() {
        // Once a key's repeats are running, silence really does mean a finger
        // came off, and it must be believed quickly — this is the half of the
        // problem that a longer hold window would have made worse.
        let e = autorepeat(b'w' as u32, 0, 900);
        assert_eq!(replay(&e, 1000).bits & key::FWD, key::FWD, "still held at +70 ms");
        assert_eq!(replay(&e, 1120).bits & key::FWD, 0, "must be released by +190 ms");
    }

    #[test]
    fn letting_go_of_everything_stops_everything() {
        let mut e = autorepeat(b'w' as u32, 0, 800);
        e.extend(autorepeat(b'k' as u32, 800, 1400));
        let seen = replay(&e, 2400);
        assert_eq!(seen.bits, 0, "silence must end the walk and the turn alike");
    }

    #[test]
    fn a_latched_key_is_bounded_and_lets_go_on_its_own() {
        // W is held, K is held on top of it, and K goes on repeating for ever.
        // W cannot be believed for ever on that evidence — the finger really
        // might have come off — so it has to lapse.
        let mut e = autorepeat(b'w' as u32, 0, 700);
        e.extend(autorepeat(b'k' as u32, 700, 4200));
        assert_eq!(replay(&e, 1800).bits & key::FWD, key::FWD, "held at +1.1 s");
        assert_eq!(replay(&e, 3400).bits & key::FWD, 0, "must lapse by LATCH_MAX");
    }

    #[test]
    fn a_few_taps_never_latch() {
        // Human taps are far slower than autorepeat, so they must never be
        // mistaken for a finger held down. Each is a step and then it is over.
        let e = vec![(0, b'w' as u32), (200, b'w' as u32), (400, b'w' as u32)];
        assert!(!replay(&e, 700).latched, "tapping must not latch a walk on");
        assert_eq!(replay(&e, 1300).bits & key::FWD, 0, "the last tap must run out");
    }

    #[test]
    fn the_opposite_key_cancels_a_latched_one() {
        let mut e = autorepeat(b'w' as u32, 0, 800);
        e.extend(autorepeat(b's' as u32, 800, 1600));
        let seen = replay(&e, 1300);
        assert_eq!(seen.bits & key::FWD, 0, "pressing back must cancel a latched forward");
        assert_eq!(seen.bits & key::BACK, key::BACK);
    }

    #[test]
    fn shift_does_not_stick_on() {
        // One capital letter used to set sprint for the rest of the run.
        let e = vec![(0, K_SHIFT), (0, b'w' as u32)];
        assert_eq!(replay(&e, 1400).bits & key::SPRINT, 0, "sprint must not latch on for ever");
    }

    #[test]
    fn the_autorepeat_delay_is_measured_not_guessed() {
        let base = Instant::now();
        let mut kb = Keyboard::new(false);
        let until = |ms: u64| {
            while base.elapsed() < Duration::from_millis(ms) {
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        assert_eq!(kb.repeat_delay(), None, "nothing measured before a hold");
        for &(t, _) in autorepeat(b'w' as u32, 0, 600).iter() {
            until(t);
            kb.press(b'w' as u32);
        }
        let d = kb.repeat_delay().expect("one hold is enough to measure it");
        assert!(
            (Duration::from_millis(450)..Duration::from_millis(600)).contains(&d),
            "measured {d:?}, expected about 500 ms"
        );
    }
}

