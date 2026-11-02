use std::vec;
use std::vec::Vec;

use super::*;

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
    assert_eq!(app.source(), MonitorSource::Input3);
    app.wheel(5);
    assert_eq!(app.source(), MonitorSource::Input4);
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
    // Steady tones are the second switch: tapping its row anywhere flips it.
    let row = layout::switch_row(1);
    app.touch(row.top_left.x + 20, row.center().y);
    assert!(app.is_marked(CallKind::Tone));
    assert_eq!(app.selected_setting(), Setting::Mark(CallKind::Tone));
    let sw = layout::switch(1).center();
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
    let bats = layout::switch(0).center();
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
