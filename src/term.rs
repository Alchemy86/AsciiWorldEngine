//! The city's keys, on the engine's terminal.
//!
//! Raw mode, the terminal's size and the key-hold state machine are the
//! engine's (`ascii_engine::term`). What the keys MEAN is the city's: which
//! ones are presses rather than holds, which pairs cancel, and how held keys
//! become the camera's input bitmask.

use crate::camera::key;
pub use ascii_engine::term::{
    terminal_size, KeyMap, Keyboard, RawTerm, K_DOWN, K_LEFT, K_PGDN, K_PGUP, K_RIGHT, K_SHIFT,
    K_UP,
};

/// Every key the game reads as a one-shot press rather than as movement.
/// Ctrl-C, Tab and Esc, then `l m n p q t v x` and Enter. Keep it in step with
/// the `kb.tapped(...)` calls in `bin/asciicity.rs` — nothing checks that they
/// agree, and a key missing from here is a key that does nothing.
const ONE_SHOT: [u32; 12] = [
    3, 9, 13, 27, b'l' as u32, b'm' as u32, b'n' as u32, b'p' as u32, b'q' as u32,
    b't' as u32, b'v' as u32, b'x' as u32,
];

/// Keys that cancel each other. Pressing one clears the other's latch, so a
/// latched walk forward never fights a deliberate step back.
const OPPOSED: [(u32, u32); 5] = [
    (b'w' as u32, b's' as u32),
    (b'a' as u32, b'd' as u32),
    (b'j' as u32, b'k' as u32),
    (b'r' as u32, b'f' as u32),
    (b'e' as u32, b'c' as u32),
];

/// The city's bindings, for `Keyboard::new`.
pub const KEYS: KeyMap = KeyMap { one_shot: &ONE_SHOT, opposed: &OPPOSED };

/// The engine's input bitmask off the keys held now. This is the ONLY thing
/// about input that crosses into the engine.
pub fn bits(kb: &Keyboard) -> u32 {
    let mut k = 0;
    if kb.down(b'w' as u32) || kb.down(K_UP) { k |= key::FWD; }
    if kb.down(b's' as u32) || kb.down(K_DOWN) { k |= key::BACK; }
    if kb.down(b'a' as u32) { k |= key::STRAFE_L; }
    if kb.down(b'd' as u32) { k |= key::STRAFE_R; }
    if kb.down(b'j' as u32) || kb.down(K_LEFT) { k |= key::TURN_L; }
    if kb.down(b'k' as u32) || kb.down(K_RIGHT) { k |= key::TURN_R; }
    if kb.down(b'r' as u32) { k |= key::LOOK_UP; }
    if kb.down(b'f' as u32) { k |= key::LOOK_DOWN; }
    if kb.down(b' ' as u32) || kb.down(K_SHIFT) { k |= key::SPRINT; }
    if kb.down(b'e' as u32) || kb.down(K_PGUP) { k |= key::RISE; }
    if kb.down(b'c' as u32) || kb.down(K_PGDN) { k |= key::SINK; }
    // Act on whatever is in reach. Edge-triggered inside the ENGINE, not
    // here, so every frontend — keyboard, film script, test — presses a
    // panel exactly once for one press.
    if kb.down(b'x' as u32) || kb.down(13) { k |= key::ACT; }
    k
}
