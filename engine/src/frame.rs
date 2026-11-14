//! The frame loop's clock: how long the last frame took, a frame-rate
//! reading, and a sleep that holds the loop to a budget.
//!
//! ```no_run
//! use ascii_engine::frame::Pacer;
//! use std::time::Duration;
//! let mut pacer = Pacer::new(Duration::from_millis(16), 0.1);
//! loop {
//!     let dt = pacer.begin();
//!     // step the world by dt, draw, paint
//!     pacer.count_frame();
//!     pacer.rest();
//! #   let _ = dt; break;
//! }
//! ```

use std::time::{Duration, Instant};

/// Paces a frame loop. See the module docs.
pub struct Pacer {
    budget: Duration,
    max_dt: f32,
    last: Instant,
    frame_start: Instant,
    window: Instant,
    frames: u32,
    fps: f32,
}

impl Pacer {
    /// `budget` is the shortest a frame may take (16 ms is about 60 a second);
    /// `max_dt` caps the step handed to a simulation, in seconds, so a stall
    /// (a resize, a suspended terminal) is one long step and not a teleport.
    pub fn new(budget: Duration, max_dt: f32) -> Self {
        let now = Instant::now();
        Pacer { budget, max_dt, last: now, frame_start: now, window: now, frames: 0, fps: 0.0 }
    }

    /// Start a frame. Returns the seconds since the last one started, capped
    /// at `max_dt`.
    pub fn begin(&mut self) -> f32 {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(self.max_dt);
        self.last = now;
        self.frame_start = now;
        dt
    }

    /// Count a finished frame. Returns true when the frame-rate reading was
    /// just refreshed, which happens every half second.
    pub fn count_frame(&mut self) -> bool {
        self.frames += 1;
        if self.window.elapsed() > Duration::from_millis(500) {
            self.fps = self.frames as f32 / self.window.elapsed().as_secs_f32();
            self.window = Instant::now();
            self.frames = 0;
            true
        } else {
            false
        }
    }

    /// Frames a second, as of the last refresh.
    pub fn fps(&self) -> f32 {
        self.fps
    }

    /// Sleep out whatever is left of the budget since `begin`.
    pub fn rest(&self) {
        let spent = self.frame_start.elapsed();
        if spent < self.budget {
            std::thread::sleep(self.budget - spent);
        }
    }
}
