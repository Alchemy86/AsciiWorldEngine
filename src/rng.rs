//! Deterministic hashing and a tiny RNG. No dependencies, no global state.
//!
//! Two *different* kinds of randomness are used by the renderer and they are
//! not interchangeable (see ARCHITECTURE.md, "Two kinds of noise"):
//!
//!   * `noise()` — a hash of integer lattice coordinates. Per-cell brightness
//!     dither, star fields, sign placement.
//!   * the ordered Bayer dither in `palette::surf_tex` — surface texture.
//!     Substituting hash noise there turns every facade into speckle.

/// xorshift64*, for one-shot world generation.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng { state: (seed ^ 0x9E37_79B9_7F4A_7C15) | 1 }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in `[0, n)`.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next_u32() % n }
    }

    /// Uniform f32 in `[0, 1)`.
    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Value noise on an integer lattice, in `[0, 1)`.
///
/// A stable integer hash: the same lattice point always gives the same value,
/// on every platform and in every build, so a seed is a promise rather than a
/// hint and two runs of the same seed speckle identically.
#[inline]
pub fn noise(a: i32, b: i32) -> f32 {
    let mut h = (a as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((b as u32).wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) as f32) / 4_294_967_296.0
}

/// Three-argument integer hash (FNV-ish), for world generation decisions.
#[inline]
pub fn hash3(a: i32, b: i32, c: i32) -> u32 {
    let mut h = 2_166_136_261u32;
    for v in [a as u32, b as u32, c as u32] {
        h ^= v;
        h = h.wrapping_mul(16_777_619);
        h ^= h >> 13;
    }
    h
}

/// `hash3` folded into `[0, 1)`.
#[inline]
pub fn hash3f(a: i32, b: i32, c: i32) -> f32 {
    (hash3(a, b, c) >> 8) as f32 / (1u32 << 24) as f32
}
