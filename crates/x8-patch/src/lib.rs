//! The parts of the X8 patch that have no firmware addresses, so `cargo test -p x8-patch` can run
//! their tests on the host. The patch itself is the `x8-patch` binary (src/main.rs).

#![cfg_attr(not(test), no_std)]

pub mod file_mask;
pub mod take_guano;
