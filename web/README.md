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
python3 web/serve.py
```

(`serve.py` is Python's built-in server with caching turned off, so a rebuilt
module is always picked up.)

Then open <http://localhost:8742/>. To load a file from the server at start,
put it in `web/samples/` (ignored by git) and open
`http://localhost:8742/?wav=samples/NAME.wav`. Add `&mode=te` (or `het`, `comb`,
`fd`, `pitch`) to start in that listening mode.

**Stereo.** In HET, COMB and 1:1 with both inputs chosen in Listen to, which is the
default, channel 1 of a two-channel WAV file plays in the left ear and channel 2 in
the right ear. FD, TE and PITCH play the average of the two channels in both ears. A
mono file plays as both inputs, so it sounds the same in both ears. 1:1 has no `mode=`
value or number key: tap its button on the screen. The emulator page has no MANUAL's
mix, BAT PLAYBACK, X8 output gain or audio rings, and its Auto volume aims for a
different level from the X8's. Use the page to hear the left and right separation, and
not to judge X8 levels or dropouts.

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

`crates/bat-app/examples/library.rs` does the same over a folder of
Xeno-canto recordings with their `.xc.json` metadata (as Oversample's xc-fetch
saves them), resampling each to 192 kHz, and writes one CSV row per recording:

```bash
cargo run -p bat-app --release --example library -- DIR OUT.csv
```

Both take `AUDIBLE=LO,HI` (Hz) to try another audible band.

## Testing a chime recording

The phone page that plays the chime, `Bat recorder setup by sound`, is in its own repository,
`batchi-chime`, beside this one in `~/projects`.

Firmware B235 and later can read the chime live: in the app's settings, tap `Chime test`, and
the screen shows the time from the chime. It does not change the X8's clock. The emulator has
the same screen. To test a recording on any firmware, record the chime on the X8 at 192 kHz,
copy the WAV to the computer and run:

```bash
cargo run --release -p chime --example decode_wav -- FILE.wav
```

It reports the time in the chime, the signal level and SNR, and how far the X8's clock
differs from the phone's. The X8's clock at the start of the file is the time in the WAV's
`bext` chunk, which the X8 writes as it closes the file, less the file's length. That is
accurate to about 1 s, so the difference is in whole seconds. It uses the same decoder as the firmware,
the `chime` crate.
