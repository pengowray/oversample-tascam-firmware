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

use bat_app::{BatApp, CallKind, HEIGHT, Mode, Rgb565Buffer, WIDTH};

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
        _ => Mode::Het,
    };
    let scale: usize = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(2).clamp(1, 4);
    let n = ((seconds * spec.sample_rate as f32) as usize).min(frames);

    let fs = spec.sample_rate as f32;
    let mut storage = vec![0.0f32; spec.sample_rate as usize];
    let mut app = BatApp::new(fs, &mut storage);
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
    app.render(&mut Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT));
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
