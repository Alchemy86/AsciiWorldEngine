//! A film script: what to do at the controls, and for how long.
//!
//! This is an INPUT source, not a camera. Every beat it produces is the same
//! `camera::key` bitmask a keyboard produces, and the two one-shot actions it
//! has — `vista` and `weather` — are the `V` and `T` keys, not an eye height
//! and not a flag. So a film goes through the camera, the collision, the
//! projection, the population and the weather exactly as a player's session
//! does, and nothing about the picture is reconstructed by the caller.
//!
//! The format is meant to be typed by hand and read at a glance:
//!
//! ```text
//! # one beat per line:  <duration> <action> [action ...]
//! 2s   wait
//! 7s   walk                 # 3.2 units/s — a walk, not a fast-forward
//! 4s   walk look-up
//! 1s   vista wait           # press V
//! 7s   wait                 # hold on the skyline
//! ```
//!
//! Durations are seconds by default (`4s`, `4.5`, `250ms`) or exact ticks
//! (`120f`). Held actions apply for the whole beat; `vista` and `weather`
//! fire once, at the start of it.

use crate::camera::key;
use crate::entities::Weather;

/// One line of a script: a duration, whatever is being held down for it, and
/// whatever is pressed once at the start of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Beat {
    /// How many engine ticks this beat lasts, and so how many frames it writes.
    pub ticks: usize,
    /// The key bitmask held for every tick of the beat.
    pub keys: u32,
    /// `level` — bring the horizon back to flat rather than leaving the pitch
    /// wherever the last `look-up` left it. It is a held action like the rest,
    /// but which key it needs depends on where the pitch currently is, so the
    /// runner adds the bit; see `level_bits`.
    pub level: bool,
    /// `vista` — press `V` once, at the start of the beat.
    pub vista: bool,
    /// `weather X` — set the weather once, at the start of the beat.
    pub weather: Option<Weather>,
    /// The source line, verbatim and without its comment. It goes in the
    /// frame's title, so a frame always says which beat it came from.
    pub label: String,
    /// 1-based source line number, for error messages.
    pub line: usize,
}

/// A parsed script. `beats` is in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub beats: Vec<Beat>,
}

impl Script {
    /// Total ticks, which is the number of frames the film will be.
    pub fn ticks(&self) -> usize {
        self.beats.iter().map(|b| b.ticks).sum()
    }
}

/// Which look key brings the horizon back toward level from `pitch`, if any.
/// Zero once it is level enough to stop, so a `level` beat settles instead of
/// hunting either side of flat.
pub fn level_bits(pitch: f32) -> u32 {
    if pitch > 0.03 {
        key::LOOK_DOWN
    } else if pitch < -0.03 {
        key::LOOK_UP
    } else {
        0
    }
}

/// Every held action, in the order they are listed to a reader. Kept as data
/// so the help text and the error message cannot drift from what parses.
pub const ACTIONS: &[(&str, u32)] = &[
    ("wait", 0),
    ("walk", key::FWD),
    ("back", key::BACK),
    ("strafe-left", key::STRAFE_L),
    ("strafe-right", key::STRAFE_R),
    ("turn-left", key::TURN_L),
    ("turn-right", key::TURN_R),
    ("sprint", key::SPRINT),
    ("look-up", key::LOOK_UP),
    ("look-down", key::LOOK_DOWN),
    ("rise", key::RISE),
    ("sink", key::SINK),
];

/// The one-shot actions and the two that take a value, for the same reason.
pub const ONE_SHOTS: &[&str] = &["level", "vista", "weather clear|rain|downpour"];

/// Parse a script. `fps` is the tick rate the durations are counted against —
/// a beat's seconds become that many ticks, which is that many frames.
///
/// Errors carry the line number and the offending word, because a script is
/// something a person is editing and a parser that only says "bad script" is
/// a parser you edit by guessing.
pub fn parse(text: &str, fps: f32) -> Result<Script, String> {
    // NaN included: a rate that is not a positive number cannot give a beat a
    // length, and the error is worth more than a script of one-frame beats.
    if fps.is_nan() || fps <= 0.0 {
        return Err(format!("fps must be above zero, got {fps}"));
    }
    let mut beats = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = raw.split('#').next().unwrap_or("").trim();
        if body.is_empty() {
            continue;
        }
        let mut words = body.split_whitespace();
        let first = words.next().unwrap_or_default();
        let ticks = duration_ticks(first, fps).ok_or_else(|| {
            format!(
                "line {line}: `{first}` is not a duration. A beat starts with one: \
`4s` seconds, `250ms`, or `120f` exact frames."
            )
        })?;
        let mut beat = Beat {
            ticks,
            keys: 0,
            level: false,
            vista: false,
            weather: None,
            label: body.to_string(),
            line,
        };
        while let Some(w) = words.next() {
            let name = w.to_ascii_lowercase().replace('_', "-");
            if name == "weather" {
                let v = words.next().ok_or_else(|| {
                    format!("line {line}: `weather` wants clear, rain or downpour after it")
                })?;
                beat.weather = Some(match v.to_ascii_lowercase().as_str() {
                    "clear" => Weather::Clear,
                    "rain" => Weather::Rain,
                    "downpour" | "storm" => Weather::Downpour,
                    other => {
                        return Err(format!(
                            "line {line}: `{other}` is not a weather. \
Try clear, rain or downpour."
                        ))
                    }
                });
                continue;
            }
            if name == "vista" {
                beat.vista = true;
                continue;
            }
            if name == "level" {
                beat.level = true;
                continue;
            }
            match ACTIONS.iter().find(|(n, _)| *n == name) {
                Some((_, bits)) => beat.keys |= bits,
                None => {
                    return Err(format!(
                        "line {line}: `{w}` is not an action. Try one of:\n  {}\n  {}",
                        ACTIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join("  "),
                        ONE_SHOTS.join("  ")
                    ))
                }
            }
        }
        beats.push(beat);
    }
    if beats.is_empty() {
        return Err("the script has no beats in it".into());
    }
    Ok(Script { beats })
}

/// `4s` / `4` / `4.5s` / `250ms` / `120f` -> a tick count.
///
/// `f` is an EXACT frame count and is deliberately the one unit the frame rate
/// does not scale: it is how a beat gets pinned to a known number of frames
/// when a film is being cut against something else. Everything else is time,
/// so re-shooting at another `--fps` gives the same film at another cadence.
/// A beat is never shorter than one frame — a script line that wrote no frame
/// at all would be a line that silently did nothing.
fn duration_ticks(w: &str, fps: f32) -> Option<usize> {
    let (num, per_second) = if let Some(n) = w.strip_suffix("ms") {
        (n, fps * 0.001)
    } else if let Some(n) = w.strip_suffix('s') {
        (n, fps)
    } else if let Some(n) = w.strip_suffix('f') {
        (n, 1.0)
    } else {
        (w, fps)
    };
    let v: f32 = num.parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    Some(((v * per_second).round() as i64).max(1) as usize)
}

/// The script `--film` runs when it is not given one, and what `--print-script`
/// prints so it can be redirected to a file and edited. It is the shot the
/// recorder exists for: a walk down a street at a walking pace, looking up at
/// the towers on the way, then `V` and a hold on the skyline. The weather is
/// left to `--weather` so the same reel can be shot clear or in the rain.
pub const DEFAULT_SCRIPT: &str = "\
# AsciiWorldEngine — a film script.
#
# One beat per line:   <duration> <action> [action ...]
#
#   duration    4s seconds (the default unit) · 250ms · 120f exact frames
#   actions     held down for the whole beat, and they combine freely:
#                 walk  back  sprint  wait
#                 strafe-left  strafe-right  turn-left  turn-right
#                 look-up  look-down  level  rise  sink
#   vista       press V — street <-> the elevated skyline. Once, at the
#               start of its beat.
#   weather clear|rain|downpour — also once, at the start of its beat.
#
# `walk` is 3.2 units/s and `sprint` is 6.5, the same as at the keyboard, and
# one frame is written per tick — so played back at --fps the film runs at the
# speed it was walked. Blank lines and #-comments are ignored.
#
# The weather comes from --weather, so this same reel shoots clear or wet.
# Rain is a STREET-level thing: the drops fall from 12 units and the vista deck
# is at 34, so the skyline hold at the end is above the weather and comes out
# dry. That is the engine, not the recorder — see AGENTS.md.

2s   wait                 # stand still a moment and let the street fill
7s   walk                 # down the avenue at a walking pace
4s   walk look-up         # up at the towers as we pass them
3s   walk level           # horizon back down
6s   walk                 # keep going, past the traffic
1s   vista wait           # press V
9s   wait                 # hold on the skyline
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_beat_is_a_duration_and_the_keys_held_for_it() {
        let s = parse("2s walk sprint", 30.0).unwrap();
        assert_eq!(s.beats.len(), 1);
        assert_eq!(s.beats[0].ticks, 60);
        assert_eq!(s.beats[0].keys, key::FWD | key::SPRINT);
    }

    #[test]
    fn durations_come_in_seconds_milliseconds_and_exact_frames() {
        let s = parse("1s wait\n500ms wait\n7f wait\n2 wait", 30.0).unwrap();
        let ticks: Vec<usize> = s.beats.iter().map(|b| b.ticks).collect();
        // `f` is an exact frame count and is NOT scaled by the frame rate.
        assert_eq!(ticks, vec![30, 15, 7, 60]);
    }

    #[test]
    fn comments_and_blank_lines_are_not_beats() {
        let s = parse("# a note\n\n  \n3s walk  # and another\n", 30.0).unwrap();
        assert_eq!(s.beats.len(), 1);
        // The label is the line without its comment — it goes in the title.
        assert_eq!(s.beats[0].label, "3s walk");
    }

    #[test]
    fn vista_and_weather_are_presses_not_state() {
        let s = parse("1s vista weather rain", 30.0).unwrap();
        assert!(s.beats[0].vista);
        assert_eq!(s.beats[0].weather, Some(Weather::Rain));
        // Neither of them holds a key down.
        assert_eq!(s.beats[0].keys, 0);
    }

    #[test]
    fn underscores_and_capitals_parse_the_same_as_hyphens() {
        let a = parse("1s LOOK_UP", 30.0).unwrap();
        let b = parse("1s look-up", 30.0).unwrap();
        assert_eq!(a.beats[0].keys, b.beats[0].keys);
        assert_eq!(a.beats[0].keys, key::LOOK_UP);
    }

    #[test]
    fn a_bad_word_says_which_line_and_what_is_allowed() {
        let e = parse("2s walk\n3s jump", 30.0).unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        assert!(e.contains("jump"), "{e}");
        assert!(e.contains("strafe-left"), "{e}");
        let e = parse("walk 2s", 30.0).unwrap_err();
        assert!(e.contains("line 1") && e.contains("duration"), "{e}");
        assert!(parse("# nothing but a comment", 30.0).is_err());
    }

    #[test]
    fn level_asks_for_the_key_that_flattens_the_horizon_and_then_stops() {
        assert_eq!(level_bits(0.4), key::LOOK_DOWN);
        assert_eq!(level_bits(-0.4), key::LOOK_UP);
        assert_eq!(level_bits(0.0), 0);
        assert_eq!(level_bits(0.02), 0);
    }

    #[test]
    fn the_built_in_script_parses_and_is_the_shot_it_claims_to_be() {
        let s = parse(DEFAULT_SCRIPT, 30.0).unwrap();
        assert!(s.ticks() > 30 * 25, "the reel should be over 25 seconds long");
        assert!(s.beats.iter().any(|b| b.vista), "it presses V");
        assert!(s.beats.iter().any(|b| b.keys & key::FWD != 0 && b.keys & key::LOOK_UP != 0),
            "it looks up while walking");
        // Nothing in the reel sprints: the note is `just walk normally`.
        assert!(s.beats.iter().all(|b| b.keys & key::SPRINT == 0));
    }
}
