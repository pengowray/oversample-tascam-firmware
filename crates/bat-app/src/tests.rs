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

/// One second of noise with a 5 ms call at 0.5 s, on both inputs.
fn signal() -> Vec<f32> {
    let mut x = noise(0.003, FS as usize);
    for (v, c) in x[96_000..].iter_mut().zip(chirp(80_000.0, 40_000.0, 0.1, 960)) {
        *v += c;
    }
    x
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
    let mut app = BatApp::new(FS, &mut storage);
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
    let mut app = BatApp::new(FS, &mut storage);
    assert_eq!(app.het_lo_hz(), 45_000.0);
    app.wheel(4);
    assert_eq!(app.het_lo_hz(), 47_000.0);
    app.wheel(-1000);
    assert_eq!(app.het_lo_hz(), 15_000.0);
    // A tap at the vertical middle of the waterfall is about 55.5 kHz.
    app.touch(layout::WF_X + 10, layout::WF_Y + layout::WF_H as i32 / 2);
    assert!((app.het_lo_hz() - 55_500.0).abs() <= 500.0, "{}", app.het_lo_hz());
}

#[test]
fn tapping_a_mode_button_selects_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage);
    let r = layout::mode_button(3);
    app.touch(r.center().x, r.center().y);
    assert_eq!(app.mode(), Mode::Te);
}

#[test]
fn time_expansion_starts_on_a_call_and_plays() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage);
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
    let mut app = BatApp::new(FS, &mut storage);
    app.button(Button::Rec);
    assert!(app.is_recording());
    run(&mut app, &noise(0.001, 2 * FS as usize), 4096);
    assert_eq!(app.recorded_seconds(), 2);
    app.button(Button::Rec);
    assert!(!app.is_recording());
}

#[test]
fn block_size_does_not_change_the_output() {
    let x = signal();
    let mut s1 = vec![0.0f32; FS as usize];
    let mut s2 = vec![0.0f32; FS as usize];
    let mut a = BatApp::new(FS, &mut s1);
    let mut b = BatApp::new(FS, &mut s2);
    assert_eq!(run(&mut a, &x, 4096), run(&mut b, &x, 333));
}
