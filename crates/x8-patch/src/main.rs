//! Code patched into the Portacapture X8's firmware (V2.03 build 217).
//!
//! The patch is linked to run from SDRAM at 0x81830000, just after the stock
//! application (see `x8patch.ld`). `tools/x8patch.py` stores it in the update
//! file after the stock image and points the stock firmware at the functions
//! named in [`X8_PATCH_HEADER`]. Nothing here runs unless the stock code calls
//! it, and every entry point does nothing but call the stock code when the
//! recorder is not in a normal start (for example in firmware update mode).
//!
//! First test: the app screens' key handler. Every button press or wheel turn
//! in a recording app draws a coloured bar across both framebuffers, a new
//! colour each time, and then runs the stock handler as before.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod patch;

#[cfg(not(target_os = "none"))]
fn main() {}
