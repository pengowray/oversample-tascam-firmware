# X8 bat app emulator

A web page that runs the bat app (`crates/bat-app`) in the browser: the same
code that would run on the Portacapture X8, compiled to WebAssembly, with
audio at 192 kHz from a WAV file or a microphone, and the X8's 320 × 480 screen
drawn to a canvas. Recording is simulated.

## Run

```bash
rustup target add wasm32-unknown-unknown
```

```bash
./web/build.sh
```

```bash
python3 -m http.server 8742 --directory web
```

Then open <http://localhost:8742/>. To load a file from the server at start,
put it in `web/samples/` (ignored by git) and open
`http://localhost:8742/?wav=samples/NAME.wav`.

`build.sh` needs the `wasm-bindgen` command-line tool at the version pinned in
`crates/bat-app-web/Cargo.toml` (`cargo install wasm-bindgen-cli --version 0.2.127`).

## How it works

- `crates/bat-app-web` wraps `BatApp` for JavaScript: `process` for audio
  blocks, `button`, `wheel`, `touch` and `set_mode` for input, and `render`
  plus `frame_ptr` for the screen.
- `app.js` decodes files with `decodeAudioData` into a 192 kHz `AudioContext`,
  so files at other sample rates are resampled. Audio runs through a
  `ScriptProcessorNode` (deprecated, but it calls the WebAssembly synchronously),
  then the volume and a limiter.
- On loading a file, the volume is set so the file's loudest sample comes out
  near -12 dBFS, because 32-bit float bat recordings are often very quiet.

## Screens without a browser

`crates/bat-app/examples/screenshot.rs` plays a 192 kHz WAV into the app and
saves the screen as a PNG:

```bash
cargo run -p bat-app --release --example screenshot -- IN.wav OUT.png 1.25 het 2
```

The arguments after the file names are seconds to play, the mode (`het`,
`comb`, `fd`, `te` or `pitch`) and the scale; three more set the waterfall
contrast (offset dB, range dB, per-row 0 or 1). `docs/screens/` has one per
mode.

`crates/bat-app/examples/calls.rs` lists the calls the app's detector finds in
a file, with a table of calls by peak frequency and length, for tuning on real
recordings:

```bash
cargo run -p bat-app --release --example calls -- IN.wav [FROM_S] [TO_S]
```
