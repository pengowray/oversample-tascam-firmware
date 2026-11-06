//! Code shared by the integration tests of the chime crate. Each test file uses
//! it with `mod support;`.

#![allow(dead_code)]

pub mod channel;

// `gen` is a reserved keyword in Rust 2024, so the module for gen.rs has to be
// declared as `r#gen`. Tests can write `support::generator` for the same module.
pub mod r#gen;
pub use self::r#gen as generator;
