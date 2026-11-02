//! Time the bat app's processing and drawing on this machine, per second of
//! 192 kHz audio, for each listening mode. The X8 is far slower, but the
//! shares show where the time goes.
//!
//! cargo run -p bat-app --release --example bench

use std::time::Instant;

use bat_app::{BatApp, HEIGHT, Mode, Rgb565Buffer, WIDTH};

fn main() {
    let fs = 192_000.0;
    let seconds = 5;
    let n = fs as usize * seconds;
    let mut s = 12345u32;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = 0.003 * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0);
            let t = i as f32 / fs;
            let call = if (t * 10.0).fract() < 0.005 { 0.1 * (std::f32::consts::TAU * 45_000.0 * t).sin() } else { 0.0 };
            noise + call
        })
        .collect();
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    for mode in Mode::ALL {
        let mut storage = vec![0.0f32; 96_000];
        let mut app = BatApp::new(fs, &mut storage);
        app.set_mode(mode);
        let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
        let start = Instant::now();
        for chunk in x.chunks(1024) {
            let k = chunk.len();
            app.process(chunk, chunk, &mut l[..k], &mut r[..k]);
        }
        let process = start.elapsed().as_secs_f64() / seconds as f64;
        let start = Instant::now();
        let frames = 100;
        for _ in 0..frames {
            app.render(&mut Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT));
        }
        let draw = start.elapsed().as_secs_f64() / frames as f64;
        println!(
            "{:6}: process {:6.1} ms per second of audio, draw {:5.2} ms per frame",
            mode.label(),
            process * 1000.0,
            draw * 1000.0
        );
    }
}
