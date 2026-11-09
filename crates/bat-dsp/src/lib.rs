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
//!
//! Two modules are for playing files rather than processing a stream:
//! [`wav`] reads a WAV file's header and decodes its samples, and
//! [`resample`] converts a file's sample rate to the output rate, so its
//! `process` takes and gives different numbers of samples.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod biquad;
pub mod classify;
pub mod detector;
pub mod freq_div;
pub mod heterodyne;
pub mod osc;
pub mod pitch_shift;
pub mod resample;
pub mod spectrum;
pub mod time_expansion;
pub mod wav;

#[cfg(test)]
pub(crate) mod test_util;

#[cfg(test)]
mod pipeline_tests;
