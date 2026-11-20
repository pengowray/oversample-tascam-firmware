//! Settings kept across power-off: what one app packs, a new app loads.

use super::*;

#[test]
fn a_new_app_loads_what_another_saved() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_remote_available(true);
    app.set_mode(Mode::Comb);
    app.set_het_lo(52_300.0);
    app.set_comb_first(31_000.0);
    app.change_setting(Setting::LowestBatCall, 2);
    app.set_auto_volume(AutoVolume::BatsOnly);
    app.set_marked(CallKind::Tone, true);
    app.set_mute_while_recording(true);
    app.set_speaker_switch(false);
    app.set_guano(false);
    app.set_phone_code_required(true);
    let saved = app.saved_settings();

    let mut storage = vec![0.0f32; 1024];
    let mut new = BatApp::new(FS, &mut storage, buffers());
    assert!(new.load_settings(&saved));
    assert_eq!(new.mode(), Mode::Comb);
    assert_eq!(new.het_lo_hz(), app.het_lo_hz());
    assert_eq!(new.lowest_bat_call_hz(), app.lowest_bat_call_hz());
    assert_eq!(new.auto_volume(), AutoVolume::BatsOnly);
    assert!(new.is_marked(CallKind::Tone) && new.is_marked(CallKind::Bat));
    assert!(new.mute_while_recording() && !new.speaker_switch() && !new.guano());
    assert!(new.phone_code_required());
    assert_eq!(new.phone_code(), app.phone_code());
    assert_eq!(new.saved_settings(), saved);
}

#[test]
fn zeros_and_damaged_bytes_change_nothing() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    let defaults = app.saved_settings();
    assert!(!app.load_settings(&[0; SAVED_BYTES]));
    let mut damaged = defaults;
    damaged[1] ^= 0x10;
    assert!(!app.load_settings(&damaged));
    let mut other_version = defaults;
    other_version[0] = 9;
    assert!(!app.load_settings(&other_version));
    assert_eq!(app.saved_settings(), defaults);
}

#[test]
fn a_mode_the_host_does_not_offer_is_not_loaded() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(Mode::Pitch);
    let saved = app.saved_settings();
    let mut storage = vec![0.0f32; 1024];
    let mut new = BatApp::new(FS, &mut storage, buffers());
    new.set_modes(&Mode::WITHOUT_PITCH);
    assert!(new.load_settings(&saved));
    assert_eq!(new.mode(), Mode::Het);
}
