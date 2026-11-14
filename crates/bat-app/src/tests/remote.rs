//! The phone remote: what wakes the decoders, what they read, and what they ignore.

use std::boxed::Box;

use super::*;
use crate::screen::pages_lines;
use crate::{RecorderCommand, RemoteResult, Screen};

use chime::encode as chime_gen;

const SECOND: usize = FS as usize;
const UNIX: u32 = 1_793_000_000;
/// What the phone page puts in the deployment ID, as the decoder gives it.
const START: [u8; 8] = *b"BATX\x01\x01\x00\x00";
const STOP: [u8; 8] = *b"BATX\x01\x02\x00\x00";
const SET_CLOCK: [u8; 8] = *b"BATX\x01\x03\x00\x00";
/// Listening mode 3, FD.
const LISTEN_FD: [u8; 8] = *b"BATX\x01\x04\x03\x00";
/// Listening mode 5, PITCH, which the X8 does not offer.
const LISTEN_PITCH: [u8; 8] = *b"BATX\x01\x04\x05\x00";

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

fn run(app: &mut BatApp, x: &[f32]) -> Vec<RecorderCommand> {
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    let mut commands = Vec::new();
    for block in x.chunks(1024) {
        app.process(block, block, &mut l[..block.len()], &mut r[..block.len()]);
        commands.extend(app.take_remote_command());
    }
    commands
}

/// An app with the phone remote on and the phone code not required, as it is by default.
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
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert_eq!(app.remote.tones, 1);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Stop]);
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
    assert_eq!(run(&mut app, &x), [RecorderCommand::Stop]);
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
    // Another recorder's deployment ID.
    assert_eq!(run(&mut app, &press(*b"AUDIOMOT", UNIX, 0.1, 1e-4)), []);
    assert_eq!(app.remote.chimes_read, 1);
    assert!(app.remote.last.is_none());
    assert_eq!(app.remote.result_shown(app.samples), None);
}

#[test]
fn an_unknown_command_is_reported_once() {
    // An unknown command, a version this build does not know, a known command with an argument, and
    // a listening mode this build does not know.
    for id in [*b"BATX\x01\x05\x00\x00", *b"BATX\x02\x01\x00\x00", *b"BATX\x01\x01\x05\x00", *b"BATX\x01\x04\x07\x00"] {
        let mut storage = vec![0.0f32; SECOND];
        let mut buffers = Box::new(Buffers::new());
        let mut app = remote_app(&mut storage, &mut buffers);
        assert_eq!(run(&mut app, &press(id, UNIX, 0.1, 1e-4)), [], "{id:?}");
        assert_eq!(app.remote.last.map(|last| last.message.deployment_id()), Some(id));
        assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::Unknown));
    }
}

#[test]
fn the_same_chime_on_both_inputs_gives_one_command() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), Some(InputName { short: "4", long: "Input 4" })]);
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
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
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    app.open_chime_test();
    run(&mut app, &press(START, UNIX + 10, 0.1, 1e-4));
    app.open_settings_page(crate::SettingsPage::Record);
    assert_eq!(app.screen(), Screen::Settings);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 20, 0.1, 1e-4)), [RecorderCommand::Stop]);
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
    app.remote_result(RemoteResult::HoldOn(RecorderCommand::Start));
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
    assert_eq!(run(&mut app, &x), [RecorderCommand::Start]);
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
    assert_eq!(run(&mut app, &press(START, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    let time = app.take_phone_time().expect("the chime's time");
    assert!(time.set_clock);
    assert_eq!(app.take_phone_time(), None);

    app.open_settings_page(crate::SettingsPage::Phone);
    let row = layout::control_row(Setting::ClockFromRemote).center();
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.clock_from_remote());
    app.button(Button::Home);
    assert_eq!(run(&mut app, &press(STOP, UNIX + 20, 0.1, 1e-4)), [RecorderCommand::Stop]);
    assert!(!app.take_phone_time().expect("the chime's time").set_clock);
}

/// A clock command sets the clock even with the setting for other phone chimes off, and its result
/// beeps once the host says how the set went.
#[test]
fn a_clock_command_sets_the_clock() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.open_settings_page(crate::SettingsPage::Phone);
    let row = layout::control_row(Setting::ClockFromRemote).center();
    app.touch(row.x, row.y);
    assert!(!app.clock_from_remote());
    app.button(Button::Home);
    assert_eq!(run(&mut app, &press(SET_CLOCK, UNIX, 0.1, 1e-4)), []);
    assert!(app.take_phone_time().expect("the chime's time").set_clock);
    assert_eq!(app.remote.result_shown(app.samples), None);
    app.set_clock_state(crate::ClockState::Setting);
    assert_eq!(app.remote.result_shown(app.samples), None);
    app.set_clock_state(crate::ClockState::Set(0));
    assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::ClockSet));
    assert!(app.remote.beeping());
    // A later set, from the clock screen, is not the phone command's.
    assert!(!app.remote.clock_pending);
}

/// A listening mode command changes the mode when the host offers it, and reports one it does not.
#[test]
fn a_listening_mode_command_changes_the_mode() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.set_modes(&crate::Mode::WITHOUT_PITCH);
    assert_eq!(app.mode, crate::Mode::Het);
    assert_eq!(run(&mut app, &press(LISTEN_FD, UNIX, 0.1, 1e-4)), []);
    assert_eq!(app.mode, crate::Mode::Fd);
    assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::ModeSet(crate::Mode::Fd)));
    assert_eq!(run(&mut app, &press(LISTEN_PITCH, UNIX + 20, 0.1, 1e-4)), []);
    assert_eq!(app.mode, crate::Mode::Fd);
    assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::ModeNotOffered(crate::Mode::Pitch)));
}

/// The code on the Phone page in these tests.
const CODE: [u8; 6] = *b"493817";

/// The deployment ID of a version 2 command made with `code`, for a chime that `press` makes
/// at `unix`.
fn coded(command: crate::RemoteCommand, code: &[u8; 6], unix: u32) -> [u8; 8] {
    command.coded_deployment_id(code, unix, 600)
}

/// An app with the phone remote on and the phone code required.
fn coded_app<'a>(storage: &'a mut [f32], buffers: &'a mut Buffers) -> BatApp<'a> {
    let mut app = remote_app(storage, buffers);
    app.set_phone_code(CODE);
    app.set_phone_code_required(true);
    app
}

#[test]
fn the_code_is_not_required_by_default() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(FS, &mut storage, &mut buffers);
    assert!(!app.phone_code_required());
    app.set_remote_available(true);
    app.set_remote_on(true);
    assert_eq!(app.phone_code(), None);
    assert_eq!(app.remote.code_shown(), None);
}

#[test]
fn a_command_made_with_the_code_is_acted_on() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    let start = coded(crate::RemoteCommand::Start, &CODE, UNIX);
    assert_eq!(run(&mut app, &press(start, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.take_phone_time().expect("the chime's time").set_clock);
    let stop = coded(crate::RemoteCommand::Stop, &CODE, UNIX + 10);
    assert_eq!(run(&mut app, &press(stop, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Stop]);
    let fd = coded(crate::RemoteCommand::Listen(crate::ListeningMode::Fd), &CODE, UNIX + 20);
    assert_eq!(run(&mut app, &press(fd, UNIX + 20, 0.1, 1e-4)), []);
    assert_eq!(app.mode, crate::Mode::Fd);
    let clock = coded(crate::RemoteCommand::SetClock, &CODE, UNIX + 30);
    assert_eq!(run(&mut app, &press(clock, UNIX + 30, 0.1, 1e-4)), []);
    assert!(app.remote.clock_pending);
}

#[test]
fn a_version_1_command_is_refused_while_the_code_is_required() {
    for id in [START, SET_CLOCK, LISTEN_FD] {
        let mut storage = vec![0.0f32; SECOND];
        let mut buffers = Box::new(Buffers::new());
        let mut app = coded_app(&mut storage, &mut buffers);
        assert_eq!(run(&mut app, &press(id, UNIX, 0.1, 1e-4)), [], "{id:?}");
        assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::CodeNeeded));
        assert_eq!(app.take_phone_time(), None);
        assert!(!app.remote.clock_pending);
        assert_eq!(app.mode, crate::Mode::Het);
    }
}

#[test]
fn a_command_made_with_another_code_is_refused() {
    for command in [crate::RemoteCommand::Start, crate::RemoteCommand::SetClock] {
        let mut storage = vec![0.0f32; SECOND];
        let mut buffers = Box::new(Buffers::new());
        let mut app = coded_app(&mut storage, &mut buffers);
        let id = coded(command, b"493818", UNIX);
        assert_eq!(run(&mut app, &press(id, UNIX, 0.1, 1e-4)), []);
        assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::CodeWrong));
        assert_eq!(app.take_phone_time(), None);
        assert!(!app.remote.clock_pending);
        // The ID of one chime in a chime with another time: a recording sent again with a
        // new time.
        let id = coded(command, &CODE, UNIX + 20);
        assert_eq!(run(&mut app, &press(id, UNIX + 30, 0.1, 1e-4)), []);
        assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::CodeWrong));
        assert_eq!(app.take_phone_time(), None);
    }
}

#[test]
fn a_command_no_later_than_the_last_one_acted_on_is_refused() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    let start = coded(crate::RemoteCommand::Start, &CODE, UNIX + 10);
    assert_eq!(run(&mut app, &press(start, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.take_phone_time().is_some());
    // An earlier command, and another command with the same time.
    for unix in [UNIX, UNIX + 10] {
        let stop = coded(crate::RemoteCommand::Stop, &CODE, unix);
        assert_eq!(run(&mut app, &press(stop, unix, 0.1, 1e-4)), [], "{unix}");
        assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::CodeOld));
        assert_eq!(app.take_phone_time(), None);
    }
    // The recording of the first command, played again after the others.
    assert_eq!(run(&mut app, &press(start, UNIX + 10, 0.1, 1e-4)), []);
    assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::CodeOld));
    // A later command is acted on.
    let stop = coded(crate::RemoteCommand::Stop, &CODE, UNIX + 11);
    assert_eq!(run(&mut app, &press(stop, UNIX + 11, 0.1, 1e-4)), [RecorderCommand::Stop]);
    // A refused command does not count as the last one: a wrong code with a later time, then
    // the right code with a time between the two.
    let wrong = coded(crate::RemoteCommand::Start, b"000000", UNIX + 50);
    assert_eq!(run(&mut app, &press(wrong, UNIX + 50, 0.1, 1e-4)), []);
    let start = coded(crate::RemoteCommand::Start, &CODE, UNIX + 40);
    assert_eq!(run(&mut app, &press(start, UNIX + 40, 0.1, 1e-4)), [RecorderCommand::Start]);
}

#[test]
fn the_same_coded_chime_on_both_inputs_gives_one_command() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), Some(InputName { short: "4", long: "Input 4" })]);
    let start = coded(crate::RemoteCommand::Start, &CODE, UNIX);
    assert_eq!(run(&mut app, &press(start, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert_eq!(app.remote.chimes_read, 2);
    // The second reading is not reported as an old command.
    assert_eq!(app.remote.result_shown(app.samples), None);
}

#[test]
fn with_the_code_not_required_any_check_is_accepted() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    app.open_settings_page(crate::SettingsPage::Phone);
    let row = layout::control_row(Setting::PhoneCode).center();
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.phone_code_required());
    app.button(Button::Home);
    let start = coded(crate::RemoteCommand::Start, b"000000", UNIX + 10);
    assert_eq!(run(&mut app, &press(start, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.take_phone_time().is_some());
    // An earlier time is accepted too, and so is version 1.
    let stop = coded(crate::RemoteCommand::Stop, &CODE, UNIX);
    assert_eq!(run(&mut app, &press(stop, UNIX, 0.1, 1e-4)), [RecorderCommand::Stop]);
    assert_eq!(run(&mut app, &press(START, UNIX + 20, 0.1, 1e-4)), [RecorderCommand::Start]);
}

/// With the code required, only a command made with the code gives the phone's time. A plain
/// AudioMoth chime after a tone, another recorder's deployment ID and an unknown command give
/// none, although Set clock from all chimes is on.
#[test]
fn only_a_command_made_with_the_code_gives_the_phone_time() {
    let unknown_with_the_code = {
        let mut id = *b"BATX\x02\x50\x00\x00";
        let check = chime::format::remote_check(&CODE, id[5], UNIX, 600);
        id[6..].copy_from_slice(&check);
        id
    };
    let cases: [(Option<[u8; 8]>, bool, Option<RemoteResult>); 6] = [
        (None, false, None),
        (Some(*b"AUDIOMOT"), false, None),
        // An unknown command in version 1, a version this build does not know, and an unknown
        // command in version 2 with a check made from another code.
        (Some(*b"BATX\x01\x05\x00\x00"), false, Some(RemoteResult::Unknown)),
        (Some(*b"BATX\x03\x10\x00\x00"), false, Some(RemoteResult::Unknown)),
        (Some(*b"BATX\x02\x50\x00\x00"), false, Some(RemoteResult::Unknown)),
        // An unknown command made with the code: its time is used, as any accepted chime's is.
        (Some(unknown_with_the_code), true, Some(RemoteResult::Unknown)),
    ];
    for (id, gives_time, result) in cases {
        for required in [true, false] {
            let mut storage = vec![0.0f32; SECOND];
            let mut buffers = Box::new(Buffers::new());
            let mut app = coded_app(&mut storage, &mut buffers);
            app.set_phone_code_required(required);
            assert!(app.clock_from_remote());
            let mut bytes = chime_gen::time_payload(UNIX, 600);
            bytes.extend(id.iter().flatten());
            let render = chime_gen::render(FS as u32, &bytes, chime_gen::tune_for_payload(bytes.len()));
            let chime: Vec<f32> = render.mix().iter().map(|&v| v as f32 * 0.1).collect();
            let tone = page_tone(500, 0.1);
            let mut seed = 5;
            let mut x = noise(SECOND + tone.len() + SECOND + chime.len() + SECOND, 1e-4, &mut seed);
            add_at(&mut x, SECOND, &tone);
            add_at(&mut x, SECOND + tone.len() + SECOND, &chime);
            assert_eq!(run(&mut app, &x), [], "{id:?}");
            assert_eq!(app.remote.chimes_read, 1, "{id:?}");
            assert_eq!(app.take_phone_time().is_some(), gives_time || !required, "{id:?} {required}");
            assert_eq!(app.remote.result_shown(app.samples), result, "{id:?} {required}");
        }
    }
}

/// The code is made when the Phone code switch is first turned on, from the input heard until
/// then, and is kept when the switch is turned off and on again.
#[test]
fn the_code_is_made_once_from_the_input() {
    let mut codes = Vec::new();
    for seed in [1u32, 2, 3] {
        let mut storage = vec![0.0f32; SECOND];
        let mut buffers = Box::new(Buffers::new());
        let mut app = BatApp::new(FS, &mut storage, &mut buffers);
        app.set_remote_available(true);
        app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), None]);
        let mut seed = seed;
        run(&mut app, &noise(SECOND / 4, 1e-4, &mut seed));
        assert_eq!(app.phone_code(), None);
        // The switch on the Phone page.
        app.open_settings_page(crate::SettingsPage::Phone);
        let row = layout::control_row(Setting::PhoneCode).center();
        app.touch(row.x, row.y);
        assert!(app.phone_code_required());
        let code = app.phone_code().expect("a code");
        assert!(code.iter().all(|c| c.is_ascii_digit()), "{code:?}");
        app.touch(row.x, row.y);
        run(&mut app, &noise(SECOND / 4, 1e-4, &mut seed));
        app.touch(row.x, row.y);
        assert!(app.phone_code_required());
        assert_eq!(app.phone_code(), Some(code));
        codes.push(code);
    }
    assert!(codes[0] != codes[1] && codes[1] != codes[2] && codes[0] != codes[2], "{codes:?}");
}

/// The Phone page shows the code only while the Phone code switch is on.
#[test]
fn the_phone_page_shows_the_code_while_it_is_in_force() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    assert_eq!(app.remote.code_shown(), Some(CODE));
    app.set_phone_code_required(false);
    assert_eq!(app.remote.code_shown(), None);
    app.set_phone_code_required(true);
    app.set_remote_on(false);
    assert_eq!(app.remote.code_shown(), Some(CODE));
}

/// Start recording can set the Mute while recording switch: argument 1 turns it on, 2 turns it
/// off, and 0 leaves it as it is. It is set when the X8 is already recording too.
#[test]
fn a_start_command_can_set_mute_while_recording() {
    const START_MUTED: [u8; 8] = *b"BATX\x01\x01\x01\x00";
    const START_UNMUTED: [u8; 8] = *b"BATX\x01\x01\x02\x00";
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    assert!(!app.mute_while_recording());
    assert_eq!(run(&mut app, &press(START_MUTED, UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.mute_while_recording());
    // Argument 0 leaves the switch on, and off after the next command turns it off.
    assert_eq!(run(&mut app, &press(START, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.mute_while_recording());
    app.set_recorder(crate::Recorder::Recording, 0);
    assert_eq!(run(&mut app, &press(START_UNMUTED, UNIX + 20, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(!app.mute_while_recording());
    assert_eq!(run(&mut app, &press(START, UNIX + 30, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(!app.mute_while_recording());
    // Already recording, with the switch off: it is turned on.
    assert_eq!(run(&mut app, &press(START_MUTED, UNIX + 40, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.mute_while_recording());
    // Any other argument is unknown, and changes nothing.
    assert_eq!(run(&mut app, &press(*b"BATX\x01\x01\x03\x00", UNIX + 50, 0.1, 1e-4)), []);
    assert_eq!(app.remote.result_shown(app.samples), Some(RemoteResult::Unknown));
    assert!(app.mute_while_recording());
}

/// In version 2 the choice is in byte 5, and a refused command does not set the switch.
#[test]
fn a_coded_start_command_sets_mute_only_with_the_code() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = coded_app(&mut storage, &mut buffers);
    let wrong = coded(crate::RemoteCommand::StartWithMute(true), b"000000", UNIX);
    assert_eq!(wrong[5], 0x11);
    assert_eq!(run(&mut app, &press(wrong, UNIX, 0.1, 1e-4)), []);
    assert!(!app.mute_while_recording());
    let right = coded(crate::RemoteCommand::StartWithMute(true), &CODE, UNIX + 10);
    assert_eq!(run(&mut app, &press(right, UNIX + 10, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(app.mute_while_recording());
    let off = coded(crate::RemoteCommand::StartWithMute(false), &CODE, UNIX + 20);
    assert_eq!(off[5], 0x12);
    assert_eq!(run(&mut app, &press(off, UNIX + 20, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(!app.mute_while_recording());
}

/// BAT PLAYBACK has no Mute while recording switch: the choice is ignored and the command passed on.
#[test]
fn a_host_without_the_mute_switch_ignores_the_choice() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = remote_app(&mut storage, &mut buffers);
    app.set_input_kind(crate::InputKind::Playback);
    assert_eq!(run(&mut app, &press(*b"BATX\x01\x01\x01\x00", UNIX, 0.1, 1e-4)), [RecorderCommand::Start]);
    assert!(!app.mute_while_recording());
}
