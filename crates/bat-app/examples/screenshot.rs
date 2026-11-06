//! Render the bat app's screen to a PNG after playing a WAV file into it.
//!
//! cargo run -p bat-app --release --example screenshot -- IN.wav OUT.png [SECONDS] [MODE] [SCALE] [OFFSET RANGE PER_ROW]
//!
//! SECONDS: how much of the file to play first (default: all of it).
//! MODE: het, comb, fd, te, pitch, or settings for the settings screen
//! (default het). SCALE: 1 to 4 (default 2).
//! OFFSET, RANGE (dB) and PER_ROW (0 or 1): waterfall contrast, as
//! `BatApp::set_display` (default: the app's defaults).
//! The file must be at 192 kHz; channel 1 feeds input 3, channel 2 input 4.
//! `MARK=bat,tone,audible,broadband` chooses which kinds of detection are
//! marked (default: bat).
//! `X8=standby|recording|paused` shows the screen as on the X8: four mode
//! buttons, and the X8's recorder in that state at 1 min 15 s.
//! `RATE=48000` shows the warning for an input at that rate.
//! `PLAYBACK=1` shows the screen as BAT PLAYBACK shows it on the X8.
//! `INPUTS=5,6` or `INPUTS=5` names the inputs for the meters and settings;
//! `INPUTS=none` shows the notice for no input.
//! `MIX=1` offers the host's mix in Listen to and chooses it; `MIX=recpause` or
//! `MIX=page` also shows a notice that it is silent. `DEBUG=1` offers the debug switch and draws the X8 patch's
//! longest debug lines over the last-call panel, to check that they fit.

use bat_app::{BatApp, Buffers, CallKind, Feed, HEIGHT, InputKind, InputName, MixSilence, Mode, Recorder, Rgb565Buffer, WIDTH};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: screenshot IN.wav OUT.png [SECONDS] [MODE] [SCALE]");
        std::process::exit(2);
    }
    let mut reader = hound::WavReader::open(&args[1]).expect("open wav");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let total_frames = reader.duration() as usize;
    let seconds: f32 =
        args.get(3).map(|s| s.parse().unwrap()).unwrap_or(total_frames as f32 / spec.sample_rate as f32);
    let wanted = ((seconds * spec.sample_rate as f32) as usize).min(total_frames) * channels;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().take(wanted).map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().take(wanted).map(|s| s.unwrap() as f32 * scale).collect()
        }
    };
    let frames = samples.len() / channels;
    let mode = match args.get(4).map(String::as_str).unwrap_or("het") {
        "comb" => Mode::Comb,
        "fd" => Mode::Fd,
        "te" => Mode::Te,
        "pitch" => Mode::Pitch,
        "thru" => Mode::Thru,
        _ => Mode::Het,
    };
    let scale: usize = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(2).clamp(1, 4);
    let n = ((seconds * spec.sample_rate as f32) as usize).min(frames);

    let fs = spec.sample_rate as f32;
    let mut storage = vec![0.0f32; spec.sample_rate as usize];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(fs, &mut storage, &mut buffers);
    app.set_mode(mode);
    if let (Some(o), Some(r), Some(p)) = (args.get(6), args.get(7), args.get(8)) {
        app.set_display(o.parse().unwrap(), r.parse().unwrap(), p == "1");
    }
    app.set_clock(21 * 3600 + 47 * 60 + 5);
    if let Ok(kinds) = std::env::var("MARK") {
        let kinds: Vec<&str> = kinds.split(',').collect();
        for (name, kind) in
            [("bat", CallKind::Bat), ("tone", CallKind::Tone), ("audible", CallKind::Audible), ("broadband", CallKind::Broadband)]
        {
            app.set_marked(kind, kinds.contains(&name));
        }
    }
    if let Ok(state) = std::env::var("X8") {
        app.set_modes(&Mode::WITHOUT_PITCH);
        let state = match state.as_str() {
            "standby" => Recorder::Standby,
            "recording" => Recorder::Recording,
            "paused" => Recorder::Paused,
            _ => Recorder::Stopped,
        };
        app.set_recorder(state, 75);
    }
    if let Ok(rate) = std::env::var("RATE") {
        app.set_input_rate(rate.parse().unwrap());
    }
    if std::env::var("PLAYBACK").is_ok_and(|v| v == "1") {
        app.set_input_kind(InputKind::Playback);
        app.set_inputs([Some(InputName { short: "L", long: "Left" }), Some(InputName { short: "R", long: "Right" })]);
    }
    if let Ok(mix) = std::env::var("MIX") {
        app.set_mix_available(true);
        app.set_feed(Feed::Mix);
        app.set_mix_silence(match mix.as_str() {
            "recpause" => Some(MixSilence::RecPause),
            "page" => Some(MixSilence::PageOpen),
            _ => None,
        });
    }
    let debug = std::env::var("DEBUG").is_ok();
    app.set_debug_available(debug);
    if let Ok(inputs) = std::env::var("INPUTS") {
        let name = |n: &str| {
            let short: &'static str = String::leak(n.to_string());
            let long: &'static str = String::leak(format!("Input {n}"));
            InputName { short, long }
        };
        let mut parts = inputs.split(',').filter(|&part| part != "none");
        app.set_inputs([parts.next().map(name), parts.next().map(name)]);
    }
    let ch = |c: usize| -> Vec<f32> { (0..n).map(|i| samples[i * channels + c.min(channels - 1)]).collect() };
    let (a, b) = (ch(0), ch(1));
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    for k in (0..n).step_by(4096) {
        let e = (k + 4096).min(n);
        app.process(&a[k..e], &b[k..e], &mut l[k..e], &mut r[k..e]);
    }

    if args.get(4).map(String::as_str) == Some("settings") {
        app.open_settings();
    }
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    let mut fb = Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT);
    app.render(&mut fb);
    if debug && args.get(4).map(String::as_str) != Some("settings") {
        // The longest lines of the patch's draw_debug, in its font and strips.
        const LINES: [&str; 4] = [
            "HET | in 5+6 | leaving standby | 192 kHz | auto",
            "CPU % irq 100 hook 100 dsp 100 (max 100) draw 100",
            "dry 12345 lost 123456 in 3600 s | ring in 8192 out 16384",
            "on 3f mix -99 dB: 99 99 99 99 99 99 99 99 99 99 99 99",
        ];
        let font = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();
        let (top, height) = app.last_call_area();
        let _ = fb.fill_solid(&Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, height)), embedded_graphics::pixelcolor::Rgb565::new(4, 8, 4));
        for (i, line) in LINES.iter().enumerate() {
            let y = top + i as i32 * 13;
            let _ = fb.fill_solid(&Rectangle::new(Point::new(0, y), Size::new(WIDTH as u32, 13)), embedded_graphics::pixelcolor::Rgb565::new(4, 8, 4));
            let _ = font.render(*line, Point::new(2, y + 10), VerticalPosition::Baseline, FontColor::Transparent(embedded_graphics::pixelcolor::Rgb565::WHITE), &mut fb);
        }
    }
    drop(fb);
    let (w, h) = (WIDTH * scale, HEIGHT * scale);
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let c = pixels[(y / scale) * WIDTH + x / scale];
            let (r5, g6, b5) = ((c >> 11) & 0x1f, (c >> 5) & 0x3f, c & 0x1f);
            let o = (y * w + x) * 3;
            rgb[o] = ((r5 << 3) | (r5 >> 2)) as u8;
            rgb[o + 1] = ((g6 << 2) | (g6 >> 4)) as u8;
            rgb[o + 2] = ((b5 << 3) | (b5 >> 2)) as u8;
        }
    }
    let file = std::fs::File::create(&args[2]).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    if let Some(call) = app.last_call() {
        eprintln!("last call ({:?}): {call:?}", app.last_call_kind().unwrap());
    }
}
