//! Stereo on recordings: whether the two ears differ, and how loud each is against the
//! mono output the app had before.
//!
//! cargo run -p bat-app --release --example stereo_levels -- FILES...
//!
//! Each file plays through the app in HET, tuned to the median peak of its bat calls, and
//! in COMB, with Auto volume off, once in stereo and once as the mono average. Levels are
//! at the headphone stage, at the stock headphone gain, with the X8's `OUTPUT_GAIN` and
//! ceiling. The files must be at 192 kHz, with the two inputs as the first two channels.
//!
//! For each detected bat call, and for the quiet 10 ms windows away from any detection:
//! - "L", "R", "mono": median RMS of each output, in dBFS.
//! - "|L-R|": the difference between the ears, median and 90th percentile.
//! - "ear-mono": the louder ear against the mono output, median.
//! - "corr": correlation between the ears, median.
//!
//! "at ceiling" is the share of samples of the whole file held at the ceiling, for each
//! output, and "peak" the loudest sample before it. The same again with Auto volume on, at
//! the X8's target.

use bat_app::{AutoVolume, BatApp, Buffers, Mode, MonitorSource};
use bat_dsp::classify::{CallClassifier, CallKind};
use bat_dsp::detector::{CallDetector, CallEvent};
use bat_dsp::spectrum::SpectrumAnalyzer;

const FS: f32 = 192_000.0;
/// From `crates/x8-patch/src/patch/audio.rs`.
const OUTPUT_GAIN: f32 = 8.0;
const OUTPUT_CEILING: f32 = 0.5;
const TARGET: f32 = 0.25 / OUTPUT_GAIN;
const WINDOW: usize = 1920;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

fn percentile(values: &mut [f32], p: f32) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    values.sort_by(f32::total_cmp);
    values[((values.len() - 1) as f32 * p) as usize]
}

fn read_two(path: &str) -> Option<(Vec<f32>, Vec<f32>)> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate != FS as u32 || spec.channels < 2 {
        eprintln!("skipped {path}: {} Hz, {} channels", spec.sample_rate, spec.channels);
        return None;
    }
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.into_samples::<f32>().map_while(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.into_samples::<i32>().map_while(Result::ok).map(|s| s as f32 * scale).collect()
        }
    };
    let side = |c: usize| samples.chunks_exact(ch).map(|f| f[c]).collect();
    Some((side(0), side(1)))
}

fn detect(a: &[f32], b: &[f32]) -> Vec<(CallEvent, bool)> {
    let mono: Vec<f32> = a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect();
    let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(FS);
    analyzer.set_hop(bat_app::FFT_HOP);
    let mut detector: CallDetector<512> = CallDetector::new(FS);
    let mut events = Vec::new();
    for block in mono.chunks(256) {
        analyzer.push(block, |col| events.extend(detector.process(col)));
    }
    events.extend(detector.flush());
    let (band_lo, _) = detector.band();
    let mut classifier = CallClassifier::new(FS, band_lo);
    let mut found: Vec<(CallEvent, bool)> = Vec::with_capacity(events.len());
    for event in events {
        let verdict = classifier.classify(&event);
        if let Some(first) = verdict.earlier
            && let Some(d) = found.iter_mut().rev().find(|d| d.0.start_sample == first.start_sample)
        {
            d.1 = true;
        }
        found.push((event, verdict.kind == CallKind::Bat));
    }
    found
}

/// The app's output in this mode, in the X8's units before its gain.
fn play(a: &[f32], b: &[f32], mode: Mode, het_hz: f32, source: MonitorSource, volume: AutoVolume) -> (Vec<f32>, Vec<f32>) {
    let mut storage = vec![0.0f32; FS as usize];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(FS, &mut storage, &mut buffers);
    app.set_mode(mode);
    app.set_source(source);
    app.set_auto_level_target(TARGET);
    app.set_auto_volume(volume);
    if mode == Mode::Het {
        app.wheel(((het_hz - 45_000.0) / 500.0) as i32);
    }
    let (mut left, mut right) = (vec![0.0; a.len()], vec![0.0; a.len()]);
    for (((x, y), l), r) in a.chunks(1024).zip(b.chunks(1024)).zip(left.chunks_mut(1024)).zip(right.chunks_mut(1024)) {
        app.process(x, y, l, r);
    }
    (left, right)
}

fn headphones(x: &[f32]) -> Vec<f32> {
    x.iter().map(|v| (v * OUTPUT_GAIN).clamp(-OUTPUT_CEILING, OUTPUT_CEILING)).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let (mut ab, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa * bb).sqrt().max(1e-20)
}

fn at_ceiling(x: &[f32]) -> f32 {
    100.0 * x.iter().filter(|v| v.abs() >= OUTPUT_CEILING).count() as f32 / x.len() as f32
}

#[derive(Default)]
struct Figures {
    left: Vec<f32>,
    right: Vec<f32>,
    mono: Vec<f32>,
    apart: Vec<f32>,
    over_mono: Vec<f32>,
    corr: Vec<f32>,
    count: usize,
}

impl Figures {
    fn add(&mut self, l: &[f32], r: &[f32], m: &[f32]) {
        let (dl, dr, dm) = (db(rms(l)), db(rms(r)), db(rms(m)));
        self.left.push(dl);
        self.right.push(dr);
        self.mono.push(dm);
        self.apart.push((dl - dr).abs());
        self.over_mono.push(dl.max(dr) - dm);
        self.corr.push(correlation(l, r));
        self.count += 1;
    }

    fn print(&mut self, name: &str) {
        println!(
            "  {name:<7}{:>6} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>9.1} {:>7.2}",
            self.count,
            percentile(&mut self.left, 0.5),
            percentile(&mut self.right, 0.5),
            percentile(&mut self.mono, 0.5),
            percentile(&mut self.apart, 0.5),
            percentile(&mut self.apart, 0.9),
            percentile(&mut self.over_mono, 0.5),
            percentile(&mut self.corr, 0.5),
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{:>9} {:>6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>9} {:>7}", "", "count", "L", "R", "mono", "|L-R|", "p90", "ear-mono", "corr");
    for path in &args {
        let Some((a, b)) = read_two(path) else { continue };
        let found = detect(&a, &b);
        let mut peaks: Vec<f32> = found.iter().filter(|d| d.1).map(|d| d.0.peak_freq_hz).collect();
        let het_hz = if peaks.is_empty() { 45_000.0 } else { (percentile(&mut peaks, 0.5) / 500.0).round() * 500.0 };
        let bats = found.iter().filter(|d| d.1).count();
        println!("{path}: {:.0} s, {bats} bat calls, {} detections", a.len() as f32 / FS, found.len());
        println!("  inputs: RMS {:.1} and {:.1} dBFS, correlation {:.2}", db(rms(&a)), db(rms(&b)), correlation(&a, &b));
        let mean: Vec<f32> = a.iter().zip(&b).map(|(x, y)| 0.5 * (x + y)).collect();
        for (mode, name) in [(Mode::Het, "HET"), (Mode::Comb, "COMB")] {
            if mode == Mode::Het {
                println!("  {name} tuned to {:.1} kHz", het_hz / 1000.0);
            } else {
                println!("  {name}");
            }
            for (volume, calls_name, quiet_name) in [(AutoVolume::Off, "calls", "quiet"), (AutoVolume::On, "c. auto", "q. auto")] {
                let (left, right) = play(&a, &b, mode, het_hz, MonitorSource::Both, volume);
                let (mono, _) = play(&mean, &mean, mode, het_hz, MonitorSource::First, volume);
                let (hl, hr, hm) = (headphones(&left), headphones(&right), headphones(&mono));
                let mut calls = Figures::default();
                let mut busy = vec![false; a.len() / WINDOW + 1];
                for (e, bat) in &found {
                    let from = e.start_sample as usize;
                    let to = (e.end_sample as usize).min(a.len());
                    // Windows near any detection are not quiet: 5 ms before to 20 ms after.
                    let first = from.saturating_sub(960) / WINDOW;
                    let last = (to + 3840) / WINDOW;
                    for w in first..=last.min(busy.len() - 1) {
                        busy[w] = true;
                    }
                    if *bat && to > from + 64 {
                        calls.add(&hl[from..to], &hr[from..to], &hm[from..to]);
                    }
                }
                let mut quiet = Figures::default();
                for (w, _) in busy.iter().enumerate().filter(|(_, b)| !**b) {
                    let (from, to) = (w * WINDOW, ((w + 1) * WINDOW).min(a.len()));
                    if to - from == WINDOW {
                        quiet.add(&hl[from..to], &hr[from..to], &hm[from..to]);
                    }
                }
                calls.print(calls_name);
                quiet.print(quiet_name);
                let peak = |x: &[f32]| db(x.iter().fold(0.0f32, |m, v| m.max(v.abs())) * OUTPUT_GAIN);
                println!(
                    "  at ceiling: L {:.3}% R {:.3}% mono {:.3}%; peak before it: L {:.1} R {:.1} mono {:.1} dBFS",
                    at_ceiling(&hl),
                    at_ceiling(&hr),
                    at_ceiling(&hm),
                    peak(&left),
                    peak(&right),
                    peak(&mono),
                );
            }
        }
    }
}
