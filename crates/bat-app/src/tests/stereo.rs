//! Each ear hearing its own input in HET, COMB, PITCH and 1:1.

use super::*;

const MODES: [Mode; 4] = [Mode::Het, Mode::Comb, Mode::Pitch, Mode::Thru];

/// Run two inputs through the app in blocks of `block` samples, as (left, right).
fn run_both(app: &mut BatApp, first: &[f32], second: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![0.0; first.len()], vec![0.0; first.len()]);
    for (((a, b), l), r) in first.chunks(block).zip(second.chunks(block)).zip(left.chunks_mut(block)).zip(right.chunks_mut(block)) {
        app.process(a, b, l, r);
    }
    (left, right)
}

fn tone(hz: f32, amp: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| (amp as f64 * (core::f64::consts::TAU * hz as f64 * i as f64 / FS as f64).sin()) as f32).collect()
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

/// Frequency of the upward zero crossings.
fn frequency(x: &[f32]) -> f32 {
    let crossings: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
    let (Some(first), Some(last)) = (crossings.first(), crossings.last()) else { return 0.0 };
    (crossings.len() - 1) as f32 * FS / (last - first) as f32
}

/// Sounds that differ between the inputs: a train of calls in noise on the first, and
/// different noise with a steady tone on the second.
fn two_signals() -> (Vec<f32>, Vec<f32>) {
    let first = signal();
    let second: Vec<f32> =
        noise(0.002, first.len()).iter().rev().zip(tone(31_000.0, 0.01, first.len())).map(|(n, t)| n + t).collect();
    (first, second)
}

/// The average of two inputs, as the mono path has always taken it.
fn average(first: &[f32], second: &[f32]) -> Vec<f32> {
    first.iter().zip(second).map(|(a, b)| 0.5 * (a + b)).collect()
}

/// A tone on one input is heard in that ear and not in the other.
///
/// The isolation between the ears has no figure to state: it is infinite. The silent
/// ear's chain sees only zeros, so its output is exactly zero, in every mode and whatever
/// Auto volume does, because the gain and the gate only multiply.
#[test]
fn a_tone_on_one_input_is_heard_in_that_ear_only() {
    let loud = tone(45_000.0, 0.1, FS as usize / 4);
    let silence = vec![0.0; loud.len()];
    for (mode, heard_hz) in [(Mode::Het, 5_000.0), (Mode::Comb, 10_000.0), (Mode::Thru, 45_000.0)] {
        for volume in AutoVolume::ALL {
            for on_first in [true, false] {
                let mut storage = vec![0.0f32; 1024];
                let mut app = BatApp::new(FS, &mut storage, buffers());
                app.set_mode(mode);
                app.set_het_lo(40_000.0);
                app.set_auto_volume(volume);
                let (a, b) = if on_first { (&loud, &silence) } else { (&silence, &loud) };
                let (left, right) = run_both(&mut app, a, b, 1024);
                assert!(app.is_stereo(), "{mode:?}");
                let (heard, quiet) = if on_first { (&left, &right) } else { (&right, &left) };
                assert!(quiet.iter().all(|&v| v == 0.0), "{mode:?} {volume:?}: the other ear should be silent");
                if volume != AutoVolume::BatsOnly {
                    let tail = &heard[heard.len() / 2..];
                    assert!((frequency(tail) - heard_hz).abs() < 0.01 * heard_hz, "{mode:?}: {} Hz", frequency(tail));
                    assert!(rms(tail) > 0.01, "{mode:?}: {}", rms(tail));
                }
            }
        }
    }
}

#[test]
fn identical_inputs_are_the_mono_chain_in_both_ears_bit_for_bit() {
    let x = train(1.5, 0.5, 6);
    for mode in MODES {
        for volume in AutoVolume::ALL {
            let mut outputs = Vec::new();
            for source in [MonitorSource::Both, MonitorSource::First] {
                let mut storage = vec![0.0f32; 1024];
                let mut app = BatApp::new(FS, &mut storage, buffers());
                app.set_mode(mode);
                app.set_auto_volume(volume);
                app.set_source(source);
                let (left, right) = run_both(&mut app, &x, &x, 1000);
                assert_eq!(app.is_stereo(), source == MonitorSource::Both);
                assert_eq!(bits(&left), bits(&right), "{mode:?} {volume:?} {source:?}");
                outputs.push(left);
            }
            assert_eq!(bits(&outputs[0]), bits(&outputs[1]), "{mode:?} {volume:?}");
            assert!(outputs[0].iter().any(|&v| v != 0.0), "{mode:?} {volume:?}");
        }
    }
}

/// With one oscillator per voice, a sound equal on both inputs stays equal in both ears,
/// however the app got there.
#[test]
fn the_oscillator_stays_locked_through_mono_retuning_and_moving_the_comb() {
    let x = tone(47_000.0, 0.1, 40_000);
    for mode in [Mode::Het, Mode::Comb] {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        // An odd number of samples on one chain moves the oscillators to an odd phase.
        app.set_source(MonitorSource::First);
        run_both(&mut app, &x[..1_023], &x[..1_023], 256);
        app.set_source(MonitorSource::Both);
        let (left, right) = run_both(&mut app, &x[1_023..10_000], &x[1_023..10_000], 256);
        assert!(app.is_stereo());
        assert_eq!(bits(&left), bits(&right), "{mode:?} after mono");
        assert!(rms(&left[2_000..]) > 0.01, "{mode:?}");
        // HET's frequency, or the comb's.
        app.wheel(3);
        let (left, right) = run_both(&mut app, &x[10_000..20_000], &x[10_000..20_000], 256);
        assert_eq!(bits(&left), bits(&right), "{mode:?} after retuning");
        assert!(rms(&left[2_000..]) > 0.01, "{mode:?}");
        // By a tap on the waterfall too.
        app.tune_to(40_000.0);
        let (left, right) = run_both(&mut app, &x[20_000..30_000], &x[20_000..30_000], 256);
        assert_eq!(bits(&left), bits(&right), "{mode:?} after a tap");
        // And back to mono and to stereo again.
        app.set_source(MonitorSource::Second);
        run_both(&mut app, &x[30_000..30_101], &x[30_000..30_101], 101);
        app.set_source(MonitorSource::Both);
        let (left, right) = run_both(&mut app, &x[30_101..], &x[30_101..], 256);
        assert_eq!(bits(&left), bits(&right), "{mode:?} after mono again");
    }
}

#[test]
fn auto_volume_keeps_the_level_difference_between_the_ears() {
    // Halving is exact in floats, and each stage is linear or a shared gain, so the
    // second ear stays exactly half of the first (6.02 dB) at every sample, and the
    // gain and the gate cannot move one ear without the other.
    let first = train(3.0, 0.5, 20);
    let second: Vec<f32> = first.iter().map(|v| 0.5 * v).collect();
    for mode in MODES {
        for volume in [AutoVolume::On, AutoVolume::BatsOnly] {
            let mut storage = vec![0.0f32; 1024];
            let mut app = BatApp::new(FS, &mut storage, buffers());
            app.set_mode(mode);
            app.set_auto_volume(volume);
            let (left, right) = run_both(&mut app, &first, &second, 1024);
            for (i, (l, r)) in left.iter().zip(&right).enumerate() {
                assert_eq!((0.5 * l).to_bits(), r.to_bits(), "{mode:?} {volume:?} at sample {i}");
            }
            assert!(rms(&left) > 1e-4, "{mode:?} {volume:?}");
            assert!((db(rms(&left) / rms(&right)) - 6.02).abs() < 0.01);
            if volume == AutoVolume::BatsOnly {
                // Bats only starts by fading shut. Then it is shut before the first call
                // and open after it, in both ears at the same sample.
                let (shut_left, shut_right) = (&left[AFTER_GATE_FIRST_CLOSES..], &right[AFTER_GATE_FIRST_CLOSES..]);
                let opens = shut_left.iter().position(|&v| v != 0.0).expect("the gate should open");
                assert_eq!(opens, shut_right.iter().position(|&v| v != 0.0).unwrap(), "{mode:?}");
                assert!(opens + AFTER_GATE_FIRST_CLOSES > FS as usize / 2, "{mode:?} opened at {opens}");
            }
        }
    }
}

#[test]
fn the_louder_ear_sets_the_gain_for_both() {
    let loud = tone(45_000.0, 0.3, FS as usize);
    let quiet = tone(45_000.0, 0.003, FS as usize);
    let tail = FS as usize * 3 / 4;
    for loud_on_first in [true, false] {
        let mut auto = level::AutoLevel::new(FS);
        auto.set_volume(AutoVolume::On);
        let (mut left, mut right) = if loud_on_first { (loud.clone(), quiet.clone()) } else { (quiet.clone(), loud.clone()) };
        // FD has no noise floor, so only the gain for the target is at work here.
        auto.apply_stereo(&mut left, &mut right, Mode::Fd, true);
        let (louder, quieter) = if loud_on_first { (&left, &right) } else { (&right, &left) };
        // The default target is a peak of 0.25, and the ears stay 40 dB apart.
        assert!((peak(&louder[tail..]) - 0.25).abs() < 0.02, "{}", peak(&louder[tail..]));
        assert!((db(rms(&louder[tail..]) / rms(&quieter[tail..])) - 40.0).abs() < 0.1);
    }
}

#[test]
fn the_noise_floor_follows_the_louder_ear() {
    let mut auto = level::AutoLevel::new(FS);
    auto.set_volume(AutoVolume::On);
    let mut left = tone(45_000.0, 0.01, 1_920);
    let mut right = tone(47_000.0, 0.1, left.len());
    auto.apply_stereo(&mut left, &mut right, Mode::Het, true);
    // The right ear's tone has a power of 0.1^2 / 2 = -23.0 dB, and the first 10 ms
    // window sets the floor.
    let floor = auto.floor_db().expect("the first window sets the floor");
    assert!((floor + 23.0).abs() < 0.1, "{floor}");
}

#[test]
fn with_one_ear_silent_the_other_is_treated_as_one_chain_would_be() {
    // Noise under a train of calls, so that the floor, the envelope and the gate are all
    // at work, for long enough that many windows of the floor go by.
    let x = train(3.0, 0.5, 20);
    let silence = vec![0.0; x.len()];
    for volume in [AutoVolume::On, AutoVolume::BatsOnly] {
        for mode in MODES {
            let mut mono = x.clone();
            let mut auto = level::AutoLevel::new(FS);
            auto.set_volume(volume);
            auto.apply(&mut mono, mode, true);

            let (mut left, mut right) = (x.clone(), silence.clone());
            let mut auto = level::AutoLevel::new(FS);
            auto.set_volume(volume);
            auto.apply_stereo(&mut left, &mut right, mode, true);
            assert_eq!(bits(&left), bits(&mono), "{mode:?} {volume:?}");
            assert!(right.iter().all(|&v| v == 0.0), "{mode:?} {volume:?}");
        }
    }
}

#[test]
fn fd_and_te_hear_the_average_in_both_ears() {
    let (first, second) = two_signals();
    let mean = average(&first, &second);
    for mode in [Mode::Fd, Mode::Te] {
        for volume in AutoVolume::ALL {
            let mut storage = vec![0.0f32; FS as usize];
            let mut app = BatApp::new(FS, &mut storage, buffers());
            app.set_mode(mode);
            app.set_auto_volume(volume);
            let (left, right) = run_both(&mut app, &first, &second, 1024);
            assert!(!app.is_stereo(), "{mode:?}");
            assert_eq!(bits(&left), bits(&right), "{mode:?}");

            let mut storage = vec![0.0f32; FS as usize];
            let mut mono = BatApp::new(FS, &mut storage, buffers());
            mono.set_mode(mode);
            mono.set_auto_volume(volume);
            mono.set_source(MonitorSource::First);
            let (expected, _) = run_both(&mut mono, &mean, &mean, 1024);
            assert_eq!(bits(&left), bits(&expected), "{mode:?} {volume:?}");
        }
    }
}

/// Play two different signals in a mode, with a join partway, and return what the
/// analysis and the time expansion recording made of them.
fn analysis_of(mode: Mode) -> (Vec<u8>, Vec<u8>, Vec<(u64, CallKind)>, Option<CallEvent>, Vec<f32>) {
    let (first, second) = two_signals();
    let mut storage = vec![0.0f32; FS as usize];
    let (waterfall, history, calls, last);
    {
        let mut app = BatApp::new(FS, &mut storage, buffers());
        mark_everything(&mut app);
        app.set_mode(mode);
        run_both(&mut app, &first[..60_000], &second[..60_000], 1024);
        app.input_lost(500);
        run_both(&mut app, &first[60_500..], &second[60_500..], 1024);
        waterfall = app.bufs.wf.to_vec();
        history = app.bufs.history.to_vec();
        calls = app.calls.iter().collect();
        last = app.last_call();
    }
    (waterfall, history, calls, last, storage)
}

#[test]
fn the_analysis_and_the_time_expansion_recording_do_not_change_with_stereo() {
    let stereo = analysis_of(Mode::Het);
    assert!(!stereo.2.is_empty(), "the calls should be found");
    // TE would start playing and stop recording, and PITCH is the same as FD here.
    for mode in [Mode::Fd, Mode::Comb, Mode::Thru] {
        let other = analysis_of(mode);
        assert_eq!(stereo.0, other.0, "waterfall in {mode:?}");
        assert_eq!(stereo.1, other.1, "snapshot history in {mode:?}");
        assert_eq!(stereo.2, other.2, "marked detections in {mode:?}");
        assert_eq!(stereo.3, other.3, "last call in {mode:?}");
        assert_eq!(bits(&stereo.4), bits(&other.4), "time expansion recording in {mode:?}");
    }
}

#[test]
fn with_one_input_on_only_one_chain_runs_whatever_is_stored() {
    let x = train(1.0, 0.3, 3);
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    for mode in MODES {
        for source in [MonitorSource::Both, MonitorSource::First, MonitorSource::Second] {
            let mut storage = vec![0.0f32; 1024];
            let mut app = BatApp::new(FS, &mut storage, buffers());
            app.set_mode(mode);
            app.set_inputs([input3, None]);
            app.set_source(source);
            assert_eq!(app.source(), source);
            let (left, right) = run_both(&mut app, &x, &x, 1024);
            assert!(!app.is_stereo(), "{mode:?} {source:?}");
            assert_eq!(bits(&left), bits(&right));
        }
        // No input on: one chain, on silence.
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        app.set_inputs([None, None]);
        run_both(&mut app, &x[..2048], &x[..2048], 1024);
        assert!(!app.is_stereo(), "{mode:?}");
    }
}

#[test]
fn the_choices_of_one_signal_are_not_stereo() {
    let (first, second) = two_signals();
    let n = 20_000;
    for mode in MODES {
        for (source, heard) in [(MonitorSource::First, &first), (MonitorSource::Second, &second)] {
            let mut storage = vec![0.0f32; 1024];
            let mut app = BatApp::new(FS, &mut storage, buffers());
            app.set_mode(mode);
            app.set_source(source);
            let (left, right) = run_both(&mut app, &first[..n], &second[..n], 1024);
            assert!(!app.is_stereo());
            assert_eq!(bits(&left), bits(&right));
            let mut storage = vec![0.0f32; 1024];
            let mut same = BatApp::new(FS, &mut storage, buffers());
            same.set_mode(mode);
            same.set_source(MonitorSource::First);
            let (expected, _) = run_both(&mut same, &heard[..n], &heard[..n], 1024);
            assert_eq!(bits(&left), bits(&expected), "{mode:?} {source:?}");
        }
    }
}

/// Whether each ear hears its own input: a tone on the first input only.
fn ears_are_separate(app: &mut BatApp) -> bool {
    // 45 kHz is where HET starts, which would come out at 0 Hz.
    let loud = tone(47_000.0, 0.1, 8_192);
    let silence = vec![0.0; loud.len()];
    let (left, right) = run_both(app, &loud, &silence, 1024);
    rms(&left[4_096..]) > 0.01 && right[4_096..].iter().all(|&v| v == 0.0)
}

#[test]
fn the_mix_with_two_inputs_on_and_playback_are_stereo_with_both_sides_chosen() {
    let name = |short, long| Some(InputName { short, long });
    for mode in MODES {
        // MANUAL's mix.
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        app.set_mix_available(true);
        app.set_feed(Feed::Mix);
        app.set_inputs([name("3", "Input 3"), name("4", "Input 4")]);
        assert!(ears_are_separate(&mut app), "the mix, {mode:?}");
        assert!(app.is_stereo());
        app.set_source(MonitorSource::First);
        run_both(&mut app, &[0.0; 16], &[0.0; 16], 16);
        assert!(!app.is_stereo(), "the mix's left side");
        // BAT PLAYBACK's left and right.
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        app.set_input_kind(InputKind::Playback);
        app.set_inputs([name("L", "Left"), name("R", "Right")]);
        assert!(ears_are_separate(&mut app), "playback, {mode:?}");
        app.set_source(MonitorSource::First);
        run_both(&mut app, &[0.0; 16], &[0.0; 16], 16);
        assert!(!app.is_stereo(), "playback's left");
    }
}

/// An app on MANUAL's mix with these inputs turned on.
fn on_the_mix<'a>(storage: &'a mut [f32], mode: Mode, inputs: [Option<InputName>; 2]) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers());
    app.set_mode(mode);
    app.set_mix_available(true);
    app.set_feed(Feed::Mix);
    app.set_inputs(inputs);
    app
}

/// With one input on, or none, the mix's two sides are averaged into one chain, the path
/// B232 used for every mix.
#[test]
fn the_mix_with_one_input_on_plays_the_average_of_its_sides_in_both_ears() {
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    let (first, second) = two_signals();
    let mean = average(&first, &second);
    for mode in MODES {
        for volume in AutoVolume::ALL {
            for inputs in [[input3, None], [None, None]] {
                let mut storage = vec![0.0f32; 1024];
                let mut app = on_the_mix(&mut storage, mode, inputs);
                app.set_auto_volume(volume);
                let (left, right) = run_both(&mut app, &first, &second, 1024);
                assert!(!app.is_stereo(), "{mode:?} {inputs:?}");
                assert_eq!(bits(&left), bits(&right), "{mode:?} {volume:?} {inputs:?}");

                let mut storage = vec![0.0f32; 1024];
                let mut same = BatApp::new(FS, &mut storage, buffers());
                same.set_mode(mode);
                same.set_auto_volume(volume);
                same.set_source(MonitorSource::First);
                let (expected, _) = run_both(&mut same, &mean, &mean, 1024);
                assert_eq!(bits(&left), bits(&expected), "{mode:?} {volume:?} {inputs:?}");
            }
        }
    }
}

/// One mic at the default pans is on one side of the mix only. It is heard in both ears,
/// 6 dB below the same mic with that side of the mix chosen.
#[test]
fn one_mic_on_one_side_of_the_mix_is_heard_in_both_ears() {
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    let mic = tone(47_000.0, 0.1, FS as usize / 4);
    let silence = vec![0.0; mic.len()];
    let tail = mic.len() / 2;
    for mode in MODES {
        for (a, b, side) in [(&mic, &silence, MonitorSource::First), (&silence, &mic, MonitorSource::Second)] {
            let mut storage = vec![0.0f32; 1024];
            let mut app = on_the_mix(&mut storage, mode, [input3, None]);
            let (left, right) = run_both(&mut app, a, b, 1024);
            assert_eq!(bits(&left), bits(&right), "{mode:?} {side:?}");
            assert!(rms(&left[tail..]) > 0.01, "{mode:?} {side:?}: {}", rms(&left[tail..]));

            let mut storage = vec![0.0f32; 1024];
            let mut app = on_the_mix(&mut storage, mode, [input3, None]);
            app.set_source(side);
            let (one_side, _) = run_both(&mut app, a, b, 1024);
            let difference = db(rms(&one_side[tail..]) / rms(&left[tail..]));
            assert!((difference - 6.02).abs() < 0.01, "{mode:?} {side:?}: {difference} dB");
        }
    }
}

#[test]
fn an_ext_in_pair_is_stereo() {
    let name = |short, long| Some(InputName { short, long });
    for mode in MODES {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        app.set_inputs([name("EL", "EXT IN left"), name("ER", "EXT IN right")]);
        assert!(ears_are_separate(&mut app), "{mode:?}");
    }
}

/// The mix's three choices are three different sounds, however many inputs are on.
#[test]
fn with_one_input_on_the_mix_listen_to_reaches_every_choice() {
    use MonitorSource::{Both, First, Second};
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    let mut storage = vec![0.0f32; 1024];
    let mut app = on_the_mix(&mut storage, Mode::Het, [input3, None]);
    let up = [(Feed::Mix, First), (Feed::Mix, Second), (Feed::Inputs, First), (Feed::Inputs, First)];
    for expected in up {
        app.change_setting(Setting::ListenTo, 1);
        assert_eq!((app.feed(), app.source()), expected);
    }
    let down = [(Feed::Mix, Second), (Feed::Mix, First), (Feed::Mix, Both), (Feed::Mix, Both)];
    for expected in down {
        app.change_setting(Setting::ListenTo, -1);
        assert_eq!((app.feed(), app.source()), expected);
    }
}

/// A return to the bat screen with a different number of inputs on: the headphones start
/// again from silence, so the fade between the chains has to start from there too.
#[test]
fn a_change_of_chains_after_a_pause_starts_from_silence() {
    let name = |short, long| Some(InputName { short, long });
    let (one, two) = ([name("3", "Input 3"), None], [name("3", "Input 3"), name("4", "Input 4")]);
    let loud = vec![0.5f32; 1024];
    let silence = vec![0.0f32; 1024];
    for (before, after) in [(two, one), (one, two)] {
        let mut storage = vec![0.0f32; 1024];
        let mut app = on_the_mix(&mut storage, Mode::Thru, before);
        run_both(&mut app, &loud, &loud, 1024);
        app.input_lost(0);
        app.set_inputs(after);
        let (left, right) = run_both(&mut app, &silence, &silence, 1024);
        assert!(left.iter().chain(&right).all(|&v| v == 0.0), "{before:?} to {after:?}: {}", peak(&left).max(peak(&right)));
    }
}

#[test]
fn a_track_turned_on_or_off_on_the_mix_changes_the_chains_with_no_click() {
    let name = |short, long| Some(InputName { short, long });
    let (one, two) = ([name("3", "Input 3"), None], [name("3", "Input 3"), name("4", "Input 4")]);
    // The same signals as for a change of Listen to.
    for (mode, first_hz, second_hz) in [(Mode::Het, 47_000.0, 52_000.0), (Mode::Comb, 58_000.0, 52_000.0), (Mode::Thru, 3_000.0, 5_000.0)] {
        let first: Vec<f32> = tone(first_hz, 0.4, FS as usize).iter().zip(noise(0.001, FS as usize)).map(|(a, b)| a + b).collect();
        let second: Vec<f32> = tone(second_hz, 0.05, FS as usize).iter().zip(noise(0.001, FS as usize).iter().rev()).map(|(a, b)| a + b).collect();
        let limit = MOST_STEP_AT_A_SWITCH * peak(&first);

        let mut storage = vec![0.0f32; 1024];
        let mut app = on_the_mix(&mut storage, mode, two);
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for (i, (a, b)) in first.chunks(1_000).zip(second.chunks(1_000)).enumerate() {
            let inputs = if i % 2 == 0 { two } else { one };
            app.set_inputs(inputs);
            let (l, r) = run_both(&mut app, a, b, 256);
            assert_eq!(app.is_stereo(), i % 2 == 0, "{mode:?}");
            left.extend(l);
            right.extend(r);
        }
        assert!(left.iter().chain(&right).all(|v| v.is_finite()), "{mode:?}");
        assert!(biggest_step(&left) < limit, "{mode:?}: left {} against {limit}", biggest_step(&left));
        assert!(biggest_step(&right) < limit, "{mode:?}: right {} against {limit}", biggest_step(&right));
    }
}

/// `first` and `second` with `lost` samples taken out at `at`, told to the app, in a
/// stereo mode, as (left, right).
fn stereo_with_lost_input(first: &[f32], second: &[f32], mode: Mode, at: usize, lost: usize, tell: bool) -> (Vec<f32>, Vec<f32>) {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(mode);
    let (mut left, mut right) = run_both(&mut app, &first[..at], &second[..at], 1024);
    assert!(app.is_stereo());
    if tell {
        app.input_lost(lost as u32);
    }
    let (l, r) = run_both(&mut app, &first[at + lost..], &second[at + lost..], 1024);
    left.extend(l);
    right.extend(r);
    (left, right)
}

#[test]
fn lost_input_leaves_no_step_in_either_ear() {
    let first = noise_under_a_loud_tone(FS as usize);
    let second: Vec<f32> = tone(1_300.0, 0.05, FS as usize).iter().zip(noise(0.0003, FS as usize).iter().rev()).map(|(a, b)| a + b).collect();
    let (at, lost) = (96_000, 50);

    // 1:1 passes the input through, so the sample either side of the join shows it.
    let (plain_left, plain_right) = stereo_with_lost_input(&first, &second, Mode::Thru, at, lost, false);
    let (left, right) = stereo_with_lost_input(&first, &second, Mode::Thru, at, lost, true);
    for (name, plain, faded) in [("left", &plain_left, &left), ("right", &plain_right, &right)] {
        assert!((plain[at] - plain[at - 1]).abs() > 0.02, "{name}: {} {}", plain[at - 1], plain[at]);
        assert!((faded[at] - faded[at - 1]).abs() < 0.002, "{name}: {} {}", faded[at - 1], faded[at]);
        // The fade changes only the first 64 samples.
        assert_eq!(faded[at + 64..], plain[at + 64..], "{name}");
    }

    for mode in [Mode::Het, Mode::Comb] {
        let (plain_left, plain_right) = stereo_with_lost_input(&first, &second, mode, at, lost, false);
        let (left, right) = stereo_with_lost_input(&first, &second, mode, at, lost, true);
        for (name, plain, faded) in [("left", &plain_left, &left), ("right", &plain_right, &right)] {
            let before = biggest_step(&faded[at - 20_000..at]);
            let click = biggest_step(&plain[at..at + 200]);
            let after = biggest_step(&faded[at..at + 200]);
            assert!(click > 1.5 * before, "{mode:?} {name}: the jump should make a click: {click} {before}");
            assert!(after < 1.5 * before, "{mode:?} {name}: {after} {before}");
        }
    }
}

#[test]
fn a_join_at_a_change_between_mono_and_stereo_has_no_step() {
    let first = tone(3_000.0, 0.3, 20_000);
    let second = tone(7_000.0, 0.1, 20_000);
    // The input is 0.3 x 2 pi x 3 / 192 = 0.03 per sample at most.
    let limit = 0.05;
    for (from, to) in [(MonitorSource::First, MonitorSource::Both), (MonitorSource::Both, MonitorSource::First)] {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(Mode::Thru);
        app.set_source(from);
        let (mut left, mut right) = run_both(&mut app, &first[..9_000], &second[..9_000], 1024);
        app.set_source(to);
        app.input_lost(137);
        let (l, r) = run_both(&mut app, &first[9_137..], &second[9_137..], 1024);
        left.extend(l);
        right.extend(r);
        assert!(biggest_step(&left) < limit, "{from:?} to {to:?}: left {}", biggest_step(&left));
        assert!(biggest_step(&right) < limit, "{from:?} to {to:?}: right {}", biggest_step(&right));
    }
}

/// Both to First in the same chunk as a join, with offsets of opposite sign on the inputs,
/// so that the average of the last samples is far from the left ear's. Returns the left
/// ear, the place of the join in it, and the time expansion recording.
fn join_at_a_change_from_stereo(mode: Mode, lost: usize) -> (Vec<f32>, usize, Vec<f32>) {
    let n = FS as usize / 4;
    let first: Vec<f32> = noise_under_a_loud_tone(n).iter().map(|v| v + 0.3).collect();
    let second: Vec<f32> = noise(0.001, n).iter().rev().map(|v| v - 0.3).collect();
    let at = 30_000;
    let mut storage = vec![0.0f32; FS as usize];
    let mut left;
    {
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        (left, _) = run_both(&mut app, &first[..at], &second[..at], 1024);
        app.set_source(MonitorSource::First);
        if lost > 0 {
            app.input_lost(lost as u32);
        }
        let (l, _) = run_both(&mut app, &first[at + lost..], &second[at + lost..], 1024);
        left.extend(l);
    }
    (left, at, storage)
}

#[test]
fn a_join_at_a_change_from_stereo_starts_the_filter_from_the_left_ear() {
    for mode in [Mode::Het, Mode::Comb] {
        // The same change with no join is the size of step to expect: the left ear carries
        // on with the same filter and the same input.
        let (plain, at, _) = join_at_a_change_from_stereo(mode, 0);
        let (joined, at_join, _) = join_at_a_change_from_stereo(mode, 137);
        assert_eq!(at, at_join);
        let expected = biggest_step(&plain[at - 20_000..at + 400]);
        let step = biggest_step(&joined[at..at + 400]);
        assert!(step < 1.5 * expected, "{mode:?}: {step} against {expected}");
    }
}

#[test]
fn a_join_at_a_change_from_stereo_does_not_change_the_time_expansion_recording() {
    // FD never has two chains, so it passes the recording the same sound as ever.
    let (_, _, plain) = join_at_a_change_from_stereo(Mode::Fd, 137);
    assert!(plain.iter().any(|&v| v != 0.0));
    for mode in [Mode::Het, Mode::Comb] {
        let (_, _, recording) = join_at_a_change_from_stereo(mode, 137);
        assert_eq!(bits(&recording), bits(&plain), "{mode:?}");
    }
}

/// The biggest change in one sample allowed across a change between one chain and two, as
/// a share of the louder input's peak. A change of the sound going into a filter makes the
/// filter's output move quickly for a moment, as it always has when Listen to changes: 0.21
/// of the peak in HET and 0.30 in COMB, with these signals. A step from one ear's sound to
/// the other's, with no fade, would be about 1.0 of the peak.
const MOST_STEP_AT_A_SWITCH: f32 = 0.35;

#[test]
fn switching_between_mono_and_stereo_many_times_is_finite_and_has_no_click() {
    // A loud tone on one input and a quieter one on the other, near HET's frequency and
    // near one of the comb's, so that the two ears hear very different levels when both
    // are on. In 1:1 the tones are low, so that a step would stand out from them.
    for (mode, first_hz, second_hz) in [(Mode::Het, 47_000.0, 52_000.0), (Mode::Comb, 58_000.0, 52_000.0), (Mode::Thru, 3_000.0, 5_000.0)] {
        let first: Vec<f32> = tone(first_hz, 0.4, FS as usize).iter().zip(noise(0.001, FS as usize)).map(|(a, b)| a + b).collect();
        let second: Vec<f32> = tone(second_hz, 0.05, FS as usize).iter().zip(noise(0.001, FS as usize).iter().rev()).map(|(a, b)| a + b).collect();
        let limit = MOST_STEP_AT_A_SWITCH * peak(&first);

        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let order = [MonitorSource::First, MonitorSource::Both, MonitorSource::Second, MonitorSource::Both];
        for (i, (a, b)) in first.chunks(1_000).zip(second.chunks(1_000)).enumerate() {
            app.set_source(order[i % order.len()]);
            let (l, r) = run_both(&mut app, a, b, 256);
            left.extend(l);
            right.extend(r);
        }
        assert!(left.iter().chain(&right).all(|v| v.is_finite()), "{mode:?}");
        assert!(biggest_step(&left) < limit, "{mode:?}: left {} against {limit}", biggest_step(&left));
        assert!(biggest_step(&right) < limit, "{mode:?}: right {} against {limit}", biggest_step(&right));
        std::println!("{mode:?}: biggest step left {}, right {}, limit {limit}", biggest_step(&left), biggest_step(&right));
    }
}

#[test]
fn a_stale_second_filter_does_not_come_back_when_stereo_starts_again() {
    let n = FS as usize / 4;
    let loud = tone(47_000.0, 0.5, n);
    let silence = vec![0.0; n];
    for mode in [Mode::Het, Mode::Comb] {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mode(mode);
        // The second filter fills with a loud sound. Stereo stops on a quiet input, and
        // starts again with the second input silent: the right ear must stay silent.
        run_both(&mut app, &silence, &loud[..n - 7], 1024);
        app.set_source(MonitorSource::First);
        run_both(&mut app, &silence, &loud, 1024);
        app.set_source(MonitorSource::Both);
        let (_, right) = run_both(&mut app, &silence, &silence, 1024);
        assert!(peak(&right) < 1e-3, "{mode:?}: {}", peak(&right));
    }
}

#[test]
fn changing_the_inputs_turned_on_changes_the_chains() {
    let input3 = Some(InputName { short: "3", long: "Input 3" });
    let input4 = Some(InputName { short: "4", long: "Input 4" });
    for feed in [Feed::Inputs, Feed::Mix] {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mix_available(feed == Feed::Mix);
        app.set_feed(feed);
        let x = [0.01f32; 256];
        run_both(&mut app, &x, &x, 256);
        assert!(app.is_stereo(), "{feed:?}");
        app.set_inputs([input3, None]);
        run_both(&mut app, &x, &x, 256);
        assert!(!app.is_stereo(), "{feed:?}");
        app.set_inputs([input3, input4]);
        run_both(&mut app, &x, &x, 256);
        assert!(app.is_stereo(), "{feed:?}");
        app.set_mode(Mode::Fd);
        run_both(&mut app, &x, &x, 256);
        assert!(!app.is_stereo(), "{feed:?}");
        app.set_mode(Mode::Thru);
        run_both(&mut app, &x, &x, 256);
        assert!(app.is_stereo(), "{feed:?}");
    }
}

/// The width of a line of the small help text.
fn small_width(line: &str) -> i32 {
    use u8g2_fonts::{FontRenderer, fonts, types::VerticalPosition};
    FontRenderer::new::<fonts::u8g2_font_helvR08_tf>()
        .get_rendered_dimensions(line, embedded_graphics::prelude::Point::zero(), VerticalPosition::Baseline)
        .map_or(0, |d| d.advance.x)
}

#[test]
fn the_line_under_listen_to_says_what_each_ear_hears() {
    use std::string::ToString;
    let name = |short, long| Some(InputName { short, long });
    let stereo = "Stereo in HET, COMB and 1:1. Mono in FD and TE.";
    let one_signal = "Heard in both ears and shown on the waterfall.";
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let help = |app: &BatApp| screen::listen_help(app).to_string();

    // A pair of inputs, with the real numbers.
    assert_eq!(help(&app), "Left ear: input 3. Right ear: input 4. Mono in FD and TE.");
    app.set_inputs([name("5", "Input 5"), name("6", "Input 6")]);
    assert_eq!(help(&app), "Left ear: input 5. Right ear: input 6. Mono in FD and TE.");
    app.set_inputs([name("1", "Input 1"), name("3", "Input 3")]);
    assert_eq!(help(&app), "Left ear: input 1. Right ear: input 3. Mono in FD and TE.");
    // EXT IN: names that are not numbers, on their own or with an input.
    app.set_inputs([name("EL", "EXT IN left"), name("ER", "EXT IN right")]);
    assert_eq!(help(&app), stereo);
    app.set_inputs([name("3", "Input 3"), name("ER", "EXT IN right")]);
    assert_eq!(help(&app), stereo);
    // One of the two, or one input on with Both still stored.
    app.set_inputs([name("3", "Input 3"), name("4", "Input 4")]);
    app.set_source(MonitorSource::First);
    assert_eq!(help(&app), one_signal);
    app.set_source(MonitorSource::Second);
    assert_eq!(help(&app), one_signal);
    app.set_source(MonitorSource::Both);
    app.set_inputs([name("3", "Input 3"), None]);
    assert_eq!(help(&app), one_signal);
    // No input on.
    app.set_inputs([None, None]);
    assert_eq!(help(&app), "No inputs turned on in MANUAL");

    // MANUAL's mix, and its sides.
    app.set_inputs([name("3", "Input 3"), name("4", "Input 4")]);
    app.set_mix_available(true);
    app.set_feed(Feed::Mix);
    assert_eq!(help(&app), stereo);
    app.set_source(MonitorSource::First);
    assert_eq!(help(&app), one_signal);
    app.set_source(MonitorSource::Second);
    assert_eq!(help(&app), one_signal);
    // One input on the mix: the average of the two sides, in both ears.
    app.set_source(MonitorSource::Both);
    app.set_inputs([name("3", "Input 3"), None]);
    assert_eq!(help(&app), one_signal);
    app.set_source(MonitorSource::First);
    assert_eq!(help(&app), one_signal);
    // No input on the mix: the same line as with No input.
    for source in [MonitorSource::Both, MonitorSource::First, MonitorSource::Second] {
        app.set_source(source);
        app.set_inputs([None, None]);
        assert_eq!(help(&app), "No inputs turned on in MANUAL", "{source:?}");
    }
    app.set_source(MonitorSource::Both);

    // BAT PLAYBACK.
    app.set_mix_available(false);
    app.set_input_kind(InputKind::Playback);
    app.set_inputs([name("L", "Left"), name("R", "Right")]);
    assert_eq!(help(&app), stereo);
    app.set_source(MonitorSource::First);
    assert_eq!(help(&app), one_signal);
    app.set_source(MonitorSource::Second);
    assert_eq!(help(&app), one_signal);
}

#[test]
fn the_help_line_agrees_with_what_the_chains_do() {
    use std::string::ToString;
    let name = |short, long| Some(InputName { short, long });
    let inputs = [[name("3", "Input 3"), name("4", "Input 4")], [name("3", "Input 3"), None], [None, None]];
    for feed in [Feed::Inputs, Feed::Mix] {
        for inputs in inputs {
            for source in [MonitorSource::Both, MonitorSource::First, MonitorSource::Second] {
                let mut storage = vec![0.0f32; 1024];
                let mut app = BatApp::new(FS, &mut storage, buffers());
                app.set_mix_available(feed == Feed::Mix);
                app.set_feed(feed);
                app.set_inputs(inputs);
                app.set_source(source);
                run_both(&mut app, &[0.0; 16], &[0.0; 16], 16);
                let help = screen::listen_help(&app).to_string();
                let says_stereo = help.starts_with("Left ear") || help.starts_with("Stereo");
                assert_eq!(says_stereo, app.is_stereo(), "{feed:?} {inputs:?} {source:?}: {help}");
            }
        }
    }
}

#[test]
fn the_help_lines_fit_inside_their_panels() {
    // Listen to's panel runs from x = 6 to 313, and its text starts at x = 16. The line
    // under Sounds to mark starts at x = 16 too. Digits are all the same width.
    let room = 314 - 16;
    for line in [
        "Left ear: input 5. Right ear: input 6. Mono in FD and TE.",
        "Stereo in HET, COMB and 1:1. Mono in FD and TE.",
        "Heard in both ears and shown on the waterfall.",
        "Marked on waterfall, played in TE and Auto volume Bats only.",
    ] {
        let width = small_width(line);
        std::println!("{width} px: {line}");
        assert!(width <= room, "{width} px, against {room} px: {line}");
    }
    // The notice's panel starts at x = 30, has 5 px of padding each side, and must stop at
    // the waterfall's right edge, x = 316.
    let room = 316 - 30 - 2 * layout::CORNER_NOTICE_PAD_X;
    for line in ["Headphones muted. Press PLAY", "then close the page in MANUAL"] {
        use u8g2_fonts::{FontRenderer, fonts, types::VerticalPosition};
        let width = FontRenderer::new::<fonts::u8g2_font_helvR10_tf>()
            .get_rendered_dimensions(line, embedded_graphics::prelude::Point::zero(), VerticalPosition::Baseline)
            .map_or(0, |d| d.advance.x);
        std::println!("{width} px: {line}");
        assert!(width <= room, "{width} px, against {room} px: {line}");
    }
}
