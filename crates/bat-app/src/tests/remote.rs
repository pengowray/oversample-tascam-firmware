//! The phone remote: what wakes the decoders, what they read, and what they ignore.

use std::boxed::Box;

use super::*;
use crate::screen::pages_lines;
use crate::{RemoteCommand, RemoteResult, Screen};

#[path = "../../../chime/tests/support/gen.rs"]
#[allow(dead_code)]
mod chime_gen;

const SECOND: usize = FS as usize;
const UNIX: u32 = 1_793_000_000;
/// What the phone page puts in the deployment ID, as the decoder gives it.
const START: [u8; 8] = *b"BATXREC\x01";
const STOP: [u8; 8] = *b"BATXREC\x02";

/// The chime for `id`, with the page's melody.
fn command_chime(id: [u8; 8], unix: u32, carrier_gain: f32) -> Vec<f32> {
    let mut bytes = chime_gen::time_payload(unix, 600);
    bytes.extend_from_slice(&id);
    let render = chime_gen::render(FS as u32, &bytes, chime_gen::tune_for_payload(bytes.len()));
    render.mix().iter().map(|&v| v as f32 * carrier_gain).collect()
}

/// The library's tone: the carrier in pairs of 10 ms and 5 ms symbols, with the polarity turned
/// over at each symbol, as `audiomothchime.js` makes it.
fn page_tone(ms: usize, gain: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut polarity = 1.0f32;
    let mut lengths = [10_usize, 5].into_iter().cycle();
    while out.len() < ms * SECOND / 1000 {
        let n = lengths.next().unwrap() * SECOND / 1000;
        let ramp = SECOND / 2000;
        for i in 0..n {
            let edge = (i.min(n - i) as f32 / ramp as f32).min(1.0);
            let envelope = (edge * core::f32::consts::FRAC_PI_2).sin().powi(2);
            let t = out.len() as f32 / FS;
            out.push(0.25 * gain * polarity * envelope * (core::f32::consts::TAU * 18_000.0 * t).sin());
        }
        polarity = -polarity;
    }
    out
}

fn noise(n: usize, amplitude: f32, seed: &mut u32) -> Vec<f32> {
    (0..n)
        .map(|_| {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * amplitude
        })
        .collect()
}

fn add_at(x: &mut [f32], at: usize, signal: &[f32]) {
    for (a, b) in x[at..].iter_mut().zip(signal) {
        *a += *b;
    }
}

/// What the phone plays for a command: the tone, a wait, the chime, in noise.
fn press(id: [u8; 8], unix: u32, gain: f32, noise_level: f32) -> Vec<f32> {
    let tone = page_tone(500, gain);
    let chime = command_chime(id, unix, gain);
    let lead = SECOND;
    let wait = SECOND;
    let mut seed = unix;
    let mut x = noise(lead + tone.len() + wait + chime.len() + SECOND, noise_level, &mut seed);
    add_at(&mut x, lead, &tone);
    add_at(&mut x, lead + tone.len() + wait, &chime);
    x
}

fn run(app: &mut BatApp, x: &[f32]) -> Vec<RemoteCommand> {
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    let mut commands = Vec::new();
    for block in x.chunks(1024) {
        app.process(block, block, &mut l[..block.len()], &mut r[..block.len()]);
        commands.extend(app.take_remote_command());
    }
    commands
}

fn remote_app<'a>(storage: &'a mut [f32], buffers: &'a mut Buffers) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers);
    app.set_remote_available(true);
    app.set_remote_on(true);
    app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), None]);
    app
}

#[test]
fn a_tone_then_a_chime_gives_the_command() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RemoteCommand::Start]);
    assert_eq!(app.remote.tones, 1);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 10, 0.1, 1e-4)), [RemoteCommand::Stop]);
}

#[test]
fn with_location_the_chime_still_gives_the_command() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    let mut bytes = chime_gen::time_payload(UNIX, 600);
    bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
    bytes.extend_from_slice(&STOP);
    let render = chime_gen::render(FS as u32, &bytes, chime_gen::tune_for_payload(bytes.len()));
    let chime: Vec<f32> = render.mix().iter().map(|&v| v as f32 * 0.1).collect();
    let tone = page_tone(500, 0.1);
    let mut seed = 5;
    let mut x = noise(SECOND + tone.len() + SECOND + chime.len() + SECOND, 1e-4, &mut seed);
    add_at(&mut x, SECOND, &tone);
    add_at(&mut x, SECOND + tone.len() + SECOND, &chime);
    assert_eq!(run(&mut app, &x), [RemoteCommand::Stop]);
}

#[test]
fn without_the_tone_the_chime_only_wakes_the_decoders() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    let chime = command_chime(START, UNIX, 0.1);
    let mut seed = 9;
    let mut x = noise(SECOND + chime.len() + SECOND, 1e-4, &mut seed);
    add_at(&mut x, SECOND, &chime);
    assert_eq!(run(&mut app, &x), []);
    assert_eq!(app.remote.tones, 1);
}

#[test]
fn a_chime_without_a_command_does_nothing() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    // Another recorder's deployment ID, and an unknown command.
    assert_eq!(run(&mut app, &press(*b"BATXREC\x03", UNIX, 0.1, 1e-4)), []);
    assert_eq!(run(&mut app, &press(*b"AUDIOMOT", UNIX + 10, 0.1, 1e-4)), []);
    assert_eq!(app.remote.chimes_read, 2);
    assert!(app.remote.last.is_none());
}

#[test]
fn the_same_chime_on_both_inputs_gives_one_command() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), Some(InputName { short: "4", long: "Input 4" })]);
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RemoteCommand::Start]);
    assert_eq!(app.remote.chimes_read, 2);
}

#[test]
fn with_the_switch_off_nothing_listens() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.set_remote_on(false);
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), []);
    assert_eq!(app.remote.tones, 0);
    app.set_remote_on(true);
    app.set_remote_available(false);
    assert_eq!(run(&mut app, &press(START, UNIX + 10, 0.1, 1e-4)), []);
    assert_eq!(app.remote.tones, 0);
}

#[test]
fn noise_and_bat_calls_do_not_wake_the_decoders() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    let mut seed = 3;
    let x = noise(10 * SECOND, 1e-3, &mut seed);
    assert_eq!(run(&mut app, &x), []);
    assert_eq!(app.remote.tones, 0);
}

#[test]
fn the_chime_test_takes_the_decoders_and_the_remote_rebuilds_them() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RemoteCommand::Start]);
    app.open_chime_test();
    run(&mut app, &press(START, UNIX + 10, 0.1, 1e-4));
    app.open_settings_page(crate::SettingsPage::Record);
    assert_eq!(app.screen(), Screen::Settings);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 20, 0.1, 1e-4)), [RemoteCommand::Stop]);
}

#[test]
fn a_result_beeps_in_both_ears_and_then_stops() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    let silence = vec![0.0f32; 1024];
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    app.process(&silence, &silence, &mut l, &mut r);
    assert!(l.iter().all(|&v| v.abs() < 1e-6));
    app.remote_result(RemoteResult::Started);
    let mut peak = 0.0f32;
    for _ in 0..50 {
        app.process(&silence, &silence, &mut l, &mut r);
        assert_eq!(l, r);
        peak = peak.max(l.iter().fold(0.0, |m: f32, v| m.max(v.abs())));
    }
    assert!((0.015..=0.025).contains(&peak), "{peak}");
    assert!(!app.remote.beeping());
    app.process(&silence, &silence, &mut l, &mut r);
    assert!(l.iter().all(|&v| v.abs() < 1e-6));
}

#[test]
fn a_result_shows_for_a_while() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.remote_result(RemoteResult::HoldOn(RemoteCommand::Start));
    assert!(app.remote.result_shown(app.samples).is_some());
    let silence = vec![0.0f32; 6 * SECOND];
    run(&mut app, &silence);
    assert!(app.remote.result_shown(app.samples).is_none());
}

/// The level of the carrier in dBFS RMS for a press at `gain`: the tone's and chime's carrier is
/// 0.25 x gain.
fn carrier_dbfs(gain: f32) -> f32 {
    20.0 * (0.25 * gain / core::f32::consts::SQRT_2).log10()
}

/// Prints how far the carrier can fall before the tone stops waking the decoders, and before the
/// chime stops being read, with a noise floor like the X8's. Run it with `--nocapture`.
#[test]
fn the_tone_is_heard_at_least_as_far_down_as_the_chime_is_read() {
    // -94 dBFS in the chime's 2 kHz band, about what the X8 showed on the unit: 18 to 21 dB of
    // SNR with the carrier at -74 to -78 dBFS. Uniform noise of this size has that density.
    let noise_level = 2.4e-4;
    let (mut lowest_tone, mut lowest_chime) = (0.0f32, 0.0f32);
    for carrier in [-60.0f32, -66.0, -70.0, -74.0, -78.0, -82.0, -85.0, -88.0, -91.0, -94.0, -97.0] {
        let gain = 10f32.powf(carrier / 20.0) * core::f32::consts::SQRT_2 / 0.25;
        let (mut tones, mut commands) = (0, 0);
        for repeat in 0..4 {
            let mut storage = vec![0.0f32; SECOND];
            let mut buffers = Box::new(Buffers::new());
            let mut app = remote_app(&mut storage, &mut buffers);
            let x = press(START, UNIX + repeat * 37, gain, noise_level);
            commands += run(&mut app, &x).len();
            tones += app.remote.tones;
        }
        std::println!("carrier {:6.1} dBFS: tone heard {tones} of 4, command read {commands} of 4", carrier_dbfs(gain));
        if tones == 4 {
            lowest_tone = carrier_dbfs(gain);
        }
        if commands == 4 {
            lowest_chime = carrier_dbfs(gain);
        }
    }
    // The chime test read chimes on the unit at -74 to -78 dBFS.
    assert!(lowest_tone <= -78.0, "{lowest_tone}");
    assert!(lowest_chime <= -78.0, "{lowest_chime}");
}

#[test]
fn if_the_first_chime_fails_the_second_tone_keeps_the_decoders_running() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    // The page's two plays. The first chime is cut off at 0.5 s, and so cannot be read.
    let tone = page_tone(500, 0.1);
    let mut first_chime = command_chime(START, UNIX, 0.1);
    first_chime.truncate(SECOND / 2);
    let second_chime = command_chime(START, UNIX + 6, 0.1);
    let mut seed = 11;
    let mut x = noise(13 * SECOND, 1e-4, &mut seed);
    add_at(&mut x, SECOND, &tone);
    // As late as the page allows: after the longest chime of the first play.
    add_at(&mut x, SECOND + tone.len() + 2 * SECOND, &first_chime);
    let second_tone_at = SECOND + tone.len() + 5 * SECOND;
    add_at(&mut x, second_tone_at, &tone);
    add_at(&mut x, second_tone_at + tone.len() + 2 * SECOND, &second_chime);
    assert_eq!(run(&mut app, &x), [RemoteCommand::Start]);
}

#[test]
fn the_screen_text_fits() {
    for (line, width, room) in pages_lines() {
        assert!(width <= room, "{line}: {width} px in {room} px");
    }
}

/// A command chime also gives the host the phone's time, with a clock set when the setting is on
/// (the default), and without one when it is off.
#[test]
fn a_command_chime_gives_the_phone_time() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    assert!(app.clock_from_remote());
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RemoteCommand::Start]);
    let time = app.take_phone_time().expect("the chime's time");
    assert!(time.set_clock);
    assert_eq!(app.take_phone_time(), None);

    app.open_settings_page(crate::SettingsPage::Record);
    let row = layout::control_row(Setting::ClockFromRemote).center();
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.clock_from_remote());
    app.button(Button::Home);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 20, 0.1, 1e-4)), [RemoteCommand::Stop]);
    assert!(!app.take_phone_time().expect("the chime's time").set_clock);
}
