//! Auto volume on recordings: what each setting does in each listening mode, for
//! choosing and checking the constants in `level.rs`.
//!
//! cargo run -p bat-app --release --example auto_volume -- [--modes het,comb,fd,te,thru] FILES...
//!
//! Each file plays through the app at the X8's target (-12 dBFS at the headphones,
//! after `OUTPUT_GAIN` of +18 dB), with Auto volume at Off, as B231 had it, On, and
//! Bats only, and HET tuned to the median peak of the bat calls in the file. Levels in
//! dB are at the headphone stage, at the stock headphone gain. The files must be at
//! 192 kHz.
//!
//! - "quiet": 10 ms windows with no detection from 5 ms before it to 20 ms after it.
//!   "gain" is the gain in those windows (median, then 5th to 95th percentile), and
//!   "noise" is the median RMS of the output in them.
//! - "gain in call" is the median gain in the other windows; "call out" is the output
//!   peak of each call or single (10th, 50th and 90th percentile).
//! - "moved" is the gain's mean absolute change per 100 ms over the file.
//! - For Bats only: the share of the file the gate is open, the openings per minute,
//!   the bat calls heard whole, and per pass the bat calls missed before the first one
//!   heard whole and after it. A pass is a run of bat calls with gaps of up to 2 s.

use bat_app::{AutoVolume, BatApp, Buffers, Mode};
use bat_dsp::classify::{CallClassifier, CallKind};
use bat_dsp::detector::{CallDetector, CallEvent};
use bat_dsp::spectrum::SpectrumAnalyzer;

const FS: f32 = 192_000.0;
/// The X8's target and output gain, from `crates/x8-patch/src/patch/audio.rs`.
const OUTPUT_GAIN: f32 = 8.0;
const OUTPUT_CEILING: f32 = 0.5;
const TARGET: f32 = 0.25 / OUTPUT_GAIN;
const WINDOW: usize = 1920;
const PASS_GAP_S: f32 = 2.0;

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

fn read_stereo(path: &str) -> Option<(Vec<f32>, Vec<f32>)> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate != FS as u32 {
        eprintln!("skipped {path}: {} Hz", spec.sample_rate);
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
    let side = |c: usize| samples.chunks_exact(ch).map(|f| f[c.min(ch - 1)]).collect();
    Some((side(0), side(1)))
}

struct Detected {
    event: CallEvent,
    kind: CallKind,
    bat: bool,
}

fn detect(a: &[f32], b: &[f32]) -> Vec<Detected> {
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
    let mut found: Vec<Detected> = Vec::with_capacity(events.len());
    for event in events {
        let verdict = classifier.classify(&event);
        if let Some(first) = verdict.earlier
            && let Some(d) = found.iter_mut().rev().find(|d| d.event.start_sample == first.start_sample)
        {
            d.bat = true;
        }
        found.push(Detected { event, kind: verdict.kind, bat: verdict.kind == CallKind::Bat });
    }
    found
}

/// The app's output per 10 ms window, at the headphone stage.
#[derive(Default)]
struct Run {
    rms: Vec<f32>,
    peak: Vec<f32>,
    gain: Vec<f32>,
    gate: Vec<f32>,
}

impl Run {
    fn add(&mut self, window: &[f32], gain: f32, gate: f32) {
        let (mut sum, mut top) = (0.0f32, 0.0f32);
        for &v in window {
            let v = (v * OUTPUT_GAIN).clamp(-OUTPUT_CEILING, OUTPUT_CEILING);
            sum += v * v;
            top = top.max(v.abs());
        }
        self.rms.push((sum / window.len() as f32).sqrt());
        self.peak.push(top);
        self.gain.push(gain);
        self.gate.push(gate);
    }
}

/// The gain stage as B231 had it, to compare with: no noise floor.
struct B231 {
    envelope: f32,
    gain: f32,
    keep: f32,
    fall: f32,
    rise: f32,
}

impl B231 {
    fn new() -> Self {
        let per_sample = |s: f32| 1.0 - (-1.0 / (s * FS)).exp();
        Self { envelope: 0.0, gain: 1.0, keep: 1.0 - per_sample(1.5), fall: per_sample(0.001), rise: per_sample(0.5) }
    }

    fn apply(&mut self, out: &mut [f32]) {
        for sample in out {
            let peak = sample.abs();
            self.envelope = if peak > self.envelope { peak } else { self.envelope * self.keep };
            let wanted = (TARGET / self.envelope.max(1e-9)).clamp(0.063, 15.85);
            self.gain += (wanted - self.gain) * if wanted < self.gain { self.fall } else { self.rise };
            *sample *= self.gain;
        }
    }
}

/// Play a file at this setting. With Auto volume off, also run the B231 gain on the
/// output, and return that run too.
fn play(a: &[f32], b: &[f32], mode: Mode, het_hz: f32, volume: AutoVolume) -> (Run, Run) {
    let mut storage = vec![0.0f32; FS as usize];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(FS, &mut storage, &mut buffers);
    app.set_mode(mode);
    // The wheel tunes HET. In the other modes it changes other settings (the comb's
    // frequency, FD's division, TE's expansion), which stay at their defaults here.
    if mode == Mode::Het {
        app.wheel(((het_hz - 45_000.0) / 500.0) as i32);
    }
    app.set_auto_level_target(TARGET);
    app.set_auto_volume(volume);
    let (mut l, mut r) = ([0.0f32; WINDOW], [0.0f32; WINDOW]);
    let (mut run, mut baseline) = (Run::default(), Run::default());
    let mut old = B231::new();
    for (x, y) in a.chunks_exact(WINDOW).zip(b.chunks_exact(WINDOW)) {
        app.process(x, y, &mut l, &mut r);
        run.add(&l, app.auto_volume_gain(), app.auto_volume_gate());
        if volume == AutoVolume::Off {
            old.apply(&mut l);
            baseline.add(&l, old.gain, 1.0);
        }
    }
    (run, baseline)
}

fn report(name: &str, run: &Run, found: &[Detected], gated: bool) {
    let settle = 3 * 100;
    let windows = run.rms.len();
    let mut near_call = vec![false; windows];
    for d in found {
        let first = (d.event.start_sample as usize).saturating_sub(960) / WINDOW;
        let last = (d.event.end_sample as usize + 3840) / WINDOW;
        for w in near_call.iter_mut().take(last + 1).skip(first) {
            *w = true;
        }
    }
    let (mut quiet_gain, mut noise, mut call_gain) = (Vec::new(), Vec::new(), Vec::new());
    for w in settle.min(windows)..windows {
        if near_call[w] {
            call_gain.push(db(run.gain[w]));
        } else {
            quiet_gain.push(db(run.gain[w]));
            noise.push(db(run.rms[w]));
        }
    }
    let mut call_out = Vec::new();
    for d in found.iter().filter(|d| d.bat || d.kind == CallKind::Single) {
        let (first, last) = (d.event.start_sample as usize / WINDOW, (d.event.end_sample as usize + 960) / WINDOW);
        if first >= settle && last < windows {
            call_out.push(db(run.peak[first..=last].iter().fold(0.0f32, |m, &v| m.max(v))));
        }
    }
    let settled = &run.gain[settle.min(windows)..];
    let moved: f32 = settled.chunks_exact(10).zip(settled.get(10..).unwrap_or(&[]).chunks_exact(10)).map(|(a, b)| db(b[0] / a[0]).abs()).sum::<f32>()
        / ((windows.saturating_sub(settle + 10) / 10).max(1) as f32);
    print!(
        "  {name:<10} gain {:>6.1} ({:>6.1} to {:>6.1})  noise {:>7.1}  gain in call {:>6.1}  call out {:>7.1} {:>7.1} {:>7.1}  moved {moved:>5.2}",
        percentile(&mut quiet_gain, 0.5),
        percentile(&mut quiet_gain.clone(), 0.05),
        percentile(&mut quiet_gain.clone(), 0.95),
        percentile(&mut noise, 0.5),
        percentile(&mut call_gain, 0.5),
        percentile(&mut call_out, 0.1),
        percentile(&mut call_out, 0.5),
        percentile(&mut call_out, 0.9),
    );
    if gated {
        gate_report(run, found);
    }
    println!();
}

fn gate_report(run: &Run, found: &[Detected]) {
    let open = run.gate.iter().filter(|&&g| g > 0.0).count();
    let openings = run.gate.windows(2).filter(|w| w[0] == 0.0 && w[1] > 0.0).count();
    let minutes = run.gate.len() as f32 * WINDOW as f32 / FS / 60.0;
    let heard = |d: &Detected| {
        let (first, last) = (d.event.start_sample as usize / WINDOW, d.event.end_sample as usize / WINDOW);
        last < run.gate.len() && first > 0 && run.gate[first - 1..=last].iter().all(|&g| g > 0.9)
    };
    let bats: Vec<&Detected> = found.iter().filter(|d| d.bat).collect();
    let whole = bats.iter().filter(|d| heard(d)).count();
    let (mut passes, mut before, mut after) = (0, 0, 0);
    let mut i = 0;
    while i < bats.len() {
        let mut k = i;
        while k + 1 < bats.len() && (bats[k + 1].event.start_sample - bats[k].event.start_sample) as f32 / FS <= PASS_GAP_S {
            k += 1;
        }
        let pass = &bats[i..=k];
        let first = pass.iter().position(|d| heard(d)).unwrap_or(pass.len());
        before += first;
        after += pass[first..].iter().filter(|d| !heard(d)).count();
        passes += 1;
        i = k + 1;
    }
    print!(
        "  open {:>3.0}%  {:>4.1} openings/min  bat calls heard {whole}/{}  passes {passes}: missed {:.2} before and {:.2} after the first heard",
        open as f32 * 100.0 / run.gate.len().max(1) as f32,
        openings as f32 / minutes.max(1e-6),
        bats.len(),
        before as f32 / (passes as f32).max(1.0),
        after as f32 / (passes as f32).max(1.0),
    );
}

fn main() {
    let mut modes = vec![Mode::Het, Mode::Comb, Mode::Fd, Mode::Te, Mode::Thru];
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--modes" {
            let list = args.next().expect("a list of modes");
            modes = list
                .split(',')
                .map(|m| match m {
                    "het" => Mode::Het,
                    "comb" => Mode::Comb,
                    "fd" => Mode::Fd,
                    "te" => Mode::Te,
                    _ => Mode::Thru,
                })
                .collect();
        } else {
            paths.push(a);
        }
    }
    for path in &paths {
        let Some((a, b)) = read_stereo(path) else { continue };
        let found = detect(&a, &b);
        let mut bat_peaks: Vec<f32> = found.iter().filter(|d| d.bat).map(|d| d.event.peak_freq_hz).collect();
        let het_hz = if bat_peaks.is_empty() { 45_000.0 } else { (percentile(&mut bat_peaks, 0.5) / 500.0).round() * 500.0 };
        println!(
            "{path}: {:.0} s, {} detections, {} bat calls, HET at {:.1} kHz",
            a.len() as f32 / FS,
            found.len(),
            found.iter().filter(|d| d.bat).count(),
            het_hz / 1000.0
        );
        for &mode in &modes {
            println!(" {}", mode.label());
            let (off, baseline) = play(&a, &b, mode, het_hz, AutoVolume::Off);
            report("Off", &off, &found, false);
            report("B231", &baseline, &found, false);
            report("On", &play(&a, &b, mode, het_hz, AutoVolume::On).0, &found, false);
            report("Bats only", &play(&a, &b, mode, het_hz, AutoVolume::BatsOnly).0, &found, true);
        }
    }
}
