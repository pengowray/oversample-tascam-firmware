//! Time the bat app's processing and drawing on this machine, per second of
//! 192 kHz audio: each stage on its own, then the whole app in each listening
//! mode. The X8 is far slower and its memory is a different shape, so the times
//! are not X8 times, but the shares show where the time goes.
//!
//! cargo run -p bat-app --release --example bench [-- IN.wav]
//!
//! The X8's processor has no SIMD, so its shares are closer to a build without
//! auto-vectorisation:
//!
//! RUSTFLAGS="-C no-vectorize-loops -C no-vectorize-slp" cargo run -p bat-app --release --example bench
//!
//! Without IN.wav it plays noise with ten 5 ms calls a second. `HOPS=128,192`
//! chooses the FFT hops for the stage table. The last table has the app with one
//! chain against two, on two different signals: the file's first two channels, or the
//! noise and the same noise backwards. The stages that the app does not
//! make public (the waterfall and snapshot writes, the meters) are copied here,
//! so keep them in step with `BatApp::analyse`. The stages add up to about the
//! whole app's analysis, as the two tables show.

use std::hint::black_box;
use std::time::Instant;

use bat_app::{AutoVolume, BatApp, Buffers, FFT_HOP, HEIGHT, Mode, MonitorSource, Rgb565Buffer, Setting, WIDTH};
use bat_dsp::classify::CallClassifier;
use bat_dsp::detector::{CallDetector, DEFAULT_BAND_LO_HZ};
use bat_dsp::freq_div::FrequencyDivider;
use bat_dsp::heterodyne::{DEFAULT_COMB_SPACING_HZ, HetComb, Heterodyne};
use bat_dsp::pitch_shift::PitchShifter;
use bat_dsp::spectrum::{Column, SpectrumAnalyzer, power_to_db};
use bat_dsp::time_expansion::TimeExpander;

const FS: f32 = 192_000.0;
const N: usize = 512;
const BINS: usize = N / 2 + 1;
const WF_H: usize = 211;
const WF_W: usize = 288;
const WF_COLUMN_SAMPLES: usize = 1920;
const CHUNK: usize = 256;
const VIEW_HI: f32 = 96_000.0;
const RUNS: usize = 5;
const STEREO_RUNS: usize = 15;

/// The shortest of some runs, which is the one least disturbed by whatever else
/// this machine is doing.
fn best_of(runs: usize, mut f: impl FnMut()) -> f64 {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

fn load(path: Option<&String>) -> Vec<f32> {
    if let Some(path) = path {
        let mut reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        let ch = spec.channels as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
            }
        };
        return samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    }
    let n = FS as usize * 5;
    let mut s = 12345u32;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = 0.003 * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0);
            let t = i as f32 / FS;
            let call = if (t * 10.0).fract() < 0.005 { 0.1 * (std::f32::consts::TAU * 45_000.0 * t).sin() } else { 0.0 };
            noise + call
        })
        .collect()
}

/// Two different signals of the same length, for the table of one chain against two.
fn load_two(path: Option<&String>, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
    if let Some(path) = path {
        let mut reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        let ch = spec.channels as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
            }
        };
        let side = |c: usize| samples.chunks(ch).map(|f| f[c.min(ch - 1)]).collect();
        return (side(0), side(1));
    }
    (x.to_vec(), x.iter().rev().copied().collect())
}

fn level(db: f32, floor_db: f32, scale: f32) -> u8 {
    ((db - floor_db) * scale).clamp(0.0, 255.0) as u8
}

/// The analysis stages at one hop, in ms per second of audio.
fn stages(x: &[f32], hop: usize) -> Vec<(&'static str, f64)> {
    let seconds = x.len() as f64 / FS as f64;
    let per_s = |t: f64| t * 1000.0 / seconds;
    let mut out = Vec::new();

    let mut an = SpectrumAnalyzer::<N>::new(FS);
    an.set_hop(hop);
    let total = best_of(RUNS, || {
        for chunk in x.chunks(CHUNK) {
            an.push(chunk, |c| {
                black_box(c.db[0]);
            });
        }
    });
    let mut an = SpectrumAnalyzer::<N>::new(FS);
    an.set_hop(hop);
    let mut cols: Vec<(u64, Vec<f32>)> = Vec::new();
    for chunk in x.chunks(CHUNK) {
        an.push(chunk, |c| cols.push((c.centre_sample, c.db.to_vec())));
    }
    let bin_hz = FS / N as f32;
    let powers: Vec<Vec<f32>> = cols.iter().map(|(_, db)| db.iter().map(|&d| 10f32.powf(d / 10.0)).collect()).collect();
    let mut db_out = [0.0f32; BINS];
    let to_db = best_of(RUNS, || {
        for p in &powers {
            for (o, &v) in db_out.iter_mut().zip(p) {
                *o = power_to_db(v);
            }
            black_box(&mut db_out);
        }
    });
    out.push(("window and FFT", per_s(total - to_db)));
    out.push(("power to dB", per_s(to_db)));

    let columns: Vec<Column<'_>> = cols.iter().map(|(t, db)| Column { centre_sample: *t, bin_hz, db }).collect();

    let mut bin_max = [f32::MIN; BINS];
    let hold = best_of(RUNS, || {
        for col in &columns {
            for (m, &db) in bin_max.iter_mut().zip(col.db) {
                *m = m.max(db);
            }
            black_box(&mut bin_max);
        }
    });
    out.push(("loudest level per bin, every FFT column", per_s(hold)));

    let mut wf = vec![0u8; WF_W * WF_H];
    let mut row_floor = [-110.0f32; WF_H];
    let mut bin_floor = [-107.0f32; BINS];
    let bin_row: Vec<u8> = (0..BINS)
        .map(|k| {
            let t = ((VIEW_HI - k as f32 * bin_hz) / VIEW_HI).clamp(0.0, 1.0);
            ((t * WF_H as f32) as usize).min(WF_H - 1) as u8
        })
        .collect();
    let scale = 255.0 / 45.0;
    let wf_columns = x.len() / WF_COLUMN_SAMPLES;
    let finish = best_of(RUNS, || {
        for c in 0..wf_columns {
            let mut pixels = [0.0f32; WF_H];
            Column { centre_sample: 0, bin_hz, db: &bin_max[..] }.to_pixels(VIEW_HI, 0.0, &mut pixels);
            for (f, &a) in row_floor.iter_mut().zip(&pixels) {
                *f = (*f + 0.02 * (a - *f)).max(-140.0);
            }
            let mean = row_floor.iter().sum::<f32>() / WF_H as f32;
            let floors = [mean + 3.0; WF_H];
            let base = (c % WF_W) * WF_H;
            for ((dst, &db), &floor) in wf[base..base + WF_H].iter_mut().zip(&pixels).zip(&floors) {
                *dst = level(db, floor, scale).min(254);
            }
            for (f, &row) in bin_floor.iter_mut().zip(&bin_row) {
                *f = floors[row as usize];
            }
            black_box(&mut wf);
            black_box(&mut bin_floor);
        }
    });
    out.push(("waterfall column, every 10 ms", per_s(finish)));

    let history_len = 192;
    let mut history = vec![0u8; history_len * BINS];
    let hist = best_of(RUNS, || {
        let mut next = 0;
        for col in &columns {
            let base = next * BINS;
            for ((dst, &db), &floor) in history[base..base + BINS].iter_mut().zip(col.db).zip(&bin_floor) {
                *dst = level(db, floor, scale);
            }
            next = (next + 1) % history_len;
            black_box(&mut history);
        }
    });
    out.push(("snapshot history, every FFT column", per_s(hist)));

    let mut events = Vec::new();
    let det = best_of(RUNS, || {
        let mut d = CallDetector::<N>::new(FS);
        events.clear();
        for col in &columns {
            if let Some(e) = d.process(col) {
                events.push(e);
            }
        }
        black_box(&events);
    });
    out.push(("detector", per_s(det)));

    let classify = best_of(RUNS, || {
        let mut c = CallClassifier::new(FS, DEFAULT_BAND_LO_HZ);
        for e in &events {
            black_box(c.classify(e));
        }
    });
    out.push(("classifier", per_s(classify)));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let x = load(args.get(1));
    let seconds = x.len() as f64 / FS as f64;
    let per_s = |t: f64| t * 1000.0 / seconds;
    println!("Times on this machine, in ms per second of 192 kHz audio, from {seconds:.1} s of audio. Not X8 times.");
    println!();

    let hops: Vec<usize> = std::env::var("HOPS")
        .unwrap_or_else(|_| format!("128,{FFT_HOP}"))
        .split(',')
        .map(|h| h.parse().expect("a hop"))
        .collect();
    let table: Vec<Vec<(&str, f64)>> = hops.iter().map(|&h| stages(&x, h)).collect();
    print!("{:<42}", "analysis stage");
    for h in &hops {
        print!("{:>10}", format!("hop {h}"));
    }
    println!();
    for i in 0..table[0].len() {
        print!("{:<42}", table[0][i].0);
        for t in &table {
            print!("{:>10.2}", t[i].1);
        }
        println!();
    }
    print!("{:<42}", "sum of the stages");
    for t in &table {
        print!("{:>10.2}", t.iter().map(|s| s.1).sum::<f64>());
    }
    println!();
    println!();

    // Each mode's DSP on its own.
    let mut y = vec![0.0f32; CHUNK];
    let run = |process: &mut dyn FnMut(&[f32], &mut [f32]), y: &mut [f32]| {
        best_of(RUNS, || {
            for chunk in x.chunks(CHUNK) {
                process(chunk, &mut y[..chunk.len()]);
                black_box(&mut *y);
            }
        })
    };
    let line = |name: &str, t: f64| println!("{:<42}{:>10.2}", name, per_s(t));
    let mut het = Heterodyne::new(FS, 45_000.0);
    line("mode DSP: HET", run(&mut |a, b| het.process(a, b), &mut y));
    let mut comb: HetComb<3> = HetComb::new(FS, 25_000.0, DEFAULT_COMB_SPACING_HZ, 3);
    line("mode DSP: COMB", run(&mut |a, b| comb.process(a, b), &mut y));
    let mut fd = FrequencyDivider::new(FS);
    line("mode DSP: FD", run(&mut |a, b| fd.process(a, b), &mut y));
    let mut storage = vec![0.0f32; 96_000];
    let mut te = TimeExpander::new(FS, &mut storage);
    line("TE recording, which runs in every mode", run(&mut |a, b| te.process(a, b), &mut y));
    {
        let mut storage = vec![0.0f32; 96_000];
        let mut te = TimeExpander::new(FS, &mut storage);
        let n = x.len().min(96_000);
        te.process(&x[..n], &mut vec![0.0; n]);
        te.set_post_ms(0.0);
        te.set_pre_ms(300.0);
        te.trigger();
        line("TE playing", run(&mut |a, b| te.process(a, b), &mut y));
    }
    let mut pitch = Box::new(PitchShifter::unconfigured());
    pitch.init(FS);
    line("mode DSP: PITCH", run(&mut |a, b| pitch.process(a, b), &mut y));
    line("mode DSP: 1:1 (a copy)", run(&mut |a, b| b.copy_from_slice(a), &mut y));

    // Two peaks and two logs per chunk, as the meters do.
    line(
        "meters",
        best_of(RUNS, || {
            for chunk in x.chunks(CHUNK) {
                for _ in 0..2 {
                    let peak = chunk.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    black_box(20.0 * libm::log10f(peak.max(1e-12)));
                }
            }
        }),
    );
    line(
        "mix to mono",
        best_of(RUNS, || {
            for chunk in x.chunks(CHUNK) {
                for ((m, &a), &b) in y.iter_mut().zip(chunk).zip(chunk) {
                    *m = 0.5 * (a + b);
                }
                black_box(&mut y);
            }
        }),
    );
    println!();

    // The whole app, with and without its analysis.
    println!("{:<10}{:>16}{:>18}{:>16}", "mode", "process", "without analysis", "draw ms/frame");
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    for mode in Mode::ALL {
        let mut storage = vec![0.0f32; 96_000];
        let mut buffers = Box::new(Buffers::new());
        let mut app = BatApp::new(FS, &mut storage, &mut buffers);
        app.set_mode(mode);
        let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
        let mut whole = |app: &mut BatApp, skip: bool| {
            app.set_skip_analysis(skip);
            best_of(RUNS, || {
                for chunk in x.chunks(1024) {
                    let k = chunk.len();
                    app.process(chunk, chunk, &mut l[..k], &mut r[..k]);
                }
            })
        };
        let process = whole(&mut app, false);
        let bare = whole(&mut app, true);
        let draw = best_of(RUNS, || {
            for _ in 0..50 {
                app.render(&mut Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT));
            }
        }) / 50.0;
        println!("{:<10}{:>16.2}{:>18.2}{:>16.3}", mode.label(), per_s(process), per_s(bare), draw * 1000.0);
    }

    // Auto level: the difference with it on, in 1:1.
    let mut storage = vec![0.0f32; 96_000];
    let mut buffers = Box::new(Buffers::new());
    let mut app = BatApp::new(FS, &mut storage, &mut buffers);
    app.set_mode(Mode::Thru);
    app.set_skip_analysis(true);
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    let mut whole = |app: &mut BatApp| {
        best_of(RUNS, || {
            for chunk in x.chunks(1024) {
                let k = chunk.len();
                app.process(chunk, chunk, &mut l[..k], &mut r[..k]);
            }
        })
    };
    let off = whole(&mut app);
    app.change_setting(Setting::AutoLevel, 1);
    let on = whole(&mut app);
    app.change_setting(Setting::AutoLevel, 1);
    let bats_only = whole(&mut app);
    println!();
    println!("{:<42}{:>10.2}", "auto volume, when on", per_s(on - off));
    println!("{:<42}{:>10.2}", "auto volume, when bats only", per_s(bats_only - off));

    // The whole app with one chain against two.
    let (first, second) = load_two(args.get(1), &x);
    println!();
    println!("Whole app, source First or Both, with Auto volume off or on, with and without its analysis:");
    println!("{:<8}{:>9}{:>9}{:>9}{:>9}  |{:>9}{:>9}{:>9}{:>9}", "mode", "First", "Both", "First on", "Both on", "First", "Both", "First on", "Both on");
    for mode in [Mode::Het, Mode::Comb, Mode::Thru] {
        print!("{:<8}", mode.label());
        let row = [
            (MonitorSource::First, AutoVolume::Off, false),
            (MonitorSource::Both, AutoVolume::Off, false),
            (MonitorSource::First, AutoVolume::On, false),
            (MonitorSource::Both, AutoVolume::On, false),
            (MonitorSource::First, AutoVolume::Off, true),
            (MonitorSource::Both, AutoVolume::Off, true),
            (MonitorSource::First, AutoVolume::On, true),
            (MonitorSource::Both, AutoVolume::On, true),
        ];
        for (i, (source, volume, skip)) in row.into_iter().enumerate() {
            let mut storage = vec![0.0f32; 96_000];
            let mut buffers = Box::new(Buffers::new());
            let mut app = BatApp::new(FS, &mut storage, &mut buffers);
            app.set_mode(mode);
            app.set_source(source);
            app.set_auto_volume(volume);
            app.set_skip_analysis(skip);
            let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
            let t = best_of(STEREO_RUNS, || {
                for (a, b) in first.chunks(1024).zip(second.chunks(1024)) {
                    let k = a.len();
                    app.process(a, b, &mut l[..k], &mut r[..k]);
                }
            });
            if i == 4 {
                print!("  |");
            }
            print!("{:>9.2}", per_s(t));
        }
        println!();
    }
}
