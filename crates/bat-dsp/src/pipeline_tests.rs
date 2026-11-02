//! Tests that chain modules the way the X8 bat app will: spectrum columns feed
//! the call detector, and the detector triggers the time expander.

use std::vec::Vec;

use crate::detector::{CallDetector, CallEvent};
use crate::spectrum::SpectrumAnalyzer;
use crate::test_util::{chirp, noise, rms, zero_crossing_freq};
use crate::time_expansion::TimeExpander;

const FS: f32 = 192_000.0;
const BLOCK: usize = 192;

struct Run {
    events: Vec<CallEvent>,
    /// Start sample of the block in which the detector first became active.
    trigger_block: Option<usize>,
    output: Vec<f32>,
}

fn run(x: &[f32]) -> Run {
    let mut analyzer: SpectrumAnalyzer<512> = SpectrumAnalyzer::new(FS);
    analyzer.set_hop(128);
    let mut detector: CallDetector<512> = CallDetector::new(FS);
    let mut storage = std::vec![0.0f32; FS as usize];
    let mut expander = TimeExpander::new(FS, &mut storage);
    let mut events = Vec::new();
    let mut trigger_block = None;
    let mut output = std::vec![0.0f32; x.len()];

    for (k, (xi, yi)) in x.chunks(BLOCK).zip(output.chunks_mut(BLOCK)).enumerate() {
        analyzer.push(xi, |col| {
            if let Some(ev) = detector.process(col) {
                events.push(ev);
            }
            if detector.is_active() && trigger_block.is_none() {
                expander.trigger();
                trigger_block = Some(k * BLOCK);
            }
        });
        expander.process(xi, yi);
    }
    Run { events, trigger_block, output }
}

#[test]
fn a_detected_call_triggers_time_expansion_and_plays_back_10_times_slower() {
    let call_at = 96_000; // 0.5 s
    let call_len = 960; // 5 ms
    let mut x = noise(0.003, FS as usize * 4, 7);
    // Bat calls start and end smoothly, so shape the chirp with a Hann envelope.
    let c = chirp(FS, 80_000.0, 40_000.0, 0.1, call_len);
    for (i, (v, c)) in x[call_at..].iter_mut().zip(c).enumerate() {
        let w = (core::f32::consts::PI * i as f32 / call_len as f32).sin();
        *v += c * w * w;
    }
    let r = run(&x);

    assert_eq!(r.events.len(), 1, "events: {:?}", r.events);
    let ev = &r.events[0];
    let ms = |s: u64, at: usize| (s as f32 - at as f32) / FS * 1000.0;
    assert!(ms(ev.start_sample, call_at).abs() < 2.0, "start off by {} ms", ms(ev.start_sample, call_at));
    assert!(ms(ev.end_sample, call_at + call_len).abs() < 2.0, "end off by {} ms", ms(ev.end_sample, call_at + call_len));
    // The range covers the part of the sweep within 20 dB of its peak. With
    // this envelope, the first and last 10% of the sweep are more than 20 dB
    // down, so that is about 44 to 76 kHz.
    assert!((40_000.0..48_000.0).contains(&ev.min_freq_hz), "{ev:?}");
    assert!((72_000.0..80_000.0).contains(&ev.max_freq_hz), "{ev:?}");

    // The detector became active within a few milliseconds of the call starting.
    let trig = r.trigger_block.expect("expander was never triggered");
    let delay_ms = (trig as f32 - call_at as f32) / FS * 1000.0;
    assert!((-1.0..5.0).contains(&delay_ms), "trigger {delay_ms} ms after the call");

    // The loudest stretch of the expander's output is the call, 10 times longer
    // (50 ms) and 10 times lower: sweeping from about 8 kHz down to 4 kHz.
    let len = call_len * 10;
    let step = len / 10;
    let (best, best_rms) = (trig..r.output.len() - len)
        .step_by(step)
        .map(|s| (s, rms(&r.output[s..s + len])))
        .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
    let quiet = rms(&r.output[best + 3 * len..best + 4 * len]);
    assert!(best_rms > 5.0 * quiet, "call rms {best_rms}, after it {quiet}");
    let call = &r.output[best..best + len];
    let first = zero_crossing_freq(&call[..len / 3], FS);
    let last = zero_crossing_freq(&call[2 * len / 3..], FS);
    assert!(first > last, "sweep should fall: {first} Hz then {last} Hz");
    assert!((3_500.0..8_500.0).contains(&first) && (3_500.0..8_500.0).contains(&last), "{first} Hz, {last} Hz");
}

#[test]
fn noise_alone_never_triggers_the_expander() {
    let x = noise(0.003, FS as usize * 3, 11);
    let r = run(&x);
    assert!(r.events.is_empty(), "events: {:?}", r.events);
    assert!(r.trigger_block.is_none());
    assert!(r.output.iter().all(|v| *v == 0.0));
}

mod in_place {
    use super::*;
    use crate::pitch_shift::PitchShifter;

    // These compile only if the constructors are `const`, so the processors
    // can sit in statics without passing through the stack.
    static _SHIFTER: PitchShifter = PitchShifter::unconfigured();
    static _ANALYZER: SpectrumAnalyzer<1024> = SpectrumAnalyzer::unconfigured();
    static _DETECTOR: CallDetector<1024> = CallDetector::unconfigured();

    fn call_in_noise() -> Vec<f32> {
        let mut x = noise(0.01, FS as usize / 2, 5);
        for (v, c) in x[48_000..].iter_mut().zip(chirp(FS, 80_000.0, 40_000.0, 0.1, 960)) {
            *v += c;
        }
        x
    }

    fn columns_and_events(
        an: &mut SpectrumAnalyzer<512>,
        det: &mut CallDetector<512>,
        x: &[f32],
    ) -> (usize, Vec<CallEvent>) {
        let mut columns = 0;
        let mut events = Vec::new();
        for block in x.chunks(BLOCK) {
            an.push(block, |c| {
                columns += 1;
                events.extend(det.process(c));
            });
        }
        (columns, events)
    }

    #[test]
    fn init_gives_the_same_results_as_new() {
        let x = call_in_noise();

        let mut fresh = PitchShifter::new(FS);
        let mut placed = PitchShifter::unconfigured();
        placed.init(FS);
        let a = crate::test_util::run_blocks(&x, BLOCK, |i, o| fresh.process(i, o));
        let b = crate::test_util::run_blocks(&x, BLOCK, |i, o| placed.process(i, o));
        assert_eq!(a, b);

        let (mut an1, mut det1) = (SpectrumAnalyzer::<512>::new(FS), CallDetector::<512>::new(FS));
        let (mut an2, mut det2) = (SpectrumAnalyzer::<512>::unconfigured(), CallDetector::<512>::unconfigured());
        an2.init(FS);
        det2.init(FS);
        let r1 = columns_and_events(&mut an1, &mut det1, &x);
        let r2 = columns_and_events(&mut an2, &mut det2, &x);
        assert_eq!(r1.0, r2.0);
        assert_eq!(r1.1.len(), 1);
        assert_eq!(std::format!("{:?}", r1.1), std::format!("{:?}", r2.1));
    }

    #[test]
    fn init_after_use_starts_again_from_scratch() {
        let x = call_in_noise();
        let mut used = PitchShifter::new(FS);
        used.set_factor(4.0);
        used.set_highpass(false);
        let _ = crate::test_util::run_blocks(&x, BLOCK, |i, o| used.process(i, o));
        used.init(FS);
        let mut fresh = PitchShifter::new(FS);
        let a = crate::test_util::run_blocks(&x, BLOCK, |i, o| used.process(i, o));
        let b = crate::test_util::run_blocks(&x, BLOCK, |i, o| fresh.process(i, o));
        assert_eq!(a, b);

        let (mut an, mut det) = (SpectrumAnalyzer::<512>::new(FS), CallDetector::<512>::new(FS));
        det.set_threshold_db(30.0);
        let _ = columns_and_events(&mut an, &mut det, &x);
        an.init(FS);
        det.init(FS);
        let (mut an_new, mut det_new) = (SpectrumAnalyzer::<512>::new(FS), CallDetector::<512>::new(FS));
        let r1 = columns_and_events(&mut an, &mut det, &x);
        let r2 = columns_and_events(&mut an_new, &mut det_new, &x);
        assert_eq!(std::format!("{:?}", r1.1), std::format!("{:?}", r2.1));
    }

    #[test]
    fn unconfigured_processors_are_quiet_and_do_not_panic() {
        let x = call_in_noise();
        let (mut an, mut det) = (SpectrumAnalyzer::<512>::unconfigured(), CallDetector::<512>::unconfigured());
        let (columns, events) = columns_and_events(&mut an, &mut det, &x);
        assert_eq!((columns, events.len()), (0, 0));
        let mut shifter = PitchShifter::unconfigured();
        let y = crate::test_util::run_blocks(&x, BLOCK, |i, o| shifter.process(i, o));
        assert!(y.iter().all(|v| v.is_finite()));
    }
}
