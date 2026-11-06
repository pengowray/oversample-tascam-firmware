//! Compare the call detector at different FFT hops, on recordings and on
//! synthetic calls, to choose the hop.
//!
//! cargo run -p bat-app --release --example hop_compare -- --hops 128,192,256 [--csv OUT.csv] FILES...
//! cargo run -p bat-app --release --example hop_compare -- --hops 128,192,256 --synthetic
//!
//! A hop written as 128+64 starts its columns 64 samples later. Comparing it
//! with plain 128 shows how many detections change only because the columns
//! fall at different moments, which is the noise in any comparison of hops.
//!
//! Each file is mixed to mono and resampled to 192 kHz as the X8 would record
//! it. Files below 192 kHz are skipped. The first hop is the reference: a
//! detection at another hop matches a reference detection when their times
//! overlap, give or take one column at each hop. "missed" counts reference
//! detections with no match, and "new" counts detections with no match in the
//! reference.
//!
//! The synthetic run plays trains of calls of known length and level in noise,
//! so it counts true misses: a call is found when a detection overlaps it, and
//! a detection overlapping no call is invented.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use bat_dsp::classify::{CallClassifier, CallKind};
use bat_dsp::detector::{CallDetector, CallEvent};
use bat_dsp::spectrum::SpectrumAnalyzer;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

const RATE: u32 = 192_000;
const FS: f32 = RATE as f32;

fn read_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.into_samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    Ok((samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect(), spec.sample_rate))
}

fn resample(mono: Vec<f32>, from: u32) -> Result<Vec<f32>, String> {
    if from == RATE {
        return Ok(mono);
    }
    let len = mono.len();
    let mut r = Fft::<f32>::new(from as usize, RATE as usize, 4096, 1, FixedSync::Both).map_err(|e| e.to_string())?;
    let input = [mono];
    let adapter = SequentialSliceOfVecs::new(&input[..], 1, len).map_err(|e| e.to_string())?;
    Ok(r.process_all(&adapter, len, None).map_err(|e| e.to_string())?.take_data())
}

/// A detection, with its kind, and whether it counts as a bat call: a train's
/// first call is a bat call once the second arrives.
#[derive(Clone, Copy)]
struct Found {
    event: CallEvent,
    kind: CallKind,
    bat: bool,
}

/// A hop, and how many samples later its columns start.
#[derive(Clone, Copy)]
struct Grid {
    hop: usize,
    offset: usize,
}

impl Grid {
    fn parse(text: &str) -> Self {
        let (hop, offset) = text.split_once('+').unwrap_or((text, "0"));
        Self { hop: hop.parse().expect("a hop"), offset: offset.parse().expect("an offset") }
    }
}

impl std::fmt::Display for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.offset == 0 { write!(f, "{}", self.hop) } else { write!(f, "{}+{}", self.hop, self.offset) }
    }
}

fn detect(x: &[f32], grid: Grid) -> Vec<Found> {
    let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(FS);
    analyzer.set_hop(grid.hop);
    let mut detector: CallDetector<512> = CallDetector::new(FS);
    let mut events = Vec::new();
    for block in x[grid.offset.min(x.len())..].chunks(256) {
        analyzer.push(block, |col| events.extend(detector.process(col)));
    }
    events.extend(detector.flush());
    for e in &mut events {
        e.start_sample += grid.offset as u64;
        e.end_sample += grid.offset as u64;
    }
    let (band_lo, _) = detector.band();
    let mut classifier = CallClassifier::new(FS, band_lo);
    let mut found: Vec<Found> = Vec::with_capacity(events.len());
    for &event in &events {
        let verdict = classifier.classify(&event);
        if let Some(first) = verdict.earlier
            && let Some(f) = found.iter_mut().rev().find(|f| f.event.start_sample == first.start_sample)
        {
            f.bat = true;
        }
        found.push(Found { event, kind: verdict.kind, bat: verdict.kind == CallKind::Bat });
    }
    found
}

fn length_ms(e: &CallEvent) -> f32 {
    (e.end_sample - e.start_sample) as f32 / FS * 1000.0
}

/// Whether two detections are the same sound: their times overlap, allowing
/// for the column spacing of both.
fn same_sound(a: &CallEvent, b: &CallEvent, slack: u64) -> bool {
    a.start_sample <= b.end_sample + slack && b.start_sample <= a.end_sample + slack
}

/// For each detection in `a`, whether some detection in `b` is the same sound.
fn matched(a: &[Found], b: &[Found], slack: u64) -> Vec<bool> {
    let mut j = 0;
    a.iter()
        .map(|x| {
            while j < b.len() && b[j].event.end_sample + slack < x.event.start_sample {
                j += 1;
            }
            b[j..].iter().take_while(|y| y.event.start_sample <= x.event.end_sample + slack).any(|y| same_sound(&x.event, &y.event, slack))
        })
        .collect()
}

#[derive(Default, Clone)]
struct Totals {
    seconds: f32,
    detections: usize,
    bats: usize,
    kinds: BTreeMap<String, usize>,
    /// Detections shorter than 2 ms, and from 2 to 5 ms.
    short: usize,
    medium: usize,
    missed: usize,
    new: usize,
    bats_missed: usize,
    bats_new: usize,
    short_missed: usize,
    medium_missed: usize,
    short_bats: usize,
    short_bats_missed: usize,
    peak_error_khz: Vec<f32>,
    length_error_ms: Vec<f32>,
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

fn add_file(t: &mut Totals, reference: &[Found], found: &[Found], ref_hop: usize, hop: usize, seconds: f32) {
    let slack = (ref_hop + hop) as u64;
    t.seconds += seconds;
    t.detections += found.len();
    t.bats += found.iter().filter(|f| f.bat).count();
    for f in found {
        let kind = if f.bat { "Bat".to_string() } else { format!("{:?}", f.kind) };
        *t.kinds.entry(kind).or_insert(0) += 1;
    }
    t.short += found.iter().filter(|f| length_ms(&f.event) < 2.0).count();
    t.medium += found.iter().filter(|f| (2.0..5.0).contains(&length_ms(&f.event))).count();

    let found_in_reference = matched(reference, found, slack);
    let reference_in_found = matched(found, reference, slack);
    t.missed += found_in_reference.iter().filter(|m| !**m).count();
    t.new += reference_in_found.iter().filter(|m| !**m).count();
    for (r, &m) in reference.iter().zip(&found_in_reference) {
        let len = length_ms(&r.event);
        if !m && len < 2.0 {
            t.short_missed += 1;
        }
        if !m && (2.0..5.0).contains(&len) {
            t.medium_missed += 1;
        }
    }

    // The same comparison for bat calls only.
    let bats = |list: &[Found]| -> Vec<Found> { list.iter().filter(|f| f.bat).copied().collect() };
    let (ref_bats, bats_found) = (bats(reference), bats(found));
    let bats_found_in_ref = matched(&ref_bats, &bats_found, slack);
    let ref_in_bats_found = matched(&bats_found, &ref_bats, slack);
    t.bats_missed += bats_found_in_ref.iter().filter(|m| !**m).count();
    t.bats_new += ref_in_bats_found.iter().filter(|m| !**m).count();
    for (r, &m) in ref_bats.iter().zip(&bats_found_in_ref) {
        if length_ms(&r.event) < 2.0 {
            t.short_bats += 1;
            if !m {
                t.short_bats_missed += 1;
            }
        }
    }
    for r in &ref_bats {
        if let Some(f) = bats_found.iter().find(|f| same_sound(&r.event, &f.event, slack)) {
            t.peak_error_khz.push((r.event.peak_freq_hz - f.event.peak_freq_hz).abs() / 1000.0);
            t.length_error_ms.push((length_ms(&r.event) - length_ms(&f.event)).abs());
        }
    }
}

fn print_totals(hops: &[Grid], totals: &mut [Totals]) {
    println!(
        "{:>7} {:>9} {:>7} {:>7} {:>6} {:>7} {:>7} {:>8} {:>8} {:>7} {:>7}  by kind",
        "hop", "detected", "<2 ms", "2-5 ms", "missed", "new", "bats", "bat miss", "bat new", "|dkHz|", "|dms|"
    );
    for (&hop, t) in hops.iter().zip(totals.iter_mut()) {
        println!(
            "{:>7} {:>9} {:>7} {:>7} {:>6} {:>7} {:>7} {:>8} {:>8} {:>7.2} {:>7.2}  {:?}",
            hop.to_string(),
            t.detections,
            t.short,
            t.medium,
            t.missed,
            t.new,
            t.bats,
            t.bats_missed,
            t.bats_new,
            median(&mut t.peak_error_khz),
            median(&mut t.length_error_ms),
            t.kinds,
        );
        println!(
            "      of the reference's calls: under 2 ms {} missed, 2 to 5 ms {} missed; bat calls under 2 ms {} of {} missed",
            t.short_missed, t.medium_missed, t.short_bats_missed, t.short_bats
        );
    }
}

fn files(hops: &[Grid], paths: &[String], csv: Option<&String>) {
    let mut totals = vec![Totals::default(); hops.len()];
    let mut rows = String::from("file,seconds");
    for h in hops {
        let _ = write!(rows, ",detected_{h},bats_{h},missed_{h},new_{h},bats_missed_{h},bats_new_{h}");
    }
    rows.push('\n');
    let mut count = 0;
    for path in paths {
        let path = Path::new(path);
        let (mono, from) = match read_mono(path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipped {}: {e}", path.display());
                continue;
            }
        };
        if from < RATE || mono.len() < RATE as usize / 2 {
            eprintln!("skipped {}: {from} Hz, {} samples", path.display(), mono.len());
            continue;
        }
        let Ok(x) = resample(mono, from) else {
            eprintln!("skipped {}: could not resample", path.display());
            continue;
        };
        count += 1;
        let seconds = x.len() as f32 / FS;
        let results: Vec<Vec<Found>> = hops.iter().map(|&h| detect(&x, h)).collect();
        let _ = write!(rows, "\"{}\",{seconds:.1}", path.file_name().map_or("", |n| n.to_str().unwrap_or("")));
        for (i, &h) in hops.iter().enumerate() {
            let mut one = Totals::default();
            add_file(&mut one, &results[0], &results[i], hops[0].hop, h.hop, seconds);
            add_file(&mut totals[i], &results[0], &results[i], hops[0].hop, h.hop, seconds);
            let _ = write!(rows, ",{},{},{},{},{},{}", one.detections, one.bats, one.missed, one.new, one.bats_missed, one.bats_new);
        }
        rows.push('\n');
    }
    println!("{count} recordings, {:.0} s, reference hop {}", totals[0].seconds, hops[0]);
    print_totals(hops, &mut totals);
    if let Some(path) = csv {
        std::fs::write(path, rows).expect("write csv");
    }
}

fn noise(amp: f32, n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            amp * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

/// A call from `f0` to `f1` Hz with a Hann envelope, `n` samples long.
fn chirp(f0: f32, f1: f32, amp: f32, n: usize) -> Vec<f32> {
    let dur = n as f64 / FS as f64;
    let k = (f1 as f64 - f0 as f64) / dur;
    (0..n)
        .map(|i| {
            let t = i as f64 / FS as f64;
            let w = (std::f64::consts::PI * i as f64 / n as f64).sin();
            (amp as f64 * w * w * (std::f64::consts::TAU * (f0 as f64 * t + 0.5 * k * t * t)).sin()) as f32
        })
        .collect()
}

/// Calls of each length and level, in trains with random offsets from the hop
/// grid, in noise. Prints the share of calls detected, and of calls classified
/// as bat calls, at each hop.
fn synthetic(hops: &[Grid]) {
    let lengths_ms = [1.0f32, 1.5, 2.0, 3.0, 5.0, 10.0];
    // Amplitude of the call against noise of amplitude 0.003, in dB.
    let levels_db = [12.0f32, 15.0, 18.0, 21.0, 24.0, 30.0, 40.0];
    let shapes: [(&str, f32, f32); 2] = [("steep FM 80 to 40 kHz", 80_000.0, 40_000.0), ("shallow FM 50 to 45 kHz", 50_000.0, 45_000.0)];
    for (name, f0, f1) in shapes {
        println!("{name}: share of calls detected (and classified as bat calls), 200 calls each");
        print!("{:>6} {:>7}", "ms", "dB");
        for h in hops {
            print!("{:>16}", format!("hop {h}"));
        }
        println!();
        let mut invented = vec![0usize; hops.len()];
        for &len_ms in &lengths_ms {
            for &level_db in &levels_db {
                let amp = 0.003 * 10f32.powf(level_db / 20.0);
                let n = (len_ms * 0.001 * FS) as usize;
                let count = 200;
                // 80 ms spacing with a varying offset, after 0.5 s to settle.
                let start = RATE as usize / 2;
                let spacing = RATE as usize * 80 / 1000;
                let mut x = noise(0.003, start + (count + 1) * spacing, 12345);
                let call = chirp(f0, f1, amp, n);
                let mut starts = Vec::with_capacity(count);
                let mut r = 99u32;
                for i in 0..count {
                    r ^= r << 13;
                    r ^= r >> 17;
                    r ^= r << 5;
                    let at = start + i * spacing + (r % 512) as usize;
                    starts.push(at);
                    for (v, c) in x[at..].iter_mut().zip(&call) {
                        *v += c;
                    }
                }
                print!("{len_ms:>6} {level_db:>7}");
                for (hi, &hop) in hops.iter().enumerate() {
                    let found = detect(&x, hop);
                    let near = |f: &Found, at: usize| {
                        // Allow the half window either side that spreads a call in time.
                        f.event.start_sample <= (at + n + 300) as u64 && f.event.end_sample + 300 >= at as u64
                    };
                    let detected = starts.iter().filter(|&&at| found.iter().any(|f| near(f, at))).count();
                    let bats = starts.iter().filter(|&&at| found.iter().any(|f| f.bat && near(f, at))).count();
                    invented[hi] += found.iter().filter(|f| !starts.iter().any(|&at| near(f, at))).count();
                    print!("{:>16}", format!("{:>3}% ({:>3}%)", detected * 100 / count, bats * 100 / count));
                }
                println!();
            }
        }
        print!("detections on no call, in all runs:");
        for (h, n) in hops.iter().zip(&invented) {
            print!("  hop {h}: {n}");
        }
        println!();
        println!();
    }
}

fn main() {
    let mut hops = vec![Grid::parse("128"), Grid::parse("256")];
    let mut csv = None;
    let mut paths = Vec::new();
    let mut synthetic_run = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--hops" => hops = args.next().expect("hops").split(',').map(Grid::parse).collect(),
            "--csv" => csv = args.next(),
            "--synthetic" => synthetic_run = true,
            _ => paths.push(a),
        }
    }
    if synthetic_run {
        synthetic(&hops);
    } else {
        files(&hops, &paths, csv.as_ref());
    }
}
