//! Code patched into the Portacapture X8's firmware (V2.03 build 217).
//!
//! The patch is linked to run from SDRAM at 0x81830000, just after the stock
//! application (see `x8patch.ld`). `tools/x8patch.py` stores it in the update
//! file after the stock image and points the stock firmware at the functions
//! named in the patch's header. Nothing here runs unless the stock code calls
//! it. When the recorder is not in a normal start (for example in firmware
//! update mode), every entry point calls the stock code it replaced, except
//! `slot_hook` and `free_slot_hook`, which replace whole stock functions and
//! give the stock result themselves.
//!
//! The bat app: the BAT DETECTOR and BAT PLAYBACK tiles on the LAUNCHER
//! (USER SETTINGS 2 and 3) open MANUAL with the bat screen over it, fed from
//! the inputs turned on in MANUAL or from the X8's playback. See
//! docs/HOMEBREW.md, steps 9 to 18.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod patch;

#[cfg(not(target_os = "none"))]
fn main() {}
