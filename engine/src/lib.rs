//! An engine for programs drawn in coloured ASCII.
//!
//! What any such program shares, and nothing about what it draws: a cell grid
//! with a depth buffer and an optional shaded background ([`Grid`]), glyph
//! ramps ordered by how much of a cell each character inks ([`ramp`]), colour
//! ([`color`]), an ordered dither ([`dither`]), a deterministic hash and RNG
//! ([`rng`]), a perspective projection for column renderers ([`project`]),
//! 24-bit ANSI, SVG and text output ([`output`]), frames to disk
//! ([`capture`]), a frame-loop clock ([`frame`]), and raw terminal mode with
//! key-hold input ([`term`], behind the `term` feature).
//!
//! The city in this repository is the first program built on it, and lives
//! outside it: nothing in here knows there is a city.
//!
//! ```
//! use ascii_engine::{ramp, color::hsl, Grid};
//! let mut g = Grid::new(20, 4);
//! for x in 0..20 {
//!     let b = x as f32 / 19.0;
//!     g.put(x, 1, ramp::glyph(ramp::CLASSIC, b), hsl(210.0, 80.0, 30.0 + 40.0 * b), 0.0);
//! }
//! let mut ansi = String::new();
//! ascii_engine::output::grid_to_ansi(&g, &mut ansi);
//! assert!(ansi.contains('@'));
//! ```

pub mod capture;
pub mod color;
pub mod dither;
pub mod frame;
pub mod grid;
pub mod output;
pub mod project;
pub mod ramp;
pub mod rng;

#[cfg(feature = "term")]
pub mod term;

pub use grid::Grid;
