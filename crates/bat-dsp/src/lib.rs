//! Real-time bat detector DSP for microcontrollers.
//!
//! Written for the TASCAM Portacapture X8 (i.MX RT1062, Cortex-M7 at 600 MHz,
//! recording at 192 kHz), and usable on any target with an FPU.
//!
//! Every processor:
//! - is `no_std` and never allocates; buffers that depend on time (look-back,
//!   time expansion) are passed in by the caller, so they can live in a static
//!   or in SDRAM;
//! - works on `f32` samples in blocks: `process(&mut self, input, output)` with
//!   `input.len() == output.len()`, any block size, the same result whichever
//!   way a signal is split into blocks;
//! - takes the sample rate once, in its constructor, and has setters for the
//!   parameters a user changes while listening.
//!
//! The output sample rate is the input sample rate, because the X8's headphone
//! DAC runs at the recording rate.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod biquad;
pub mod detector;
pub mod freq_div;
pub mod heterodyne;
pub mod osc;
pub mod pitch_shift;
pub mod spectrum;
pub mod time_expansion;

#[cfg(test)]
pub(crate) mod test_util;

#[cfg(test)]
mod pipeline_tests;
