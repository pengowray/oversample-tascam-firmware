//! Render the bat app's screen to a PNG after playing a WAV file into it.
//!
//! cargo run -p bat-app --release --example screenshot -- IN.wav OUT.png [SECONDS] [MODE] [SCALE] [OFFSET RANGE PER_ROW]
//!
//! SECONDS: how much of the file to play first (default: all of it).
//! MODE: het, comb, fd, te, pitch, settings for the settings screen, chime for the chime
//! test, which listens to the file, tools for the settings screen's Tests page (PAGE=record for Record), cputest for the CPU test's
//! screen (CPU_ROWS=N runs N rows first, with made-up figures, and CPU_STOP=1 stops it early)
//! or cpurun for the main screen during a CPU test (default het). SCALE: 1 to 4 (default 2).
//! OFFSET, RANGE (dB) and PER_ROW (0 or 1): waterfall contrast, as
//! `BatApp::set_display` (default: the app's defaults).
//! The file must be at 192 kHz; channel 1 feeds input 3, channel 2 input 4.
//! `MARK=bat,tone,audible,broadband` chooses which kinds of detection are
//! marked (default: bat).
//! `X8=standby|recording|paused` shows the screen as on the X8: four mode
//! buttons, and the X8's recorder in that state at 1 min 15 s.
//! `RATE=48000` shows the warning for an input at that rate.
//! `PLAYBACK=1` shows the screen as BAT PLAYBACK shows it on the X8.
//! `FILES=list|playing|paused|nofile|reading|empty|failed|folders|foldersreading|foldersfailed|nofolders` adds made-up recordings to BAT PLAYBACK's
//! player: `list` shows the list, the others the main screen or the list in that state.
//! `FILES=info|inforeading|infonone|infonotplayable|infomain` shows the file information view,
//! opened by a long press on the list (or from the title bar for `infomain`); `INFO_SCROLL=N`
//! turns the wheel N steps on it.
//! `BUILD=240` gives the app the X8 patch's build number, for the CPU test.
//! `INPUTS=5,6` or `INPUTS=5` names the inputs for the meters and settings;
//! `INPUTS=none` shows the notice for no input.
//! `SKIP=8.2-8.25,8.6-8.8` skips the analysis in those times, in seconds, as the
//! X8 patch does when it falls behind, to show the waterfall's grey columns.
//! `MIX=1` offers the host's mix in Listen to and chooses it; `MIX=recpause`,
//! `MIX=stopped` or `MIX=page` also shows the notice for why it is silent (REC PAUSE
//! on; REC PAUSE off; a settings page open). `DEBUG=1` offers the debug switch and draws the X8 patch's
//! longest debug lines over the last-call panel, to check that they fit.
//! `DRAW=on` or `DRAW=off` offers the Tests page's drawing switch (with `DEBUG=1`) in that state.
//! `AUTO=on` or `AUTO=bats` sets Auto volume, for the settings screen.
//! `GUANO=off` turns the GUANO metadata switch off, for the Record page.
//! `MUTE=on` turns Mute while recording on, for the Record page and the main screen with
//! `X8=recording`.
//! `REC_READY=off` shows MANUAL's MASTER with REC READY off, for the settings screen's MIX file row.
//! `TAKE_MIX=on` or `TAKE_MIX=off` says whether the take being recorded has a MIX file, for the CPU
//! test.
//! `LISTEN=first` or `LISTEN=second` chooses one side in Listen to, for the settings
//! screen; the default is both.

use bat_app::{
    AutoVolume, BatApp, Buffers, CallKind, Feed, FileEntry, HEIGHT, InputKind, InputName, MixSilence, Mode,
    MonitorSource, Recorder, Rgb565Buffer, WIDTH,
};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

/// Run the CPU test on silence for `rows` rows, with made-up figures that rise with the row. With
/// `rows` at the plan's length the test ends by itself. Otherwise it is left running.
fn run_cpu_test(app: &mut BatApp, rows: usize) {
    app.start_cpu_test();
    let plan = app.cpu_test_rows().len();
    let mut figures = bat_app::HostFigures { irq_percent: 43, hook_percent: 9, draw_percent: 13, ..Default::default() };
    let block = 192_000 / 5;
    let (a, b) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    for row in 0..rows.min(plan) {
        for _ in 0..40 {
            app.process(&a, &b, &mut l, &mut r);
            figures.dsp_percent = 35 + 2 * row as u32;
            figures.window = figures.window.wrapping_add(1);
            figures.waiting_most = 2_500 + 120 * row as u32;
            if row > 10 {
                figures.lost += 40 * (row as u32 - 10);
                figures.dry += (row as u32 / 8).min(1);
            }
            app.cpu_test_figures(figures);
        }
    }
    if rows < plan && std::env::var("CPU_STOP").is_ok() {
        app.touch(160, 200);
    }
}

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
    let seconds: f32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(total_frames as f32 / spec.sample_rate as f32);
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
    let mut file_storage = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 64];
    let mut details_storage = vec![0u8; bat_app::player::DETAILS_BYTES];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(fs, &mut storage, &mut buffers);
    app.set_mode(mode);
    if let (Some(o), Some(r), Some(p)) = (args.get(6), args.get(7), args.get(8)) {
        app.set_display(o.parse().unwrap(), r.parse().unwrap(), p == "1");
    }
    app.set_clock(21 * 3600 + 47 * 60 + 5);
    if let Ok(kinds) = std::env::var("MARK") {
        let kinds: Vec<&str> = kinds.split(',').collect();
        for (name, kind) in [
            ("bat", CallKind::Bat),
            ("tone", CallKind::Tone),
            ("audible", CallKind::Audible),
            ("broadband", CallKind::Broadband),
        ] {
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
            "stopped" => Some(MixSilence::Stopped),
            "page" => Some(MixSilence::PageOpen),
            _ => None,
        });
    }
    match std::env::var("LISTEN").as_deref() {
        Ok("first") => app.set_source(MonitorSource::First),
        Ok("second") => app.set_source(MonitorSource::Second),
        _ => {}
    }
    if let Ok(volume) = std::env::var("AUTO") {
        app.set_auto_volume(match volume.as_str() {
            "on" => AutoVolume::On,
            "bats" => AutoVolume::BatsOnly,
            _ => AutoVolume::Off,
        });
    }
    let debug = std::env::var("DEBUG").is_ok();
    app.set_debug_available(debug);
    if let Ok(draw) = std::env::var("DRAW") {
        app.set_draw_option_available(true);
        app.set_draw_on_input(draw == "on");
    }
    if let Ok(build) = std::env::var("BUILD") {
        app.set_host_build(build.parse().unwrap());
    }
    app.set_mix_file_available(debug && std::env::var("PLAYBACK").is_err());
    app.set_guano_available(debug && std::env::var("PLAYBACK").is_err());
    app.set_guano(std::env::var("GUANO").as_deref() != Ok("off"));
    app.set_pause_option_available(debug && std::env::var("PLAYBACK").is_err());
    app.set_mute_while_recording(std::env::var("MUTE").as_deref() == Ok("on"));
    app.set_manual_mix_file(std::env::var("REC_READY").as_deref() != Ok("off"));
    app.set_take_mix_file(match std::env::var("TAKE_MIX").as_deref() {
        Ok("on") => Some(true),
        Ok("off") => Some(false),
        _ => None,
    });
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
    if args.get(4).map(String::as_str) == Some("chime") {
        app.open_chime_test();
    }
    let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
    let skips: Vec<(f32, f32)> = std::env::var("SKIP").map_or(Vec::new(), |list| {
        list.split(',')
            .map(|range| {
                let (from, to) = range.split_once('-').expect("FROM-TO");
                (from.parse().expect("a time"), to.parse().expect("a time"))
            })
            .collect()
    });
    for k in (0..n).step_by(1024) {
        let e = (k + 1024).min(n);
        let t = k as f32 / fs;
        app.set_skip_analysis(skips.iter().any(|&(from, to)| t >= from && t < to));
        app.process(&a[k..e], &b[k..e], &mut l[k..e], &mut r[k..e]);
    }

    // X8_BEHIND=N: the X8's clock N s behind the phone's, after a chime on the phone time screen;
    // CLOCK_STATE=after shows the status line for a set waiting for the take to end.
    if let Ok(behind) = std::env::var("X8_BEHIND")
        && let Some(time) = app.take_phone_time()
    {
        app.set_host_clock(Some(time.local_ms.div_euclid(1000) - behind.parse::<i64>().unwrap()));
    }
    // PAUSE_REFUSED=1: the main screen's notice that REC did not pause the take.
    if std::env::var("PAUSE_REFUSED").is_ok() {
        app.pause_refused();
    }
    if std::env::var("CLOCK_STATE").as_deref() == Ok("after") {
        app.set_clock_state(bat_app::ClockState::AfterTake);
    }
    if args.get(4).map(String::as_str) == Some("settings") {
        app.open_settings_page(match std::env::var("PAGE").as_deref() {
            Ok("record") => bat_app::SettingsPage::Record,
            Ok("phone") => bat_app::SettingsPage::Phone,
            Ok("tests") => bat_app::SettingsPage::Tests,
            _ => bat_app::SettingsPage::Listen,
        });
    }
    match args.get(4).map(String::as_str) {
        // As BAT DETECTOR on the X8 has it: all four tabs.
        Some("tools") => {
            app.set_remote_available(true);
            app.set_remote_on(true);
            app.set_debug_available(true);
            app.set_mix_file_available(true);
            app.open_settings_page(match std::env::var("PAGE").as_deref() {
                Ok("record") => bat_app::SettingsPage::Record,
                Ok("phone") => bat_app::SettingsPage::Phone,
                _ => bat_app::SettingsPage::Tests,
            });
        }
        Some("cputest") => {
            app.open_cpu_test();
            if let Ok(rows) = std::env::var("CPU_ROWS") {
                run_cpu_test(&mut app, rows.parse().unwrap());
            }
        }
        // RESULT picks another phone remote notice: clockset, aftertake, clockfailed, mode, nopitch.
        Some("unknown") => {
            app.set_remote_available(true);
            app.set_remote_on(true);
            app.remote_result(match std::env::var("RESULT").as_deref() {
                Ok("clockset") => bat_app::RemoteResult::ClockSet,
                Ok("aftertake") => bat_app::RemoteResult::ClockAfterTake,
                Ok("clockfailed") => bat_app::RemoteResult::ClockFailed,
                Ok("mode") => bat_app::RemoteResult::ModeSet(Mode::Comb),
                Ok("nopitch") => bat_app::RemoteResult::ModeNotOffered(Mode::Pitch),
                _ => bat_app::RemoteResult::Unknown,
            });
        }
        Some("cpurun") => {
            app.set_remote_available(true);
            app.set_remote_on(true);
            app.remote_result(bat_app::RemoteResult::Started);
            run_cpu_test(&mut app, 7);
        }
        _ => {}
    }
    if let Ok(files) = std::env::var("FILES") {
        app.set_input_kind(InputKind::Playback);
        app.set_inputs([Some(InputName { short: "L", long: "Left" }), Some(InputName { short: "R", long: "Right" })]);
        fake_files(&mut app, &mut file_storage, &mut details_storage, &files);
    }
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    let mut fb = Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT);
    app.render(&mut fb);
    if debug && !matches!(args.get(4).map(String::as_str), Some("settings" | "tools" | "cputest")) {
        // The longest lines of the patch's draw_debug that a test should reach, in
        // its font and strips. Line 3 has no room for every count at its largest
        // at once: then the last digits of the output ring level are cut off.
        const LINES: [&str; 4] = [
            "HET | in 5+6 | leaving standby | 192 kHz | auto bats only",
            "CPU % irq 100 hook 100 itcm dsp 100 (max 100) draw 100",
            "dry 9999 lost 999999 skip 9999 in 999 s | ring in 8192 out 15360",
            "on 3f mix -99 dB: 99 99 99 99 99 99 99 99 99 99 99 99",
        ];
        let font = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();
        let (top, height) = app.last_call_area();
        let _ = fb.fill_solid(
            &Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, height)),
            embedded_graphics::pixelcolor::Rgb565::new(4, 8, 4),
        );
        for (i, line) in LINES.iter().enumerate() {
            let y = top + i as i32 * 13;
            let width = font
                .get_rendered_dimensions(*line, Point::new(2, y + 10), VerticalPosition::Baseline)
                .map_or(0, |d| d.advance.x);
            eprintln!("debug line {}: {} px wide, starting at x = 2, of {WIDTH}", i + 1, width);
            let _ = fb.fill_solid(
                &Rectangle::new(Point::new(0, y), Size::new(WIDTH as u32, 13)),
                embedded_graphics::pixelcolor::Rgb565::new(4, 8, 4),
            );
            let _ = font.render(
                *line,
                Point::new(2, y + 10),
                VerticalPosition::Baseline,
                FontColor::Transparent(embedded_graphics::pixelcolor::Rgb565::WHITE),
                &mut fb,
            );
        }
    }
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

/// Made-up recordings for BAT PLAYBACK's player, in the state `which` names (see the top of this file).
fn fake_files<'a>(app: &mut BatApp<'a>, storage: &'a mut [FileEntry], details: &'a mut [u8], which: &str) {
    use bat_app::player::{Audio, FileInfo, NotPlayable};
    let player = app.player_mut();
    player.set_storage(storage);
    player.set_details_storage(details);
    player.scan_started();
    if which == "failed" {
        app.open_files();
        app.player_mut().scan_finished(false);
        return;
    }
    player.add_folder(0, b"SOUND");
    player.add_folder(1, b"FOLDER_0001");
    player.add_folder(2, b"FOLDER_0002");
    let date = |y: u16, m: u16, d: u16| ((y - 1980) << 9) | (m << 5) | d;
    let time = |h: u16, m: u16| (h << 11) | (m << 5);
    let x8 = |frames: u32, channels: u16| FileInfo::Playable(Audio { sample_rate: 192_000, channels, frames });
    let files: [(&[u8], u8, u16, u16, FileInfo); 11] = [
        (b"260315_0066_3-4.wav", 2, date(2026, 3, 15), time(21, 4), x8(192_000 * 754, 2)),
        (b"260315_0065_3-4.wav", 2, date(2026, 3, 15), time(20, 51), x8(192_000 * 92, 2)),
        (b"260315_0064.wav", 2, date(2026, 3, 15), time(20, 40), FileInfo::Unknown),
        (b"260306_0061_3-4.wav", 1, date(2026, 3, 6), time(19, 12), x8(192_000 * 2792, 2)),
        (b"260306_0062_3-4.wav", 1, date(2026, 3, 6), time(19, 59), x8(192_000 * 2792, 2)),
        (b"260223_0048_3-4.wav", 1, date(2026, 2, 23), time(21, 47), x8(192_000 * 41, 1)),
        (
            b"20260131_210710.WAV",
            0,
            date(2026, 1, 31),
            time(21, 7),
            FileInfo::Playable(Audio { sample_rate: 384_000, channels: 1, frames: 384_000 * 10 }),
        ),
        (
            b"20260131_210721.WAV",
            0,
            date(2026, 1, 31),
            time(21, 7),
            FileInfo::Playable(Audio { sample_rate: 384_000, channels: 1, frames: 384_000 * 10 }),
        ),
        (
            b"XC809855 - Common Pipistrelle - Pipistrellus pipistrellus.wav",
            0,
            date(2025, 11, 2),
            time(10, 30),
            FileInfo::Playable(Audio { sample_rate: 250_000, channels: 1, frames: 250_000 * 8 }),
        ),
        (b"TEST_8BIT.WAV", 0, date(2025, 6, 1), time(9, 0), FileInfo::NotPlayable(NotPlayable::Unsupported)),
        (b"BROKEN.WAV", 0, 0, 0, FileInfo::NotPlayable(NotPlayable::NoAudio)),
    ];
    if which != "empty" {
        for (id, (name, folder, d, t, info)) in files.into_iter().enumerate() {
            // The size an X8 take of that length has: its 5,548-byte header, then 32-bit samples.
            let bytes = match info {
                FileInfo::Playable(a) => 5_548 + a.frames * u32::from(a.channels) * 4,
                _ => 1_000_000,
            };
            let mut e = FileEntry::new(id as u16, folder, name, d, t, bytes);
            e.info = info;
            player.add_file(e);
        }
    }
    if which == "reading" {
        app.open_files();
        return;
    }
    player.scan_finished(true);
    match which {
        "end" => {
            app.open_files();
            app.player_mut().move_highlight(10, 8);
        }
        "longplaying" => {
            app.player_mut().choose(8);
            app.player_mut().opened();
        }
        "list" | "empty" => {
            if which == "list" {
                app.player_mut().choose(1);
                app.player_mut().opened();
            }
            app.open_files();
            app.player_mut().move_highlight(3, 8);
        }
        "info" | "inforeading" | "infonone" | "infonotplayable" | "infomain" => {
            // A long press on a row of the list, or a tap on the title bar's information button.
            let file = match which {
                "infonone" => 6,
                "infonotplayable" => 9,
                _ => 5,
            };
            if which == "infomain" {
                app.player_mut().choose(file);
                app.player_mut().opened();
                app.touch(266, 12);
            } else {
                app.open_files();
                let player = app.player_mut();
                // The last 8 of the 11 rows, so rows 3 to 10 are shown.
                player.scroll_to(3, 8);
                let row_y = 50 + (file as i32 - 3) * 47 + 20;
                app.touch(160, row_y);
                app.hold();
                app.release();
            }
            let mut details = None;
            while let Some(request) = app.player_mut().take_request() {
                if let bat_app::player::Request::Details { id } = request {
                    details = Some(id);
                }
            }
            let Some(id) = details else { return };
            let xri: &[u8] = b"T=XRI 008a;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=3:EXT MIC,4:EXT MIC;\
INPUT GAIN=3:LOW,4:LOW;LEVEL=3:20,4:20;AUTO_LEVEL=3:OFF,4:OFF;,";
            let guano: &[u8] = b"GUANO|Version: 1.0\nTimestamp: 2026-03-15T20:51:26+11:00\nLength: 754.000000\n\
Samplerate: 192000\nMake: TASCAM\nModel: Portacapture X8\nFirmware Version: V2.03 with bat patch B258\n\
Original Filename: 260315_0066_3-4.wav\nLoc Position: -36.123456 146.000007\nX8Bat|Pauses: 0\nX8Bat|Paused: 0.000\n\
Note: Gatehouse bat hole, SO.4 mics on inputs 3 and 4, Marshmallow on input 5, about two metres from the roost\n";
            use bat_app::player::Encoding;
            match which {
                "inforeading" => {}
                "infonone" => app.player_mut().details_read(id, Some((Some(Encoding::Pcm16), b"", b""))),
                "infonotplayable" => app.player_mut().details_read(id, Some((None, b"", b""))),
                _ => app.player_mut().details_read(id, Some((Some(Encoding::Float32), xri, guano))),
            }
            if let Ok(steps) = std::env::var("INFO_SCROLL") {
                app.wheel(steps.parse().unwrap());
            }
        }
        "folders" | "foldersreading" | "foldersfailed" | "nofolders" => {
            app.open_files();
            // Tap Folder, at the bottom right.
            app.touch(213, 453);
            app.release();
            let player = app.player_mut();
            let names: &[&[u8]] = match which {
                "folders" => &[b"PortaX8", b"BATS", b"SOUND", b"Xeno-canto downloads 2025", b"bats 384k"],
                "foldersreading" => &[b"BATS", b"SOUND"],
                _ => &[],
            };
            for name in names {
                player.add_top_folder(name);
            }
            match which {
                "folders" | "nofolders" => player.folders_finished(true),
                "foldersfailed" => player.folders_finished(false),
                _ => {}
            }
        }
        "playing" | "paused" => {
            let player = app.player_mut();
            player.choose(1);
            player.opened();
            player.set_position(192_000 * 31);
            if which == "paused" {
                player.play_pause();
            }
        }
        _ => {}
    }
}
