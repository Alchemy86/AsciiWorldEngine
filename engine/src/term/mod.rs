//! The terminal: raw mode, its size, and key state.
//!
//! **Key holds.** A terminal normally delivers keypresses, not key-holds, which
//! is why terminal programs that move feel like they stutter. Where the
//! terminal speaks the **kitty keyboard protocol** we ask for press/release
//! events and get genuine hold-to-move; where it does not, `Keyboard` infers
//! holds from the autorepeat stream, which gets most of the way there. Which
//! mode you got is `RawTerm::kitty`, so a program can say so on screen.

mod input;
mod raw;

pub use input::{KeyMap, Keyboard, K_DOWN, K_LEFT, K_PGDN, K_PGUP, K_RIGHT, K_SHIFT, K_UP};
pub use raw::{restore, terminal_size, RawTerm};
