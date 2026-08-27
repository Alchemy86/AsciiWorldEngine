//! The browser build — an optional extra target off the same core, not the
//! foundation. Build it with:
//!
//! ```text
//! ./build-wasm.sh
//! ```
//!
//! No wasm-bindgen: plain `extern "C"` exports and a view over linear memory.
//! The whole per-frame boundary is the one flat buffer `frame_ptr()` points at,
//! so the JS side does exactly four things — load the module, feed keys and a
//! `dt` in, paint the bytes that come back, and size to the window.

use crate::Engine;

static mut ENGINE: Option<Engine> = None;

#[inline]
fn eng() -> &'static mut Engine {
    // Single-threaded wasm; the engine lives for the page's lifetime.
    unsafe { (*core::ptr::addr_of_mut!(ENGINE)).as_mut().unwrap() }
}

/// Build the world. `cell_w`/`cell_h` are the display cell's pixel size.
#[no_mangle]
pub extern "C" fn ac_init(cols: u32, rows: u32, cell_w: f32, cell_h: f32, seed: u32) {
    let e = Engine::new(cols as usize, rows as usize, cell_w, cell_h, seed);
    unsafe { ENGINE = Some(e) }
}

#[no_mangle]
pub extern "C" fn ac_resize(cols: u32, rows: u32, cell_w: f32, cell_h: f32) {
    eng().resize(cols as usize, rows as usize, cell_w, cell_h);
}

/// Advance the world. Timed separately from `ac_render` so the page can report
/// simulation and render cost apart rather than quoting one number for both.
#[no_mangle]
pub extern "C" fn ac_step(dt: f32, keys: u32, look_x: f32, look_y: f32) {
    eng().step(dt, keys, look_x, look_y);
}

#[no_mangle]
pub extern "C" fn ac_render() {
    let e = eng();
    e.render();
    e.frame();
}

/// Pointer to the packed frame: 4 bytes per cell, `[glyph, r, g, b]`.
#[no_mangle]
pub extern "C" fn ac_frame_ptr() -> *const u8 {
    eng().frame().as_ptr()
}

#[no_mangle]
pub extern "C" fn ac_frame_len() -> u32 {
    (eng().proj.cols * eng().proj.rows * 4) as u32
}

/// The background plane: 3 bytes a cell, `[r, g, b]`, row-major, black
/// everywhere except a registration plate. `ac_has_panels` is zero on a frame
/// that painted none, and the page skips the whole plane when it is.
#[no_mangle]
pub extern "C" fn ac_bg_ptr() -> *const u8 {
    eng().grid.bg.as_ptr()
}

#[no_mangle]
pub extern "C" fn ac_has_panels() -> u32 {
    eng().grid.has_panels as u32
}

/// Turn registration plates on or off, as `--no-plates` does natively.
#[no_mangle]
pub extern "C" fn ac_set_plates_on(on: u32) {
    eng().set_plates_on(on != 0);
}

#[no_mangle]
pub extern "C" fn ac_toggle_vista() {
    eng().cam.toggle_vista();
}

#[no_mangle]
pub extern "C" fn ac_cam_x() -> f32 { eng().cam.x }
#[no_mangle]
pub extern "C" fn ac_cam_z() -> f32 { eng().cam.z }
#[no_mangle]
pub extern "C" fn ac_cam_yaw() -> f32 { eng().cam.yaw }
#[no_mangle]
pub extern "C" fn ac_cam_eye() -> f32 { eng().cam.eye }
#[no_mangle]
pub extern "C" fn ac_hits() -> u32 { eng().hit_count() as u32 }
