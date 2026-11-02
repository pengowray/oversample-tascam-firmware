// Link the patch at its fixed address when building for the X8; on the host
// the crate is an empty program, so workspace-wide builds and tests still work.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("thumbv7em") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins=-T{dir}/x8patch.ld");
    }
    println!("cargo:rerun-if-changed=x8patch.ld");
}
