//! The native terminal frontend. This is the product: `cargo run` and you are
//! walking the city. No node, no browser, no wasm anywhere on this path.
//!
//! It does four things and no more — raw mode and key state, the terminal's
//! size, hand the engine an input bitmask and a `dt`, paint the buffer the
//! engine hands back.
//!
//! **Key holds.** A terminal normally delivers keypresses, not key-holds, which
//! is why terminal walkers feel like they stutter. Where the terminal speaks the
//! **kitty keyboard protocol** we ask for press/release events and get genuine
//! hold-to-walk; where it does not, a key counts as held for a short window
//! after its last autorepeat, which gets most of the way there. Which mode you
//! got is on the HUD, so the feel of it is never a mystery.

mod input;
mod raw;

pub use input::Keyboard;
pub use raw::{terminal_size, RawTerm};
