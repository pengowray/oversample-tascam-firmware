//! Run the bat app's call detector and classifier over a folder of Xeno-canto
//! recordings, to see which species the bat-call checks keep and which they
//! miss.
//!
//! cargo run -p bat-app --release --example library -- DIR OUT.csv
//!
//! Each `NAME.wav` with a `NAME.xc.json` beside it (as Oversample's xc-fetch
//! saves them) is mixed to mono, resampled to 192 kHz as the X8 would record
//! it, and run through the same spectrum, detector and classifier settings as
//! the app. Recordings below 192 kHz are listed but not run. One CSV row per
//! recording: its metadata, the detections by kind, and the median peak
//! frequency of all detections and of bat calls.

use std::fmt::Write as _;
use std::path::Path;

use bat_dsp::classify::{CallClassifier, CallKind};
use bat_dsp::detector::CallDetector;
use bat_dsp::spectrum::SpectrumAnalyzer;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

const RATE: u32 = 192_000;

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
    let mono = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok((mono, spec.sample_rate))
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

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// `AUDIBLE=LO,HI` (Hz) overrides the detector's audible band, for tuning.
fn audible_band_from_env() -> Option<(f32, f32)> {
    let v = std::env::var("AUDIBLE").ok()?;
    let (a, b) = v.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[1]);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".wav"))
        .collect();
    names.sort();

    let mut csv = String::from(
        "xc_id,group,genus,species,english,country,type,quality,sample_rate,seconds,status,\
         detections,bat,tone,broadband,too_low,audible,single,median_peak_khz,median_bat_peak_khz\n",
    );
    let fs = RATE as f32;
    for name in &names {
        let stem = name.trim_end_matches(".wav");
        let Ok(text) = std::fs::read_to_string(dir.join(format!("{stem}.xc.json"))) else { continue };
        let j: serde_json::Value = serde_json::from_str(&text).expect("json");
        let field = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").replace(',', ";");
        let mut row = format!(
            "{},{},{},{},{},{},{},{},",
            j.get("xc_id").map(|v| v.to_string()).unwrap_or_default().trim_matches('"'),
            field("grp"),
            field("gen"),
            field("sp"),
            field("en"),
            field("cnt"),
            field("type"),
            field("q"),
        );
        let (mono, rate) = match read_mono(&dir.join(name)) {
            Ok(x) => x,
            Err(e) => {
                let _ = writeln!(row, ",,error: {}", e.replace(',', ";"));
                csv.push_str(&row);
                continue;
            }
        };
        let _ = write!(row, "{rate},{:.1},", mono.len() as f32 / rate as f32);
        if rate < RATE {
            let _ = writeln!(row, "below 192 kHz");
            csv.push_str(&row);
            continue;
        }
        let x = match resample(mono, rate) {
            Ok(x) => x,
            Err(e) => {
                let _ = writeln!(row, "error: {}", e.replace(',', ";"));
                csv.push_str(&row);
                continue;
            }
        };

        let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(fs);
        analyzer.set_hop(128);
        let mut detector: CallDetector<512> = CallDetector::new(fs);
    if let Some((lo, hi)) = audible_band_from_env() {
        detector.set_audible_band(lo, hi);
    }
        let mut events = Vec::new();
        for block in x.chunks(4096) {
            analyzer.push(block, |col| events.extend(detector.process(col)));
        }
        events.extend(detector.flush());
        if let Ok(dump) = std::env::var("EVENTS_DIR") {
            let mut out = String::from("start_s,length_ms,peak_khz,min_khz,max_khz,db,audible_db\n");
            for e in &events {
                let _ = writeln!(
                    out,
                    "{:.4},{:.2},{:.2},{:.2},{:.2},{:.1},{:.1}",
                    e.start_sample as f32 / fs,
                    (e.end_sample - e.start_sample) as f32 / fs * 1000.0,
                    e.peak_freq_hz / 1000.0,
                    e.min_freq_hz / 1000.0,
                    e.max_freq_hz / 1000.0,
                    e.peak_db_above_floor,
                    e.audible_db_above_floor
                );
            }
            let id = j.get("xc_id").map(|v| v.to_string()).unwrap_or_default();
            std::fs::write(Path::new(&dump).join(format!("{}.csv", id.trim_matches('"'))), out).expect("write events");
        }
        let mut classifier = CallClassifier::new(fs, detector.band().0);
        let (mut bat, mut tone, mut broadband, mut too_low, mut audible, mut single) = (0, 0, 0, 0, 0, 0i64);
        let mut bat_peaks = Vec::new();
        for e in &events {
            let v = classifier.classify(e);
            match v.kind {
                CallKind::Bat => {
                    bat += 1;
                    bat_peaks.push(e.peak_freq_hz / 1000.0);
                }
                CallKind::Tone => tone += 1,
                CallKind::Broadband => broadband += 1,
                CallKind::TooLow => too_low += 1,
                CallKind::Audible => audible += 1,
                CallKind::Single => single += 1,
            }
            if let Some(first) = v.earlier {
                bat += 1;
                single -= 1;
                bat_peaks.push(first.peak_freq_hz / 1000.0);
            }
        }
        let peaks = events.iter().map(|e| e.peak_freq_hz / 1000.0).collect();
        let _ = writeln!(
            row,
            "ok,{},{bat},{tone},{broadband},{too_low},{audible},{single},{:.1},{:.1}",
            events.len(),
            median(peaks),
            median(bat_peaks)
        );
        eprint!(".");
        csv.push_str(&row);
    }
    eprintln!();
    std::fs::write(&args[2], csv).expect("write csv");
}
