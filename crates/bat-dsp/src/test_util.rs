//! Signals and measurements for tests. Host only (uses std).

use std::vec::Vec;

pub fn sine(fs: f32, freq: f32, amp: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (core::f64::consts::TAU * freq as f64 * i as f64 / fs as f64).sin() as f32)
        .collect()
}

/// Linear chirp from `f0` to `f1` over `n` samples.
#[allow(dead_code)]
pub fn chirp(fs: f32, f0: f32, f1: f32, amp: f32, n: usize) -> Vec<f32> {
    let dur = n as f64 / fs as f64;
    let k = (f1 as f64 - f0 as f64) / dur;
    (0..n)
        .map(|i| {
            let t = i as f64 / fs as f64;
            amp * (core::f64::consts::TAU * (f0 as f64 * t + 0.5 * k * t * t)).sin() as f32
        })
        .collect()
}

/// Deterministic white noise in [-amp, amp] (xorshift), so tests repeat.
#[allow(dead_code)]
pub fn noise(amp: f32, n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            amp * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// Frequency of a clean tone from its zero-crossing rate.
pub fn zero_crossing_freq(x: &[f32], fs: f32) -> f32 {
    let mut first = None;
    let mut last = 0usize;
    let mut count = 0u32;
    for i in 1..x.len() {
        if (x[i - 1] < 0.0) != (x[i] < 0.0) {
            if first.is_none() {
                first = Some(i);
            } else {
                count += 1;
            }
            last = i;
        }
    }
    match first {
        Some(f) if count > 0 => count as f32 / 2.0 / ((last - f) as f32 / fs),
        _ => 0.0,
    }
}

/// Power of `x` at one frequency (Goertzel), normalised so a full-scale sine
/// at that frequency gives about 0.5.
pub fn tone_power(x: &[f32], fs: f32, freq: f32) -> f32 {
    let w = core::f64::consts::TAU * freq as f64 / fs as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let p = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    (2.0 * p / (x.len() as f64 * x.len() as f64)) as f32
}

/// Run a block processor over `x` in blocks of `block` samples.
pub fn run_blocks(x: &[f32], block: usize, mut f: impl FnMut(&[f32], &mut [f32])) -> Vec<f32> {
    let mut y = std::vec![0.0; x.len()];
    for (xi, yi) in x.chunks(block).zip(y.chunks_mut(block)) {
        f(xi, yi);
    }
    y
}
