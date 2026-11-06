//! List the calls the bat app's detector finds in a WAV file, with the same
//! spectrum and detector settings as the app, for tuning on real recordings.
//!
//! cargo run -p bat-app --release --example calls -- IN.wav [FROM_S] [TO_S]
//!
//! Prints one line per call between FROM_S and TO_S (default: the whole
//! file), then a summary. A call whose range reaches 25 kHz or more below its
//! peak is marked "wide": usually a separate sound at the same moment.
//! `HOP=128` uses another FFT hop in place of the app's.

use bat_dsp::classify::{CallClassifier, CallKind};
use bat_dsp::detector::CallDetector;
use bat_dsp::spectrum::SpectrumAnalyzer;

/// `AUDIBLE=LO,HI` (Hz) overrides the detector's audible band, for tuning.
fn audible_band_from_env() -> Option<(f32, f32)> {
    let v = std::env::var("AUDIBLE").ok()?;
    let (a, b) = v.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut reader = hound::WavReader::open(&args[1]).expect("open wav");
    let spec = reader.spec();
    let fs = spec.sample_rate as f32;
    let ch = spec.channels as usize;
    let from: f32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(0.0);
    let to: f32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(f32::MAX);

    let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(fs);
    let hop = std::env::var("HOP").map_or(bat_app::FFT_HOP, |h| h.parse().expect("a hop"));
    analyzer.set_hop(hop);
    let mut detector: CallDetector<512> = CallDetector::new(fs);
    if let Some((lo, hi)) = audible_band_from_env() {
        detector.set_audible_band(lo, hi);
    }
    let mut events = Vec::new();
    let mut mono = Vec::with_capacity(4096);
    let mut frame = Vec::with_capacity(ch);
    let mut samples: Box<dyn Iterator<Item = f32>> = match spec.sample_format {
        hound::SampleFormat::Float => Box::new(reader.samples::<f32>().map(Result::unwrap)),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            Box::new(reader.samples::<i32>().map(move |s| s.unwrap() as f32 * scale))
        }
    };
    loop {
        mono.clear();
        while mono.len() < 4096 {
            frame.clear();
            frame.extend(samples.by_ref().take(ch));
            if frame.len() < ch {
                break;
            }
            // The app's default: inputs 3 and 4 mixed.
            mono.push(frame.iter().sum::<f32>() / ch as f32);
        }
        if mono.is_empty() {
            break;
        }
        analyzer.push(&mono, |col| events.extend(detector.process(col)));
        if mono.len() < 4096 {
            break;
        }
    }
    events.extend(detector.flush());

    let s = |x: u64| x as f32 / fs;
    if let Ok(path) = std::env::var("CSV") {
        use std::io::Write;
        let mut f = std::fs::File::create(path).expect("create csv");
        writeln!(f, "start_s,length_ms,peak_khz,min_khz,max_khz,db,audible_db").unwrap();
        for e in &events {
            writeln!(
                f,
                "{:.4},{:.2},{:.2},{:.2},{:.2},{:.1},{:.1}",
                s(e.start_sample),
                (s(e.end_sample) - s(e.start_sample)) * 1000.0,
                e.peak_freq_hz / 1000.0,
                e.min_freq_hz / 1000.0,
                e.max_freq_hz / 1000.0,
                e.peak_db_above_floor,
                e.audible_db_above_floor
            )
            .unwrap();
        }
    }
    let mut wide = 0;
    let mut shown = 0;
    for e in &events {
        let is_wide = e.peak_freq_hz - e.min_freq_hz >= 25_000.0 || e.max_freq_hz - e.peak_freq_hz >= 25_000.0;
        wide += is_wide as usize;
        let t = s(e.start_sample);
        if t >= from && t <= to {
            shown += 1;
            println!(
                "{:8.3} s  {:5.1} ms  peak {:5.1} kHz  range {:4.0}-{:4.0} kHz  {:4.1} dB{}",
                t,
                (s(e.end_sample) - t) * 1000.0,
                e.peak_freq_hz / 1000.0,
                e.min_freq_hz / 1000.0,
                e.max_freq_hz / 1000.0,
                e.peak_db_above_floor,
                if is_wide { "  wide" } else { "" }
            );
        }
    }
    println!("{} calls in the file, {} shown, {} wide", events.len(), shown, wide);

    // Sort the detections as the bat app does.
    let (band_lo, _) = detector.band();
    let mut classifier = CallClassifier::new(fs, band_lo);
    let mut kinds: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    let mut bats: Vec<u64> = Vec::new();
    for e in &events {
        let v = classifier.classify(e);
        *kinds.entry(format!("{:?}", v.kind)).or_insert(0) += 1;
        if let Some(first) = v.earlier {
            bats.push(first.start_sample);
        }
        if v.kind == CallKind::Bat {
            bats.push(e.start_sample);
        }
    }
    // A train's first call is reported as Single, then handed back as a bat
    // call when the second arrives.
    let handed_back = bats.len() as i64 - kinds.get("Bat").copied().unwrap_or(0);
    *kinds.entry("Single".to_string()).or_insert(0) -= handed_back;
    kinds.insert("Bat".to_string(), bats.len() as i64);
    println!("by kind: {kinds:?}");
    let minutes = s(events.last().map_or(0, |e| e.end_sample)) / 60.0;
    println!("bat calls: {} ({:.1} a minute)", bats.len(), bats.len() as f32 / minutes.max(1e-6));
    if let Ok(path) = std::env::var("BATS") {
        let list: Vec<String> = bats.iter().map(|&t| format!("{:.4}", s(t))).collect();
        std::fs::write(path, list.join("\n")).expect("write bats");
    }

    // How the calls spread by peak frequency and by length.
    let bands = [(0.0, 20.0), (20.0, 30.0), (30.0, 40.0), (40.0, 60.0), (60.0, 200.0)];
    let lengths = [(0.0, 2.0), (2.0, 5.0), (5.0, 20.0), (20.0, 1000.0)];
    print!("{:>12}", "peak kHz");
    for (a, b) in lengths {
        print!("{:>12}", format!("{a}-{b} ms"));
    }
    println!();
    for (lo, hi) in bands {
        print!("{:>12}", format!("{lo}-{hi}"));
        for (a, b) in lengths {
            let n = events
                .iter()
                .filter(|e| {
                    let f = e.peak_freq_hz / 1000.0;
                    let ms = (s(e.end_sample) - s(e.start_sample)) * 1000.0;
                    f >= lo && f < hi && ms >= a && ms < b
                })
                .count();
            print!("{n:>12}");
        }
        println!();
    }
}
