//! Code patched into the Portacapture X8's firmware (V2.03 build 217).
//!
//! The patch is linked to run from SDRAM at 0x81830000, just after the stock
//! application (see `x8patch.ld`). `tools/x8patch.py` stores it in the update
//! file after the stock image and points the stock firmware at the functions
//! named in the patch's header. Nothing here runs unless the stock code calls
//! it, and every entry point does nothing but call the stock code when the
//! recorder is not in a normal start (for example in firmware update mode).
//!
//! Test 2: live heterodyne. The stock key handler, which task 1 calls for every
//! message while the launcher or an app is on screen, is replaced by
//! `key_hook`. On its first call it installs `audio_hook` as the audio DMA
//! callback. Pressing MARK three times within 1.5 s turns the heterodyne on or
//! off; while it is on, the wheel tunes it in 1 kHz steps (the app does not see
//! those turns), and at 192 kHz the headphones hear inputs 3 and 4 mixed and
//! heterodyned, at the stock headphone volume. A status line across the top of
//! the screen shows what the patch sees.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod patch;

#[cfg(not(target_os = "none"))]
fn main() {}
