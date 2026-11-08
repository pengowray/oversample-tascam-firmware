#!/bin/sh
# Builds the patch for the X8. Same as `cargo build -p x8-patch --release --target
# thumbv7em-none-eabihf`, but with Cargo's home folder removed from the source paths the
# firmware stores for error messages, so the same source and Rust version give the same update
# file on any computer. Extra arguments go to cargo.
set -eu
cargo_home=${CARGO_HOME:-$HOME/.cargo}
cd "$(dirname "$0")/.."
exec cargo build -p x8-patch --release --target thumbv7em-none-eabihf \
    --config "target.thumbv7em-none-eabihf.rustflags=[\"--remap-path-prefix=$cargo_home=/cargo\"]" "$@"
