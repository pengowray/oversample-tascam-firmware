use std::vec;
use std::vec::Vec;

use super::*;
use bat_dsp::spectrum::SpectrumAnalyzer;

mod chime_test;
mod cpu_test;
mod remote;
mod stereo;

const FS: f32 = 192_000.0;

fn chirp(f0: f32, f1: f32, amp: f32, n: usize) -> Vec<f32> {
    let dur = n as f64 / FS as f64;
    let k = (f1 as f64 - f0 as f64) / dur;
    (0..n)
        .map(|i| {
            let t = i as f64 / FS as f64;
            let w = (core::f64::consts::PI * i as f64 / n as f64).sin();
            (amp as f64 * w * w * (core::f64::consts::TAU * (f0 as f64 * t + 0.5 * k * t * t)).sin()) as f32
        })
        .collect()
}

fn noise(amp: f32, n: usize) -> Vec<f32> {
    let mut s = 12345u32;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            amp * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

/// One second of noise with a train of three 5 ms calls, 80 ms apart from
/// 0.5 s, on both inputs. A single call is not shown: it takes a train.
fn signal() -> Vec<f32> {
    let mut x = noise(0.003, FS as usize);
    for k in 0..3 {
        let at = 96_000 + k * 15_360;
        for (v, c) in x[at..].iter_mut().zip(chirp(80_000.0, 40_000.0, 0.1, 960)) {
            *v += c;
        }
    }
    x
}

/// The app's large buffers, kept for the life of the test.
fn buffers() -> &'static mut Buffers {
    std::boxed::Box::leak(std::boxed::Box::new(Buffers::new()))
}

fn run(app: &mut BatApp, x: &[f32], block: usize) -> Vec<f32> {
    let mut out = vec![0.0; x.len()];
    let mut out_r = vec![0.0; x.len()];
    for ((xi, ol), or) in x.chunks(block).zip(out.chunks_mut(block)).zip(out_r.chunks_mut(block)) {
        app.process(xi, xi, ol, or);
    }
    out
}

fn draw(app: &BatApp) -> Vec<u16> {
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    app.render(&mut Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT));
    pixels
}

#[test]
fn finds_a_call_and_draws_every_mode_without_panicking() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    run(&mut app, &signal(), 4096);
    let call = app.last_call().expect("the call was not detected");
    assert!((40_000.0..80_000.0).contains(&call.peak_freq_hz), "{call:?}");
    for mode in Mode::ALL {
        app.set_mode(mode);
        let pixels = draw(&app);
        assert!(pixels.iter().any(|&p| p != 0));
    }
}

#[test]
fn wheel_and_taps_change_the_heterodyne_frequency() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    assert_eq!(app.het_lo_hz(), 45_000.0);
    app.wheel(4);
    assert_eq!(app.het_lo_hz(), 47_000.0);
    app.wheel(-1000);
    assert_eq!(app.het_lo_hz(), 15_000.0);
    // A tap at the vertical middle of the waterfall (0 to 96 kHz) is about 48 kHz.
    app.touch(layout::WF_X + 10, layout::WF_Y + layout::WF_H as i32 / 2);
    assert!((app.het_lo_hz() - 48_000.0).abs() <= 500.0, "{}", app.het_lo_hz());
}

#[test]
fn tapping_a_mode_button_selects_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let r = layout::mode_button(3, Mode::ALL.len());
    app.touch(r.center().x, r.center().y);
    assert_eq!(app.mode(), Mode::Te);
}

#[test]
fn time_expansion_starts_on_a_call_and_plays() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(Mode::Te);
    // Play the call, then enough quiet for capture to finish.
    let mut x = signal();
    x.extend(noise(0.003, FS as usize / 2));
    let y = run(&mut app, &x, 1000);
    assert!(matches!(app.te_status(), TeStatus::Playing { .. }), "{:?}", app.te_status());
    assert!(y.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn rec_toggles_and_counts_recorded_time() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.button(Button::Rec);
    assert!(app.is_recording());
    run(&mut app, &noise(0.001, 2 * FS as usize), 4096);
    assert_eq!(app.recorded_seconds(), 2);
    app.button(Button::Rec);
    assert!(!app.is_recording());
}

#[test]
fn without_pitch_the_buttons_fill_the_row_and_next_mode_cycles_them() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(Mode::Pitch);
    app.set_modes(&Mode::WITHOUT_PITCH);
    assert_eq!(app.mode(), Mode::Het);
    let last = layout::mode_button(3, 5);
    app.touch(last.center().x, last.center().y);
    assert_eq!(app.mode(), Mode::Te);
    let end = layout::mode_button(4, 5);
    assert!(end.top_left.x + end.size.width as i32 > 300);
    app.next_mode();
    assert_eq!(app.mode(), Mode::Thru);
    app.next_mode();
    assert_eq!(app.mode(), Mode::Het);
}

#[test]
fn dragging_on_the_waterfall_tunes_het() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let wf = layout::waterfall();
    let (x, top) = (wf.center().x, wf.top_left.y);
    assert!(app.is_draggable(x, top + 10));
    app.touch(x, top + 10);
    let high = app.het_lo_hz();
    app.drag(x, top + 60);
    assert!(app.het_lo_hz() < high - 10_000.0, "{high} -> {}", app.het_lo_hz());
    app.set_mode(Mode::Thru);
    assert!(!app.is_draggable(x, top + 10));
}

#[test]
fn an_external_recorder_replaces_rec() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_recorder(Recorder::Standby, 0);
    app.button(Button::Rec);
    assert_eq!(app.recorder(), (Recorder::Standby, 0));
    app.set_recorder(Recorder::Recording, 75);
    assert!(app.is_recording());
    assert_eq!(app.recorder(), (Recorder::Recording, 75));
}

#[test]
fn listen_to_has_nothing_to_choose_with_one_input() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let name = InputName { short: "5", long: "Input 5" };
    app.set_inputs([Some(name), None]);
    assert_eq!(app.inputs(), [Some(name), None]);
    let before = app.source();
    app.change_setting(Setting::ListenTo, 1);
    assert_eq!(app.source(), before);
    assert!(draw(&app).iter().any(|&p| p != 0));
    app.open_settings();
    assert!(draw(&app).iter().any(|&p| p != 0));
    app.set_inputs([Some(name), Some(InputName { short: "6", long: "Input 6" })]);
    app.change_setting(Setting::ListenTo, 1);
    assert_ne!(app.source(), before);
}

#[test]
fn listen_to_steps_through_the_mix_and_then_the_inputs() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    assert_eq!(app.feed(), Feed::Inputs);
    app.set_mix_available(true);
    app.set_feed(Feed::Mix);
    let mut seen = vec![(app.feed(), app.source())];
    for _ in 0..7 {
        app.change_setting(Setting::ListenTo, 1);
        assert!(draw(&app).iter().any(|&p| p != 0));
        seen.push((app.feed(), app.source()));
    }
    use MonitorSource::{Both, First, Second};
    assert_eq!(
        seen,
        [
            (Feed::Mix, Both),
            (Feed::Mix, First),
            (Feed::Mix, Second),
            (Feed::Inputs, Both),
            (Feed::Inputs, First),
            (Feed::Inputs, Second),
            (Feed::Inputs, Second),
            (Feed::Inputs, Second),
        ]
    );
    // Without the mix, as in BAT PLAYBACK, only the inputs remain.
    app.set_mix_available(false);
    assert_eq!(app.feed(), Feed::Inputs);
    app.change_setting(Setting::ListenTo, -5);
    assert_eq!((app.feed(), app.source()), (Feed::Inputs, Both));
}

#[test]
fn listen_to_in_playback_leaves_the_live_choice_alone() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let name = |short, long| Some(InputName { short, long });
    app.set_mix_available(true);
    app.set_feed(Feed::Mix);
    app.set_input_kind(InputKind::Playback);
    app.set_mix_available(false);
    app.set_inputs([name("L", "Left"), name("R", "Right")]);
    app.change_setting(Setting::ListenTo, 1);
    assert_eq!((app.feed(), app.source()), (Feed::Inputs, MonitorSource::First));
    app.set_input_kind(InputKind::Live);
    app.set_mix_available(true);
    app.set_inputs([name("3", "Input 3"), name("4", "Input 4")]);
    assert_eq!((app.feed(), app.source()), (Feed::Mix, MonitorSource::Both));
    // The playback choice is still there on the way back.
    app.set_input_kind(InputKind::Playback);
    assert_eq!(app.source(), MonitorSource::First);
}

#[test]
fn listen_to_steps_from_what_is_shown_when_inputs_change() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    let input4 = Some(InputName { short: "4", long: "Input 4" });
    app.set_mix_available(true);
    app.set_feed(Feed::Inputs);
    app.set_source(MonitorSource::Both);
    // Input 4 turned off: the screen shows Input 3, the last choice.
    app.set_inputs([input3, None]);
    app.change_setting(Setting::ListenTo, 1);
    assert_eq!((app.feed(), app.source()), (Feed::Inputs, MonitorSource::Both));
    app.set_inputs([input3, input4]);
    assert_eq!((app.feed(), app.source()), (Feed::Inputs, MonitorSource::Both));
    app.set_inputs([input3, None]);
    app.change_setting(Setting::ListenTo, -1);
    assert_eq!((app.feed(), app.source()), (Feed::Mix, MonitorSource::Second));
    // No input on, with the inputs still chosen: plus stays, minus reaches the mix.
    app.set_feed(Feed::Inputs);
    app.set_source(MonitorSource::Both);
    app.set_inputs([None, None]);
    app.change_setting(Setting::ListenTo, 1);
    assert_eq!(app.feed(), Feed::Inputs);
    app.change_setting(Setting::ListenTo, -1);
    assert_eq!((app.feed(), app.source()), (Feed::Mix, MonitorSource::Second));
}

#[test]
fn hidden_controls_are_not_tappable() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_settings_page(SettingsPage::Tests);
    let debug = layout::control_row(Setting::DebugInfo).center();
    assert!(!app.is_tappable(debug.x, debug.y));
    app.set_debug_available(true);
    assert!(app.is_tappable(debug.x, debug.y));
    app.open_settings_page(SettingsPage::Listen);
    let minus = layout::setting_minus(1).center();
    assert!(app.is_tappable(minus.x, minus.y));
    app.set_inputs([Some(InputName { short: "5", long: "Input 5" }), None]);
    assert!(!app.is_tappable(minus.x, minus.y));
}

/// Noise of amplitude `noise_amp`, with a burst of a 1 kHz tone of amplitude `amp`, 5 ms
/// long, every 100 ms: the pattern of the calls of a pass. A steady sound counts as
/// background, so calls have to be short.
fn bursts(noise_amp: f32, amp: f32, seconds: f32) -> Vec<f32> {
    let mut x = noise(noise_amp, (seconds * FS) as usize);
    for (i, v) in x.iter_mut().enumerate() {
        if i % ms(100.0) < ms(5.0) {
            *v += amp * (i as f32 * core::f32::consts::TAU * 1000.0 / FS).sin();
        }
    }
    x
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

/// `auto` on `x`, in the mode that leaves the floor in use, and the output's second half.
fn second_half(auto: &mut level::AutoLevel, mut x: Vec<f32>) -> Vec<f32> {
    auto.apply(&mut x, Mode::Het, true);
    x.split_off(x.len() / 2)
}

/// Where the X8 aims: -12 dBFS at the headphones, before `OUTPUT_GAIN` of +18 dB.
const X8_TARGET: f32 = 0.25 / 8.0;
/// How far below the target steady noise comes out.
const NOISE_BELOW_TARGET_DB: f32 = 36.0;

#[test]
fn auto_level_holds_its_limits_and_timing() {
    let mut auto = level::AutoLevel::new(FS);
    // Off, it changes nothing.
    assert!((peak(&second_half(&mut auto, bursts(0.0, 0.1, 0.2))) - 0.1).abs() < 0.001);
    auto.set_volume(AutoVolume::On);
    assert_eq!(auto.volume(), AutoVolume::On);
    // Calls far above the noise settle on the target.
    let settled = peak(&second_half(&mut auto, bursts(0.0001, 0.05, 6.0)));
    assert!((settled - 0.25).abs() < 0.02, "{settled}");
    // After the sound stops, the gain rises about 6 dB per second.
    let before = auto.gain();
    let mut silence = vec![0.0f32; (2.0 * FS) as usize];
    auto.apply(&mut silence, Mode::Het, true);
    let rise_db = 20.0 * (auto.gain() / before).log10();
    assert!(rise_db > 8.0 && rise_db < 14.0, "{rise_db}");
    // The upper limit is +24 dB, for calls with a floor too low to reach the ceiling.
    let quiet = peak(&second_half(&mut auto, bursts(0.00001, 0.001, 12.0)));
    assert!((quiet - 0.001 * 15.85).abs() < 0.0005, "{quiet}");
    // The lower limit is -24 dB, reached with the X8's target.
    auto.set_target(X8_TARGET);
    let loud = peak(&second_half(&mut auto, bursts(0.0, 1.0, 4.0)));
    assert!((loud - 0.063).abs() < 0.004, "{loud}");
}

#[test]
fn auto_level_applies_in_each_mode_and_only_to_the_output() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    // Calls at 46 kHz, heard at 1 kHz in HET.
    let mut input = noise(0.0001, FS as usize);
    for (i, v) in input.iter_mut().enumerate() {
        if i % ms(100.0) < ms(5.0) {
            *v += 0.05 * (i as f32 * core::f32::consts::TAU * 46_000.0 / FS).sin();
        }
    }
    for mode in [Mode::Thru, Mode::Het] {
        app.set_mode(mode);
        let loudest = |app: &mut BatApp| {
            let (mut l, mut r) = (vec![0.0; input.len()], vec![0.0; input.len()]);
            app.process(&input, &input, &mut l, &mut r);
            peak(&l[l.len() / 2..])
        };
        let fixed = loudest(&mut app);
        let meter_before = app.meters[0].level_db;
        app.change_setting(Setting::AutoLevel, 1);
        loudest(&mut app);
        let automatic = loudest(&mut app);
        assert_eq!(app.auto_volume(), AutoVolume::On);
        assert!(automatic > fixed * 1.5, "{mode:?} {fixed} {automatic}");
        assert!((app.meters[0].level_db - meter_before).abs() < 1.0);
        app.change_setting(Setting::AutoLevel, -1);
        assert_eq!(app.auto_volume(), AutoVolume::Off);
    }
}

#[test]
fn steady_noise_and_tones_come_out_at_the_ceiling_at_any_level() {
    let tone = |n: usize, amp: f32| -> Vec<f32> { (0..n).map(|i| amp * (i as f32 * core::f32::consts::TAU * 1000.0 / FS).sin()).collect() };
    let ten_seconds = (10.0 * FS) as usize;
    for target in [0.25, X8_TARGET] {
        let ceiling = db(target) - NOISE_BELOW_TARGET_DB;
        // From a gain of about +14 dB to about -20 dB.
        for level_above_ceiling_db in [-14.0, 0.0, 20.0] {
            let amp = 10f32.powf((ceiling + level_above_ceiling_db) / 20.0);
            let inputs = [noise(amp * 3f32.sqrt(), ten_seconds), tone(ten_seconds, amp * 2f32.sqrt())];
            for input in inputs {
                let mut auto = level::AutoLevel::new(FS);
                auto.set_target(target);
                auto.set_volume(AutoVolume::On);
                let out = second_half(&mut auto, input);
                let error = db(rms(&out[out.len() / 2..])) - ceiling;
                assert!(error.abs() < 1.5, "{target} {level_above_ceiling_db}: {error} dB from the ceiling");
            }
        }
    }
}

#[test]
fn short_calls_do_not_raise_the_floor() {
    let hiss_only = noise(0.001, (10.0 * FS) as usize);
    let mut with_calls = hiss_only.clone();
    // 5 ms every 100 ms, 40 dB above the hiss.
    for (i, v) in with_calls.iter_mut().enumerate() {
        if i % ms(100.0) < ms(5.0) {
            *v += 0.1 * (i as f32 * core::f32::consts::TAU * 1000.0 / FS).sin();
        }
    }
    let floor_after = |mut x: Vec<f32>| {
        let mut auto = level::AutoLevel::new(FS);
        auto.set_volume(AutoVolume::On);
        auto.apply(&mut x, Mode::Het, true);
        auto.floor_db().unwrap()
    };
    let raised = floor_after(with_calls) - floor_after(hiss_only);
    assert!(raised.abs() < 2.0, "{raised} dB");
}

#[test]
fn a_loud_sound_reaches_the_target_and_a_sound_near_the_floor_stays_low() {
    let target = 0.25;
    let noise_amp = 0.01;
    // Noise RMS is -45 dBFS, close to the ceiling for this target (-48 dBFS).
    let loud_call = 1.0; // 45 dB above the noise.
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    let out = second_half(&mut auto, bursts(noise_amp, loud_call, 8.0));
    assert!((peak(&out) - target).abs() < 0.03, "{}", peak(&out));
    let near_floor = 0.02; // 12 dB above the noise's RMS.
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    let out = second_half(&mut auto, bursts(noise_amp, near_floor, 8.0));
    assert!(peak(&out) < target * 0.2, "{}", peak(&out));
}

#[test]
fn time_expansion_calls_are_not_taken_for_a_noise_floor() {
    // Playback after a silence: mostly low frequencies, in level, with a call in it.
    let mut x = vec![0.0f32; (20.0 * FS) as usize];
    for (i, v) in x.iter_mut().enumerate() {
        let p = i % ms(5000.0);
        if p < ms(3000.0) {
            *v = 0.005 * (i as f32 * core::f32::consts::TAU * 30.0 / FS).sin();
        }
        if p % ms(300.0) < ms(30.0) && p < ms(3000.0) {
            *v += 0.02 * (i as f32 * core::f32::consts::TAU * 4000.0 / FS).sin();
        }
    }
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    auto.apply(&mut x, Mode::Te, true);
    let settled = peak(&x[x.len() / 2..]);
    assert!(settled > 0.2, "{settled}");
}

#[test]
fn digital_silence_leaves_the_floor_where_it_was() {
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    second_half(&mut auto, noise(0.05, (6.0 * FS) as usize));
    let (floor, gain) = (auto.floor_db().unwrap(), auto.gain());
    // The mix muted by the X8, or time expansion between playbacks.
    auto.apply(&mut vec![0.0f32; (10.0 * FS) as usize], Mode::Het, true);
    assert_eq!(auto.floor_db().unwrap(), floor);
    assert!((auto.gain() / gain - 1.0).abs() < 0.01, "{}", auto.gain() / gain);
    // The noise comes back no louder.
    let mut back = noise(0.05, ms(50.0));
    auto.apply(&mut back, Mode::Het, true);
    assert!(rms(&back) < 10f32.powf((db(0.25) - NOISE_BELOW_TARGET_DB + 2.0) / 20.0), "{}", db(rms(&back)));
}

#[test]
fn a_very_quiet_input_then_real_noise_settles_in_seconds() {
    // Not digital silence, so the floor follows it down, as far as it needs to go.
    let mut auto = level::AutoLevel::new(FS);
    auto.set_target(X8_TARGET);
    auto.set_volume(AutoVolume::On);
    auto.apply(&mut noise(0.00001, (5.0 * FS) as usize), Mode::Het, true);
    // Then noise 40 dB above where the floor is held, as when an input comes back.
    let ceiling = db(X8_TARGET) - NOISE_BELOW_TARGET_DB;
    let mut x = noise(10f32.powf((ceiling + 16.0) / 20.0) * 3f32.sqrt(), (10.0 * FS) as usize);
    auto.apply(&mut x, Mode::Het, true);
    let settled_at = x.chunks(ms(100.0)).position(|block| db(rms(block)) < ceiling + 3.0).unwrap_or(usize::MAX);
    assert!(settled_at <= 60, "{settled_at}");
}

#[test]
fn the_first_window_sets_the_floor_without_a_burst_of_gain() {
    // Auto volume turned on in noise far above the ceiling: the gain goes down from
    // no change, and never up towards +24 dB.
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    let mut x = noise(0.05, ms(500.0));
    auto.apply(&mut x, Mode::Het, true);
    assert!(auto.gain() <= 1.0, "{}", auto.gain());
}

#[test]
fn a_change_of_noise_moves_the_gain_slowly() {
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    auto.apply(&mut noise(0.001, (6.0 * FS) as usize), Mode::Het, true);
    // Noise 20 dB louder arrives. Its peaks are below what the gain allows for, so
    // the gain stays on the slow path: no 10 ms window moves it more than 1 dB.
    let mut x = noise(0.01, (6.0 * FS) as usize);
    let mut last = auto.gain();
    for block in x.chunks_mut(ms(10.0)) {
        auto.apply(block, Mode::Het, true);
        assert!(db(auto.gain() / last).abs() < 1.0, "{}", db(auto.gain() / last));
        last = auto.gain();
    }
    // By the end, the noise is back near the ceiling.
    assert!(db(rms(&x[x.len() - ms(500.0)..])) < db(0.25) - NOISE_BELOW_TARGET_DB + 3.0);
}

#[test]
fn fd_calls_are_not_taken_for_a_noise_floor() {
    // FD's output is exact silence with clicks during calls. The floor would be the
    // level of the calls, and hold them down at the ceiling.
    let mut x = vec![0.0f32; (20.0 * FS) as usize];
    for (i, v) in x.iter_mut().enumerate() {
        if i % ms(100.0) < ms(20.0) {
            *v = 0.02 * (i as f32 * core::f32::consts::TAU * 1000.0 / FS).sin();
        }
    }
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    auto.apply(&mut x, Mode::Fd, true);
    let settled = peak(&x[x.len() / 2..]);
    assert!((settled - 0.25).abs() < 0.03, "{settled}");
}

/// Noise of amplitude 0.003, with calls from `from_s`, 100 ms apart, on 80 to 40 kHz.
fn train(seconds: f32, from_s: f32, calls: usize) -> Vec<f32> {
    let mut x = noise(0.003, (seconds * FS) as usize);
    for k in 0..calls {
        let at = ms(from_s * 1000.0 + k as f32 * 100.0);
        for (v, c) in x[at..].iter_mut().zip(chirp(80_000.0, 40_000.0, 0.1, 960)) {
            *v += c;
        }
    }
    x
}

/// Bats only starts with the gate open and fading shut, so it closes without a step
/// when it is chosen. By this many samples (250 ms), it has closed.
const AFTER_GATE_FIRST_CLOSES: usize = 48_000;

fn bats_only_app<'a>(storage: &'a mut [f32]) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers());
    app.set_auto_volume(AutoVolume::BatsOnly);
    app
}

#[test]
fn bats_only_opens_on_the_first_call_and_closes_after_the_hold() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let last_call = 1.0 + 7.0 * 0.1;
    let out = run(&mut app, &train(4.0, 1.0, 8), 1024);
    let at = |s: f32| ms(s * 1000.0);
    // Silent until the first call has been detected, which is at its end.
    assert!(out[at(0.3)..at(1.0)].iter().all(|&v| v == 0.0));
    // Open from just after the first call, through the calls and the gaps.
    assert!(peak(&out[at(1.02)..at(1.1)]) > 0.0);
    for k in 1..8 {
        let call = 1.0 + k as f32 * 0.1;
        assert!(peak(&out[at(call)..at(call + 0.01)]) > 0.1, "call {k}: {}", peak(&out[at(call)..at(call + 0.01)]));
    }
    // Open for the hold after the last call, then closed after the fade.
    assert!(peak(&out[at(last_call + 0.9)..at(last_call + 1.0)]) > 0.0);
    assert!(out[at(last_call + 1.3)..].iter().all(|&v| v == 0.0));
}

#[test]
fn bats_only_stays_shut_for_faint_calls_that_are_not_marked_as_bats() {
    // Faint enough that the detector finds them, and no pair makes a train.
    let mut x = noise(0.003, (3.0 * FS) as usize);
    for k in 0..12 {
        let at = ms(1000.0 + k as f32 * 100.0);
        for (v, c) in x[at..].iter_mut().zip(chirp(60_000.0, 50_000.0, 0.0025, 1_920)) {
            *v += c;
        }
    }
    let mut detector: bat_dsp::detector::CallDetector<512> = bat_dsp::detector::CallDetector::new(FS);
    let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(FS);
    analyzer.set_hop(FFT_HOP);
    let mut events = Vec::new();
    analyzer.push(&x, |col| events.extend(detector.process(col)));
    assert!(events.len() >= 6, "{}", events.len());
    assert!(events.iter().all(|e| e.peak_db_above_floor < DEFAULT_TRAIN_MIN_DB), "{events:?}");
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run(&mut app, &x, 1024);
    assert!(out[AFTER_GATE_FIRST_CLOSES..].iter().all(|&v| v == 0.0), "{}", peak(&out));
}

#[test]
fn bats_only_stays_shut_on_hiss_and_on_a_steady_tone() {
    let hiss = noise(0.003, (4.0 * FS) as usize);
    let mut tone = hiss.clone();
    for (i, v) in tone.iter_mut().enumerate().skip(ms(500.0)) {
        *v += 0.05 * (core::f32::consts::TAU * 46_000.0 * i as f32 / FS).sin();
    }
    for x in [hiss, tone] {
        let mut storage = vec![0.0f32; FS as usize];
        let mut app = bats_only_app(&mut storage);
        let out = run(&mut app, &x, 1024);
        assert!(out[AFTER_GATE_FIRST_CLOSES..].iter().all(|&v| v == 0.0), "{}", peak(&out));
        assert!(app.auto_volume_gate() == 0.0);
    }
}

#[test]
fn bats_only_closes_on_a_broken_tone_once_it_is_known_to_be_a_tone() {
    let mut x = broken_tone();
    x.extend(noise(0.003, (2.0 * FS) as usize));
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run(&mut app, &x, 1024);
    // The first pieces pass as calls, up to about 0.35 s. After the hold and the fade
    // the gate is closed, and the rest of the tone does not open it.
    assert!(out[ms(1600.0)..].iter().all(|&v| v == 0.0), "{}", peak(&out[ms(1600.0)..]));
}

#[test]
fn bats_only_follows_the_marked_kinds_and_the_lowest_bat_call() {
    let opens = |set_up: &dyn Fn(&mut BatApp), x: &[f32]| {
        let mut storage = vec![0.0f32; FS as usize];
        let mut app = bats_only_app(&mut storage);
        set_up(&mut app);
        peak(&run(&mut app, x, 1024)[AFTER_GATE_FIRST_CLOSES..]) > 0.0
    };
    let bats = train(2.0, 1.0, 6);
    assert!(opens(&|_| {}, &bats));
    assert!(!opens(&|app| app.set_marked(CallKind::Bat, false), &bats));

    // Calls peaking near 22 kHz: too low when the lowest bat call is 25 kHz.
    let mut low = noise(0.003, 2 * FS as usize);
    for k in 0..6 {
        let at = ms(1000.0 + k as f32 * 100.0);
        for (v, c) in low[at..].iter_mut().zip(chirp(23_000.0, 21_000.0, 0.1, 1_920)) {
            *v += c;
        }
    }
    assert!(opens(&|_| {}, &low));
    assert!(!opens(&|app| app.change_setting(Setting::LowestBatCall, 3), &low));

    // A marked steady tone opens it, once it has been heard as one.
    let tone = broken_tone();
    assert!(opens(&|app| app.set_marked(CallKind::Tone, true), &tone));
}

#[test]
fn bats_only_lets_all_of_a_time_expansion_playback_through() {
    let x = train(8.0, 1.0, 3);
    let play = |volume: AutoVolume| {
        let mut storage = vec![0.0f32; FS as usize];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(Mode::Te);
        app.set_auto_volume(volume);
        run(&mut app, &x, 1024)
    };
    let (on, bats_only) = (play(AutoVolume::On), play(AutoVolume::BatsOnly));
    assert!(peak(&on) > 0.0);
    assert_eq!(on, bats_only);
}

#[test]
fn skipped_analysis_neither_closes_nor_opens_the_gate() {
    // Calls for 0.5 s from 1 s, then a skip from 1.6 s to 4 s, longer than the hold.
    let x = train(8.0, 1.0, 5);
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run_skipping(&mut app, &x, &[ms(1600.0)..ms(4000.0)]);
    let at = |s: f32| ms(s * 1000.0);
    assert!(rms(&out[at(3.9)..at(4.0)]) > 0.0, "the gate closed while nothing could reopen it");
    // After the skip, the rest of the hold runs, and the gate closes.
    assert!(rms(&out[at(4.0)..at(4.2)]) > 0.0);
    assert!(out[at(5.5)..].iter().all(|&v| v == 0.0));

    // A train inside a skip does not open a closed gate, and neither does the restart.
    let x = train(8.0, 2.0, 5);
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run_skipping(&mut app, &x, &[ms(1500.0)..ms(3000.0)]);
    assert!(out[AFTER_GATE_FIRST_CLOSES..].iter().all(|&v| v == 0.0), "{}", peak(&out));

    // Lost input does not use up the hold either.
    let x = train(4.0, 1.0, 5);
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let n = ms(1600.0);
    let (mut l, mut r) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    app.process(&x[..n], &x[..n], &mut l[..n], &mut r[..n]);
    app.input_lost(FS as u32 * 2);
    app.process(&x[n..n + 1024], &x[n..n + 1024], &mut l[n..n + 1024], &mut r[n..n + 1024]);
    assert!(app.auto_volume_gate() > 0.99);
}

#[test]
fn the_gate_opens_and_closes_without_a_step() {
    // A signal that is level, so any step in the output comes from the gate.
    let tone_input = |n: usize| vec![0.01f32; n];
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::BatsOnly);
    auto.apply(&mut tone_input(ms(500.0)), Mode::Het, true);
    for block in tone_input(ms(500.0)).chunks_mut(256) {
        auto.apply(block, Mode::Het, true);
    }
    assert_eq!(auto.gate_level(), 0.0);
    auto.call_detected(false);
    let mut out = tone_input(ms(1000.0));
    for block in out.chunks_mut(256) {
        auto.apply(block, Mode::Het, true);
    }
    let top = peak(&out);
    assert!(top > 0.0);
    let step = out.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    // The square law over 5 ms: at most 2 / 960 of the level per sample.
    assert!(step < top * 0.0025, "{step} of {top}");
    assert!(out[..ms(5.0)].iter().any(|&v| v < top * 0.5));
    // Closed again after the 0.6 s hold and the 0.2 s fade.
    assert_eq!(auto.gate_level(), 0.0);
    assert_eq!(out[out.len() - 1], 0.0);
}

#[test]
fn the_auto_volume_row_shows_three_choices_a_tap_picks_one_and_the_wheel_steps() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    assert_eq!(app.auto_volume(), AutoVolume::Off);
    app.open_settings();
    let row = layout::control_row(Setting::AutoLevel);
    let (top, bottom) = (row.top_left.y, row.center().y + 18);
    // Every choice can be reached in one tap from any other, at any height of the row.
    for (i, &volume) in AutoVolume::ALL.iter().enumerate() {
        let c = layout::auto_volume_choice(i);
        assert!(c.size.width >= 24);
        for y in [top + 1, bottom] {
            assert!(app.is_tappable(c.center().x, y));
            app.touch(c.center().x, y);
            assert_eq!(app.auto_volume(), volume);
            assert_eq!(app.selected_setting(), Setting::AutoLevel);
        }
    }
    let before = draw(&app);
    app.wheel(-1);
    assert_eq!(app.auto_volume(), AutoVolume::On);
    assert_ne!(draw(&app), before);
    app.wheel(-5);
    assert_eq!(app.auto_volume(), AutoVolume::Off);
    app.wheel(1);
    assert_eq!(app.auto_volume(), AutoVolume::On);
    app.wheel(5);
    assert_eq!(app.auto_volume(), AutoVolume::BatsOnly);
    // Every label fits its choice.
    for (i, &volume) in AutoVolume::ALL.iter().enumerate() {
        let font = u8g2_fonts::FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_helvR10_tf>();
        let width = font
            .get_rendered_dimensions(volume.label(), embedded_graphics::prelude::Point::zero(), u8g2_fonts::types::VerticalPosition::Baseline)
            .unwrap()
            .advance
            .x;
        assert!(width + 4 <= layout::auto_volume_choice(i).size.width as i32, "{volume:?}: {width}");
    }
}

/// WCAG contrast of two RGB565 colours.
fn contrast(a: u16, b: u16) -> f32 {
    let luminance = |c: u16| {
        let channel = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let (r, g, b) = (((c >> 11) & 31) as f32 / 31.0, ((c >> 5) & 63) as f32 / 63.0, (c & 31) as f32 / 31.0);
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn each_silent_mix_notice_is_small_legible_and_inside_the_waterfall() {
    use embedded_graphics::pixelcolor::raw::RawU16;
    use embedded_graphics::prelude::RawData;
    let raw = |c: embedded_graphics::pixelcolor::Rgb565| RawU16::from(c).into_inner();
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mix_available(true);
    app.set_feed(Feed::Mix);
    // Fill the waterfall with bright content, so the panel is what makes it legible.
    run(&mut app, &noise(0.1, FS as usize), 4096);
    app.bufs.wf.fill(254);
    let clear = draw(&app);
    for (silence, most_lines) in [(MixSilence::RecPause, 1), (MixSilence::Stopped, 1), (MixSilence::PageOpen, 2)] {
        app.set_mix_silence(Some(silence));
        let with_notice = draw(&app);
        let (mut x0, mut x1, mut y0, mut y1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        let mut colours = std::collections::BTreeSet::new();
        for (i, (a, b)) in with_notice.iter().zip(&clear).enumerate() {
            if a != b {
                let (x, y) = ((i % WIDTH) as i32, (i / WIDTH) as i32);
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                colours.insert(*a);
            }
        }
        // Inside the waterfall, so the frequency labels and the time axis are untouched.
        assert!(x0 >= layout::WF_X && x1 < layout::WF_X + layout::WF_W as i32, "{silence:?} {x0} {x1}");
        assert!(y0 >= layout::WF_Y && y1 < layout::WF_Y + layout::WF_H as i32, "{silence:?} {y0} {y1}");
        // A small part of it: a line is 15 px high.
        assert!(y1 - y0 < most_lines * 15 + 4, "{silence:?} {y0} {y1}");
        assert!(x0 - layout::WF_X < 4 && y0 - layout::WF_Y < 4, "{silence:?} is not in the corner");
        // White text on the solid panel; the text is antialiased by no font here.
        assert!(colours.contains(&raw(palette::PANEL)), "{silence:?}");
        assert!(colours.contains(&raw(palette::TEXT)), "{silence:?}");
        assert!(contrast(raw(palette::TEXT), raw(palette::PANEL)) >= 4.5);
    }
    // The other notices still take priority over this one.
    app.set_inputs([None, None]);
    let no_input = draw(&app);
    app.set_mix_silence(None);
    assert_eq!(draw(&app), no_input);
}

#[test]
fn the_debug_switch_only_works_when_the_host_has_figures() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_settings_page(SettingsPage::Tests);
    let row = layout::control_row(Setting::DebugInfo).center();
    app.touch(row.x, row.y);
    assert!(!app.debug_info());
    app.set_debug_available(true);
    app.touch(row.x, row.y);
    assert!(app.debug_info());
    assert!(draw(&app).iter().any(|&p| p != 0));
}

#[test]
fn no_input_draws_a_notice_and_no_meters() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let meter_is_blank = |pixels: &[u16], y: i32| {
        let row = &pixels[y as usize * WIDTH..][..WIDTH];
        let x = layout::METER_X as usize;
        row[x..x + layout::METER_W as usize].iter().all(|&p| p == row[1])
    };
    let with_inputs = draw(&app);
    assert!(layout::METER_Y.iter().all(|&y| !meter_is_blank(&with_inputs, y + 1)));
    app.set_inputs([None, None]);
    let without_inputs = draw(&app);
    assert_ne!(without_inputs, with_inputs);
    assert!(layout::METER_Y.iter().all(|&y| meter_is_blank(&without_inputs, y + 1)));
    let before = app.source();
    app.change_setting(Setting::ListenTo, 1);
    assert_eq!(app.source(), before);
    app.open_settings();
    assert!(draw(&app).iter().any(|&p| p != 0));
}

#[test]
fn the_rate_warning_follows_the_input_rate() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_rate(48_000);
    assert_eq!(app.rate_warning(), Some(48_000));
    app.set_input_rate(FS as u32);
    assert_eq!(app.rate_warning(), None);
}

#[test]
fn block_size_does_not_change_the_output() {
    let x = signal();
    let mut s1 = vec![0.0f32; FS as usize];
    let mut s2 = vec![0.0f32; FS as usize];
    let mut a = BatApp::new(FS, &mut s1, buffers());
    let mut b = BatApp::new(FS, &mut s2, buffers());
    assert_eq!(run(&mut a, &x, 4096), run(&mut b, &x, 333));
}

#[test]
fn a_single_call_is_not_shown_as_a_bat() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let mut x = noise(0.003, FS as usize);
    for (v, c) in x[96_000..].iter_mut().zip(chirp(80_000.0, 40_000.0, 0.1, 960)) {
        *v += c;
    }
    run(&mut app, &x, 4096);
    assert!(app.last_call().is_none());
}

#[test]
fn settings_screen_opens_changes_and_closes() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    assert_eq!(app.lowest_bat_call_hz(), 16_000.0);
    let m = layout::MENU.center();
    app.touch(m.x, m.y);
    assert_eq!(app.screen(), Screen::Settings);
    // The wheel changes the selected row: the first, lowest bat call.
    app.wheel(1);
    assert_eq!(app.lowest_bat_call_hz(), 18_000.0);
    let minus = layout::setting_minus(0).center();
    app.touch(minus.x, minus.y);
    app.touch(minus.x, minus.y);
    assert_eq!(app.lowest_bat_call_hz(), 14_000.0);
    // + on the second row selects it and moves Listen to on from the mix.
    let plus = layout::setting_plus(1).center();
    app.touch(plus.x, plus.y);
    assert_eq!(app.selected_setting(), Setting::ListenTo);
    assert_eq!(app.source(), MonitorSource::First);
    app.wheel(5);
    assert_eq!(app.source(), MonitorSource::Second);
    assert!(draw(&app).iter().any(|&p| p != 0));
    let done = layout::DONE.center();
    app.touch(done.x, done.y);
    assert_eq!(app.screen(), Screen::Main);
    app.touch(m.x, m.y);
    app.button(Button::Home);
    assert_eq!(app.screen(), Screen::Main);
}

#[test]
fn a_lower_lowest_bat_call_finds_a_14_khz_train() {
    // Three 18 to 14 kHz calls: below the default, kept once lowered.
    let mut x = noise(0.003, FS as usize);
    for k in 0..3 {
        let at = 96_000 + k * 15_360;
        for (v, c) in x[at..].iter_mut().zip(chirp(17_000.0, 14_000.0, 0.1, 1_920)) {
            *v += c;
        }
    }
    let mut s1 = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut s1, buffers());
    run(&mut app, &x, 4096);
    assert!(app.last_call().is_none());
    let mut s2 = vec![0.0f32; 1024];
    let mut low = BatApp::new(FS, &mut s2, buffers());
    low.change_setting(Setting::LowestBatCall, -1);
    assert_eq!(low.lowest_bat_call_hz(), 14_000.0);
    run(&mut low, &x, 4096);
    let call = low.last_call().expect("the 14 kHz train was not found");
    assert!(call.peak_freq_hz < 17_500.0, "{call:?}");
}

/// Two seconds of a 22 kHz tone switching off for 5 ms every 35 ms, with
/// 2 ms fades, as a rat's 22 kHz call breaks up into pieces.
fn broken_tone() -> Vec<f32> {
    let mut x = noise(0.003, 2 * FS as usize);
    let (period, on, fade) = (ms(35.0), ms(30.0), ms(2.0));
    for (i, v) in x.iter_mut().enumerate().skip(ms(200.0)) {
        let p = i % period;
        let g = if p >= on { 0.0 } else { (p.min(on - p) as f32 / fade as f32).min(1.0) };
        *v += 0.05 * g * (core::f32::consts::TAU * 22_000.0 * i as f32 / FS).sin();
    }
    x
}

fn ms(t: f32) -> usize {
    (t * 0.001 * FS) as usize
}

#[test]
fn a_steady_tone_is_marked_only_when_its_switch_is_on() {
    let x = broken_tone();
    let mut s1 = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut s1, buffers());
    run(&mut app, &x, 4096);
    // The first pieces, before the tone has lasted 150 ms, may pass as bat
    // calls; none of the rest are marked.
    assert!(app.calls.iter().count() <= 4, "{}", app.calls.iter().count());
    assert!(app.calls.iter().all(|(_, k)| k == CallKind::Bat));

    let mut s2 = vec![0.0f32; 1024];
    let mut tones = BatApp::new(FS, &mut s2, buffers());
    tones.set_marked(CallKind::Tone, true);
    run(&mut tones, &x, 4096);
    assert_eq!(tones.last_call_kind(), Some(CallKind::Tone));
    assert!(tones.calls.iter().filter(|&(_, k)| k == CallKind::Tone).count() >= 10);
    assert!(draw(&tones).iter().any(|&p| p != 0));
}

#[test]
fn switches_flip_on_a_tap_and_follow_the_wheel() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    assert!(app.is_marked(CallKind::Bat));
    assert!(!app.is_marked(CallKind::Tone) && !app.is_marked(CallKind::Audible) && !app.is_marked(CallKind::Broadband));
    app.open_settings();
    // Steady tones are the second sound to mark: tapping its row anywhere flips it.
    let row = layout::control_row(Setting::Mark(CallKind::Tone));
    app.touch(row.top_left.x + 20, row.center().y);
    assert!(app.is_marked(CallKind::Tone));
    assert_eq!(app.selected_setting(), Setting::Mark(CallKind::Tone));
    let sw = layout::switch(Setting::Mark(CallKind::Tone)).center();
    assert!(app.is_tappable(sw.x, sw.y));
    app.touch(sw.x, sw.y);
    assert!(!app.is_marked(CallKind::Tone));
    // The wheel switches the selected row on going up and off going down.
    app.wheel(1);
    assert!(app.is_marked(CallKind::Tone));
    app.wheel(1);
    assert!(app.is_marked(CallKind::Tone));
    app.wheel(-2);
    assert!(!app.is_marked(CallKind::Tone));
    // Bat calls can be switched off too.
    let bats = layout::switch(Setting::Mark(CallKind::Bat)).center();
    app.touch(bats.x, bats.y);
    assert!(!app.is_marked(CallKind::Bat));
    run(&mut app, &signal(), 4096);
    assert!(app.last_call().is_none());
}

#[test]
fn init_in_place_over_garbage_matches_new() {
    // Built over memory full of 0xA5 bytes, as a static might hold after a
    // previous run, the app must behave exactly as one from new().
    let x = signal();
    let mut s1 = vec![0.0f32; 1024];
    let mut fresh = BatApp::new(FS, &mut s1, buffers());
    let out_fresh = run(&mut fresh, &x, 4096);

    let mut s2 = vec![0.0f32; 1024];
    let mut slot = std::boxed::Box::<BatApp>::new_uninit();
    // SAFETY: filling the slot's bytes before building into it.
    unsafe { core::ptr::write_bytes(slot.as_mut_ptr().cast::<u8>(), 0xa5, core::mem::size_of::<BatApp>()) };
    // SAFETY: init_in_place writes every field.
    let placed = unsafe {
        BatApp::init_in_place(slot.as_mut_ptr(), FS, &mut s2, buffers());
        slot.assume_init_mut()
    };
    let out_placed = run(placed, &x, 4096);
    assert_eq!(out_fresh, out_placed);
    assert_eq!(draw(&fresh), draw(placed));
    assert_eq!(fresh.last_call(), placed.last_call());
}

/// Run `x` through the app in blocks of 1024 samples, as the X8 patch does,
/// skipping the analysis of the blocks that start in `skip` (in samples).
fn run_skipping(app: &mut BatApp, x: &[f32], skip: &[core::ops::Range<usize>]) -> Vec<f32> {
    let mut out = vec![0.0; x.len()];
    let mut out_r = vec![0.0; 1024];
    for (k, (xi, ol)) in x.chunks(1024).zip(out.chunks_mut(1024)).enumerate() {
        let at = k * 1024;
        app.set_skip_analysis(skip.iter().any(|r| r.contains(&at)));
        app.process(xi, xi, ol, &mut out_r[..xi.len()]);
    }
    out
}

fn mark_everything(app: &mut BatApp) {
    for kind in [CallKind::Bat, CallKind::Single, CallKind::Tone, CallKind::Audible, CallKind::Broadband, CallKind::TooLow] {
        app.set_marked(kind, true);
    }
}

/// Quiet ultrasonic noise under a loud 1 kHz tone, so a jump in the signal makes
/// a click far above the noise, as the X8's low-frequency sound does.
fn noise_under_a_loud_tone(n: usize) -> Vec<f32> {
    let mut x = noise(0.0003, n);
    for (i, v) in x.iter_mut().enumerate() {
        *v += 0.05 * (core::f32::consts::TAU * 1000.0 * i as f32 / FS).sin();
    }
    x
}

#[test]
fn skipping_analysis_does_not_change_the_output_of_any_mode() {
    // The signal has calls that the analysis would act on, so that any effect of
    // the analysis on the sound would show. Time expansion is the one mode whose
    // output follows the detector; see the next two tests.
    let x = signal();
    let skips = [20_000..50_000, 100_000..110_000, 150_000..152_000];
    for mode in [Mode::Het, Mode::Comb, Mode::Fd, Mode::Pitch, Mode::Thru] {
        let (mut s1, mut s2) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
        let mut whole = BatApp::new(FS, &mut s1, buffers());
        let mut skipping = BatApp::new(FS, &mut s2, buffers());
        whole.set_mode(mode);
        skipping.set_mode(mode);
        let (a, b) = (run_skipping(&mut whole, &x, &[]), run_skipping(&mut skipping, &x, &skips));
        assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()), "{mode:?} changed");
        assert!(skipping.skipped_chunks() > 0);
        assert_eq!(whole.skipped_chunks(), 0);
    }
}

#[test]
fn time_expansion_output_is_unchanged_by_skipping_when_no_trigger_is_missed() {
    // Noise only, so neither run triggers: both stay silent, and both record.
    let x = noise(0.003, FS as usize);
    let (mut s1, mut s2) = (vec![0.0f32; FS as usize], vec![0.0f32; FS as usize]);
    let mut whole = BatApp::new(FS, &mut s1, buffers());
    let mut skipping = BatApp::new(FS, &mut s2, buffers());
    whole.set_mode(Mode::Te);
    skipping.set_mode(Mode::Te);
    let a = run_skipping(&mut whole, &x, &[]);
    let b = run_skipping(&mut skipping, &x, &[20_000..100_000]);
    assert_eq!(a, b);
    // MARK captures the same sound in both, with analysis on or off.
    whole.button(Button::Mark);
    skipping.button(Button::Mark);
    let rest = noise(0.003, FS as usize / 2);
    let a = run_skipping(&mut whole, &rest, &[]);
    let b = run_skipping(&mut skipping, &rest, &[0..FS as usize]);
    assert!(matches!(skipping.te_status(), TeStatus::Playing { .. }));
    assert_eq!(a, b);
}

#[test]
fn time_expansion_does_not_trigger_while_skipping_and_does_afterwards() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(Mode::Te);
    // A train while skipping: no detection, so no trigger, and no sound.
    let y = run_skipping(&mut app, &signal(), &[0..FS as usize]);
    assert_eq!(app.te_status(), TeStatus::Waiting);
    assert!(y.iter().all(|&v| v == 0.0));
    // The same train with analysis on again.
    run_skipping(&mut app, &signal(), &[]);
    assert_ne!(app.te_status(), TeStatus::Waiting);
    assert!(app.last_call().is_some());
}

#[test]
fn a_call_cut_by_skipping_is_reported_once_and_nothing_is_invented() {
    let mut x = noise(0.003, FS as usize);
    let (at, len) = (96_000, ms(10.0));
    for (i, v) in x[at..at + len].iter_mut().enumerate() {
        *v += 0.1 * (core::f32::consts::TAU * 50_000.0 * i as f32 / FS).sin();
    }
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    mark_everything(&mut app);
    // Skipping starts in the block at 97,280, 6.7 ms into the call, and ends
    // after it. The part before the skip is reported at the length it reached.
    run_skipping(&mut app, &x, &[97_280..100_000]);
    assert_eq!(app.calls.iter().count(), 1, "{:?}", app.calls.iter().collect::<Vec<_>>());
    let call = app.last_call().unwrap();
    assert!((call.start_sample as i64 - at as i64).abs() < ms(2.0) as i64, "{call:?}");
    assert!(call.end_sample <= 97_280, "{call:?}");
}

#[test]
fn skipping_and_resuming_invents_no_calls() {
    let x = noise_under_a_loud_tone(2 * FS as usize);
    // Episodes of every length from one block to a quarter of a second.
    let mut skips = Vec::new();
    let mut at = 40_000;
    for blocks in [1, 2, 3, 5, 8, 13, 21, 34, 47] {
        skips.push(at..at + blocks * 1024);
        at += blocks * 1024 + 11_000;
    }
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    mark_everything(&mut app);
    run_skipping(&mut app, &x, &skips);
    assert_eq!(app.calls.iter().count(), 0, "{:?}", app.calls.iter().collect::<Vec<_>>());
    assert!(app.last_call().is_none());
}

/// `x` with `lost` samples taken out at `at`, told to the app or not, as (the
/// marked detections, the output).
fn run_with_lost_input(x: &[f32], mode: Mode, at: usize, lost: usize, tell: bool) -> (Vec<(u64, CallKind)>, Vec<f32>) {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    mark_everything(&mut app);
    app.set_mode(mode);
    let mut out = run(&mut app, &x[..at], 1024);
    if tell {
        app.input_lost(lost as u32);
    }
    out.extend(run(&mut app, &x[at + lost..], 1024));
    if tell {
        assert_eq!(app.samples, x.len() as u64);
    }
    (app.calls.iter().collect(), out)
}

#[test]
fn lost_input_makes_no_false_call_and_no_step_in_the_output() {
    let x = noise_under_a_loud_tone(FS as usize);
    let (at, lost) = (96_000, 50);
    // The samples either side differ by far more than neighbours in the tone.
    let (without, plain) = run_with_lost_input(&x, Mode::Thru, at, lost, false);
    assert!(!without.is_empty(), "the jump should be a false call when the app is not told");
    assert!((plain[at] - plain[at - 1]).abs() > 0.02, "{} {}", plain[at - 1], plain[at]);

    let (with, faded) = run_with_lost_input(&x, Mode::Thru, at, lost, true);
    assert!(with.is_empty(), "{with:?}");
    assert!((faded[at] - faded[at - 1]).abs() < 0.002, "{} {}", faded[at - 1], faded[at]);
    // The fade changes only the first 64 samples.
    assert_eq!(faded[at + 64..], plain[at + 64..]);
}

/// The biggest change from one sample to the next in `x`.
fn biggest_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn lost_input_makes_no_click_in_the_modes_that_mix_the_input() {
    let x = noise_under_a_loud_tone(FS as usize);
    let (at, lost) = (96_000, 50);
    for mode in [Mode::Het, Mode::Comb] {
        let (_, plain) = run_with_lost_input(&x, mode, at, lost, false);
        let (_, faded) = run_with_lost_input(&x, mode, at, lost, true);
        let before = biggest_step(&faded[at - 20_000..at]);
        let click = biggest_step(&plain[at..at + 200]);
        let after = biggest_step(&faded[at..at + 200]);
        assert!(click > 1.5 * before, "{mode:?}: the jump should make a click: {click} {before}");
        assert!(after < 1.5 * before, "{mode:?}: {after} {before}");
    }
}

#[test]
fn lost_input_makes_no_step_in_a_time_expansion_capture() {
    let x = noise_under_a_loud_tone(FS as usize);
    let playback_step = |lost: usize| {
        let mut storage = vec![0.0f32; 96_000];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(Mode::Te);
        let at = ms(300.0);
        run(&mut app, &x[..ms(200.0)], 1024);
        app.button(Button::Mark);
        run(&mut app, &x[ms(200.0)..at], 1024);
        if lost > 0 {
            app.input_lost(lost as u32);
        }
        let mut out = run(&mut app, &x[at + lost..], 1024);
        out.extend(run(&mut app, &vec![0.0; 4 * FS as usize], 1024));
        let first = out.iter().position(|&v| v != 0.0).unwrap();
        let last = out.iter().rposition(|&v| v != 0.0).unwrap();
        biggest_step(&out[first + 2000..last - 2000])
    };
    let (smooth, joined) = (playback_step(0), playback_step(53));
    assert!(joined < 2.0 * smooth, "{joined} {smooth}");
}

#[test]
fn a_steady_wide_sound_cut_by_skips_is_not_taken_for_calls() {
    // Wider than a tone's pieces, from 1 s to the end, with 16 ms skipped every 100 ms.
    let mut x = noise(0.003, 6 * FS as usize);
    for khz in 38..=44 {
        for (i, v) in x[ms(1000.0)..].iter_mut().enumerate() {
            *v += 0.01 * (core::f32::consts::TAU * khz as f32 * 1000.0 * i as f32 / FS).sin();
        }
    }
    let skips: Vec<_> = (0..59).map(|k| k * ms(100.0)..k * ms(100.0) + ms(16.0)).collect();
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run_skipping(&mut app, &x, &skips);
    assert!(out[AFTER_GATE_FIRST_CLOSES..].iter().all(|&v| v == 0.0), "the gate opened: {}", peak(&out));
}

#[test]
fn changing_the_mode_sets_the_noise_floor_again() {
    let x = noise(0.01, FS as usize);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_auto_level_target(X8_TARGET);
    app.set_auto_volume(AutoVolume::On);
    app.set_mode(Mode::Het);
    run(&mut app, &x, 1024);
    let het_floor = app.level.floor_db().unwrap();
    app.set_mode(Mode::Thru);
    assert_eq!(app.level.floor_db(), None);
    run(&mut app, &x[..ms(20.0)], 1024);
    let thru_floor = app.level.floor_db().unwrap();
    assert!((thru_floor - db(rms(&x))).abs() < 3.0, "{thru_floor} {}", db(rms(&x)));
    assert!(thru_floor > het_floor + 3.0, "{thru_floor} {het_floor}");
    // A setting that does not change the sound leaves the floor alone.
    app.set_mode(Mode::Thru);
    assert_eq!(app.level.floor_db(), Some(thru_floor));
}

#[test]
fn bats_only_closes_on_evenly_spaced_pulses() {
    // 12 ms pulses every 50 ms, from 1 s to the end, wider than a tone's pieces.
    let mut x = noise(0.003, 6 * FS as usize);
    for k in 0..100 {
        let at = ms(1000.0 + k as f32 * 50.0);
        for (v, c) in x[at..].iter_mut().zip(chirp(45_000.0, 43_000.0, 0.05, ms(12.0))) {
            *v += c;
        }
    }
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = bats_only_app(&mut storage);
    let out = run(&mut app, &x, 1024);
    // The first pulses open the gate for a moment. Eight pulses, the 1 s hold, and
    // the 200 ms close end by 2.6 s.
    assert!(rms(&out[ms(1000.0)..ms(2000.0)]) > 0.0);
    assert!(out[ms(2700.0)..].iter().all(|&v| v == 0.0), "{}", peak(&out[ms(2700.0)..]));
}

#[test]
fn times_stay_true_across_skips_and_lost_input() {
    let mut x = noise(0.003, 2 * FS as usize);
    let t0 = 300_000;
    for k in 0..3 {
        for (v, c) in x[t0 + k * 15_360..].iter_mut().zip(chirp(80_000.0, 40_000.0, 0.1, 960)) {
            *v += c;
        }
    }
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    // 100,000 samples, of which 30,000 are not analysed and 20,000 are lost.
    run_skipping(&mut app, &x[..50_000], &[10_000..40_000]);
    app.input_lost(20_000);
    run_skipping(&mut app, &x[70_000..], &[]);
    assert_eq!(app.samples, x.len() as u64);
    let starts: Vec<u64> = app.calls.iter().map(|(t, _)| t).collect();
    assert_eq!(starts.len(), 3, "{starts:?}");
    for (k, t) in starts.iter().enumerate() {
        let expect = (t0 + k * 15_360) as i64;
        assert!((*t as i64 - expect).abs() < ms(2.0) as i64, "call {k} at {t}, expected {expect}");
    }
}

#[test]
fn columns_not_analysed_are_grey_and_the_waterfall_keeps_moving() {
    let x = noise(0.003, FS as usize);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    run(&mut app, &x[..96_000], 1024);
    let (floor_before, column_before) = (app.row_floor, app.wf_column);
    let real = draw(&app);

    // Half a second skipped, which is about 50 display columns. The floor must
    // not follow the column cut short by the skip, or columns with no data.
    let skipped = 96_000;
    run_skipping(&mut app, &noise(0.003, skipped), &[0..skipped]);
    assert_eq!(app.row_floor, floor_before);
    assert_eq!(app.skipped_chunks(), (skipped / CHUNK) as u32);
    let expected = (96_000 + skipped as u64 - FFT_N as u64 / 2) / 1920;
    assert_eq!(app.wf_column, expected);
    assert!(app.wf_column >= column_before + 49);

    // The newest columns on screen are grey, right up to the last finished one,
    // and the columns before them are not.
    let pixels = draw(&app);
    let pixel = |x: i32, y: i32| pixels[y as usize * WIDTH + x as usize];
    let grey = palette::WATERFALL_565[WF_NO_DATA as usize];
    let right = layout::WF_X + WF_W as i32 - 1;
    let middle = layout::WF_Y + WF_H as i32 / 2;
    let count = (app.wf_column - column_before) as i32;
    assert!((0..count - 1).all(|back| pixel(right - back, middle) == grey), "the skip is not grey");
    assert_ne!(pixel(right - count - 2, middle), grey);
    assert_ne!(pixels, real);

    // After the skip the waterfall carries on with real columns, at true times.
    run_skipping(&mut app, &noise(0.003, 19_200), &[]);
    let pixels = draw(&app);
    let pixel = |x: i32, y: i32| pixels[y as usize * WIDTH + x as usize];
    assert_ne!(pixel(right - 2, middle), grey);
    assert_eq!(pixel(right - 20, middle), grey);
}

#[test]
fn a_skip_longer_than_the_screen_leaves_only_grey() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    run(&mut app, &noise(0.003, FS as usize), 1024);
    let long = 4 * FS as usize;
    run_skipping(&mut app, &noise(0.003, long), &[0..long]);
    assert!(app.bufs.wf.iter().all(|&v| v == WF_NO_DATA));
}

#[test]
fn the_waterfall_is_the_same_as_taking_pixels_from_every_column() {
    // The app takes the loudest level per bin over a display column and makes the
    // pixels once. This recomputes the waterfall the way that was replaced, with
    // pixels from every spectrum column, and compares every byte.
    let x = signal();
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    run(&mut app, &x, 4096);

    let mut analyzer = SpectrumAnalyzer::<FFT_N>::new(FS);
    analyzer.set_hop(FFT_HOP);
    let mut detector = CallDetector::<FFT_N>::new(FS);
    let mut accum = [f32::MIN; WF_H];
    let mut floor = [-110.0f32; WF_H];
    let mut floor_set = false;
    let mut column = 0u64;
    let mut wf = vec![0u8; WF_W * WF_H];
    let mut next = 0;
    let scale = 255.0 / DEFAULT_DISPLAY_RANGE_DB;
    analyzer.push(&x, |col| {
        detector.process(col);
        let this_column = col.centre_sample / 1920;
        if this_column != column {
            if !detector.is_active() {
                let rate = if floor_set { 0.02 } else { 1.0 };
                for (f, &a) in floor.iter_mut().zip(accum.iter()) {
                    *f = (*f + rate * (a - *f)).max(MIN_FLOOR_DB);
                }
                floor_set = true;
            }
            let black = floor.iter().sum::<f32>() / WF_H as f32 + DEFAULT_DISPLAY_OFFSET_DB;
            for (dst, &db) in wf[next * WF_H..(next + 1) * WF_H].iter_mut().zip(accum.iter()) {
                *dst = level(db, black, scale).min(WF_NO_DATA - 1);
            }
            next = (next + 1) % WF_W;
            column = this_column;
            accum.fill(f32::MIN);
        }
        let mut px = [0.0f32; WF_H];
        col.to_pixels(VIEW_HI_HZ, VIEW_LO_HZ, &mut px);
        for (a, &p) in accum.iter_mut().zip(&px) {
            *a = a.max(p);
        }
    });
    assert_eq!(next, app.wf_next);
    assert_eq!(column, app.wf_column);
    assert!(wf == app.bufs.wf, "the waterfall differs");
}

#[test]
fn a_train_of_short_calls_is_found_at_the_apps_hop() {
    // Ten calls of each length, 18 dB above the noise's peaks, at varying offsets
    // from the spectrum columns.
    for len_ms in [1.0, 2.0, 5.0] {
        let mut x = noise(0.003, FS as usize);
        // The spacing varies by more than 1 ms, as a bat's does: evenly spaced pulses
        // stop counting as calls.
        let starts: Vec<usize> = (0..10).map(|k| 60_000 + k * 12_000 + k * 37 + (k % 3) * 500).collect();
        let call = chirp(80_000.0, 40_000.0, 0.024, ms(len_ms));
        for &at in &starts {
            for (v, c) in x[at..].iter_mut().zip(&call) {
                *v += c;
            }
        }
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        run(&mut app, &x, 1024);
        let found: Vec<u64> = app.calls.iter().map(|(t, _)| t).collect();
        assert_eq!(found.len(), starts.len(), "{len_ms} ms: {found:?}");
    }
}

#[test]
fn history_covers_the_longest_call_and_its_snapshot_margins() {
    // The longest call reported is 100 ms, and it is reported 2 ms after its end.
    let columns_needed = (100.0 + 2.0 + 2.0 * SNAP_MARGIN_MS) * 0.001 * FS / FFT_HOP as f32;
    assert!(HISTORY as f32 >= columns_needed, "{HISTORY} columns, {columns_needed} needed");
}

/// The MIX file switch flips whatever MANUAL's REC READY for MASTER says, and its line changes
/// with both: saved, not saved because of REC READY, or not saved because of the switch.
#[test]
fn mix_file_line_follows_switch_and_rec_ready() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let row = layout::control_row(Setting::MixFile).center();
    app.set_mix_file_available(true);
    app.open_settings_page(SettingsPage::Record);
    let saved = draw(&app);
    app.set_manual_mix_file(false);
    let not_ready = draw(&app);
    assert_ne!(saved, not_ready);
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.mix_file());
    let overridden = draw(&app);
    assert_ne!(overridden, not_ready);
    app.wheel(1);
    assert!(app.mix_file());
}

/// The settings screen's tabs: each opens its page, the wheel changes only a setting on the page
/// shown, and a host with nothing for Record gets no Record tab.
#[test]
fn settings_tabs() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_settings();
    assert_eq!(app.settings_page(), SettingsPage::Listen);
    assert_eq!(app.settings_pages().collect::<Vec<_>>(), [SettingsPage::Listen, SettingsPage::Tests]);
    app.set_mix_file_available(true);
    app.set_debug_available(true);
    assert_eq!(app.settings_pages().count(), 3);
    for (i, page) in SettingsPage::ALL.into_iter().enumerate() {
        let tab = layout::tab(i, 3).center();
        assert!(app.is_tappable(tab.x, tab.y));
        app.touch(tab.x, tab.y);
        assert_eq!(app.settings_page(), page);
        assert_eq!(app.screen(), Screen::Settings);
    }
    // On Tests the wheel changes Debug info, the page's setting.
    assert_eq!(app.selected_setting(), Setting::DebugInfo);
    app.wheel(1);
    assert!(app.debug_info());
    // Record's MIX file, then back to Listen, where the first setting is selected again.
    app.open_settings_page(SettingsPage::Record);
    assert_eq!(app.selected_setting(), Setting::MixFile);
    app.wheel(-1);
    assert!(!app.mix_file());
    app.open_settings_page(SettingsPage::Listen);
    assert_eq!(app.selected_setting(), Setting::LowestBatCall);
    // The page stays when the settings screen is closed and opened again.
    app.open_settings_page(SettingsPage::Record);
    app.button(Button::Home);
    app.open_settings();
    assert_eq!(app.settings_page(), SettingsPage::Record);
    // A host that stops offering Record's items shows Listen in its place.
    app.set_mix_file_available(false);
    assert_eq!(app.settings_page(), SettingsPage::Listen);
    assert!(draw(&app).iter().any(|&p| p != 0));
}

/// The GUANO switch on the Record page: on by default, offered only by a host that asks for it,
/// and flipped by a tap on its row.
#[test]
fn guano_switch() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let row = layout::panel_switch_row(layout::RECORD_GUANO).center();
    // The MIX file switch gives the host a Record page.
    app.set_mix_file_available(true);
    app.open_settings_page(SettingsPage::Record);
    assert!(app.guano());
    assert!(!app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(app.guano());
    app.set_guano_available(true);
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.guano());
    app.touch(row.x, row.y);
    assert!(app.guano());
}

/// The MIX file switch: on by default, offered only by a host that asks for it,
/// and flipped by a tap on its row or by the wheel.
#[test]
fn mix_file_switch() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let row = layout::control_row(Setting::MixFile).center();
    // GUANO metadata gives the host a Record page.
    app.set_guano_available(true);
    app.open_settings_page(SettingsPage::Record);
    assert!(app.mix_file());
    assert!(!app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(app.mix_file());
    app.set_mix_file_available(true);
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(!app.mix_file());
    assert_eq!(app.selected_setting(), Setting::MixFile);
    app.wheel(1);
    assert!(app.mix_file());
    app.wheel(-1);
    assert!(!app.mix_file());
}

/// The pause switch: offered by a host that asks for it, off by default, flipped by a tap; a
/// refused pause shows on the main screen for a few seconds.
#[test]
fn pause_switch_and_notice() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_pause_option_available(true);
    app.open_settings_page(SettingsPage::Record);
    assert!(!app.pause_with_rec());
    let row = layout::control_row(Setting::PauseWithRec).center();
    assert!(app.is_tappable(row.x, row.y));
    app.touch(row.x, row.y);
    assert!(app.pause_with_rec());
    app.button(Button::Home);
    let before = draw(&app);
    app.pause_refused();
    assert!(app.pause_refused_shown());
    assert_ne!(draw(&app), before);
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    for _ in 0..(4 * FS as usize / 1024) {
        app.process(&[0.0; 1024], &[0.0; 1024], &mut l, &mut r);
    }
    assert!(!app.pause_refused_shown());
}
