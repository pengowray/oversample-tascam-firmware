//! Mute while recording: the headphones go silent during a take, and only then.

use super::*;

/// A 45 kHz tone at 0.1 on both inputs, in HET at 40 kHz with Auto volume off, so the
/// headphones hear a steady 5 kHz tone.
fn app_with_tone(storage: &mut [f32]) -> BatApp<'_> {
    let mut app = BatApp::new(FS, storage, buffers());
    app.set_mode(Mode::Het);
    app.set_het_lo(40_000.0);
    app.set_auto_volume(AutoVolume::Off);
    app.set_mute_while_recording(true);
    app
}

/// Peak of the left ear for `seconds` of the tone, played in blocks of 1,024 samples.
fn play(app: &mut BatApp, seconds: f32) -> Vec<f32> {
    let n = (FS * seconds) as usize;
    let input: Vec<f32> = (0..n).map(|i| 0.1 * (core::f32::consts::TAU * 45_000.0 * i as f32 / FS).sin()).collect();
    let mut left = vec![0.0; n];
    let mut right = vec![0.0; n];
    for ((x, l), r) in input.chunks(1024).zip(left.chunks_mut(1024)).zip(right.chunks_mut(1024)) {
        app.process(x, x, l, r);
    }
    left
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn headphones_are_silent_during_a_take_and_return_after_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = app_with_tone(&mut storage);
    app.set_recorder(Recorder::Standby, 0);
    let before = play(&mut app, 0.2);
    assert!(peak(&before[before.len() / 2..]) > 0.01, "heard in record standby");
    assert!(!app.muted_for_take());

    app.set_recorder(Recorder::Recording, 0);
    assert!(app.muted_for_take());
    let take = play(&mut app, 0.2);
    // The fade takes 20 ms; after it the output is exactly zero.
    let fade = (FS * 0.02) as usize + 1024;
    assert!(take[fade..].iter().all(|&v| v == 0.0), "silent during the take");
    // The fade starts from the sound, with no step.
    assert!((take[0] - before[before.len() - 1]).abs() < 0.01);

    app.set_recorder(Recorder::Stopped, 1);
    let after = play(&mut app, 0.2);
    assert!(peak(&after[after.len() / 2..]) > 0.01, "heard again after the take");
}

#[test]
fn a_paused_take_is_heard() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = app_with_tone(&mut storage);
    app.set_recorder(Recorder::Paused, 5);
    let out = play(&mut app, 0.2);
    assert!(peak(&out[out.len() / 2..]) > 0.01);
}

#[test]
fn with_the_switch_off_a_take_is_heard() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = app_with_tone(&mut storage);
    app.set_mute_while_recording(false);
    app.set_recorder(Recorder::Recording, 0);
    let out = play(&mut app, 0.2);
    assert!(peak(&out[out.len() / 2..]) > 0.01);
}

#[test]
fn the_cpu_test_plays_every_mode_during_its_takes() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = app_with_tone(&mut storage);
    app.set_recorder(Recorder::Recording, 0);
    app.start_cpu_test();
    assert!(!app.muted_for_take());
}

/// An audible 2 kHz tone on both inputs, which 1:1 passes on as it is.
fn play_audible(app: &mut BatApp, seconds: f32) -> Vec<f32> {
    let n = (FS * seconds) as usize;
    let input: Vec<f32> = (0..n).map(|i| 0.1 * (core::f32::consts::TAU * 2_000.0 * i as f32 / FS).sin()).collect();
    let mut left = vec![0.0; n];
    let mut right = vec![0.0; n];
    for ((x, l), r) in input.chunks(1024).zip(left.chunks_mut(1024)).zip(right.chunks_mut(1024)) {
        app.process(x, x, l, r);
    }
    left
}

#[test]
fn one_to_one_is_silent_on_a_speaker_with_live_input() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_auto_volume(AutoVolume::Off);
    app.set_mode(Mode::Thru);
    let headphones = play_audible(&mut app, 0.2);
    assert!(peak(&headphones[headphones.len() / 2..]) > 0.01, "1:1 is heard in headphones");

    app.set_speaker(true);
    assert!(app.speaker_muted());
    let speaker = play_audible(&mut app, 0.2);
    let fade = (FS * 0.02) as usize + 1024;
    assert!(speaker[fade..].iter().all(|&v| v == 0.0), "1:1 is silent on the speaker");

    // Another mode plays on the speaker, and 1:1 returns with headphones.
    app.set_mode(Mode::Het);
    assert!(!app.speaker_muted());
    app.set_mode(Mode::Thru);
    app.set_speaker(false);
    let again = play_audible(&mut app, 0.2);
    assert!(peak(&again[again.len() / 2..]) > 0.01);
}

#[test]
fn a_file_plays_in_one_to_one_on_a_speaker() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_auto_volume(AutoVolume::Off);
    app.set_input_kind(InputKind::Playback);
    app.set_mode(Mode::Thru);
    app.set_speaker(true);
    assert!(!app.speaker_muted());
    let out = play_audible(&mut app, 0.2);
    assert!(peak(&out[out.len() / 2..]) > 0.01);
}

#[test]
fn het_and_comb_tuned_below_30_khz_are_silent_on_a_speaker() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_speaker(true);
    app.set_mode(Mode::Het);
    app.set_het_lo(29_000.0);
    assert!(app.speaker_muted());
    app.set_het_lo(30_000.0);
    assert!(!app.speaker_muted());
    app.set_mode(Mode::Comb);
    app.set_comb_first(25_000.0);
    assert!(app.speaker_muted());
    app.set_comb_first(30_000.0);
    assert!(!app.speaker_muted());
    for mode in [Mode::Fd, Mode::Te, Mode::Pitch] {
        app.set_mode(mode);
        assert!(!app.speaker_muted());
    }
}
