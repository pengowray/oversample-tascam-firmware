//! BAT PLAYBACK's player: the list's order, choosing, playing, seeking and the requests to the host.

use std::format;
use std::string::{String, ToString};
use std::vec;

use super::*;
use crate::layout;
use crate::player::{Audio, FileInfo, NotPlayable, Request, Scan, Transport};
use crate::{Button, FileEntry, InputKind, Screen};

fn date(y: u16, m: u16, d: u16) -> u16 {
    ((y - 1980) << 9) | (m << 5) | d
}

fn entry(id: u16, name: &str, d: u16, t: u16) -> FileEntry {
    let mut e = FileEntry::new(id, 0, name.as_bytes(), d, t, 1000);
    e.info = FileInfo::Playable(Audio { sample_rate: 192_000, channels: 2, frames: 192_000 * 10 });
    e
}

/// An app in BAT PLAYBACK with three files listed: ids 0 (oldest) to 2 (newest), named so that
/// BROWSE's order puts the newest first.
fn listed<'a>(storage: &'a mut [f32], files: &'a mut [FileEntry]) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers());
    app.set_input_kind(InputKind::Playback);
    let player = app.player_mut();
    player.set_storage(files);
    player.scan_started();
    player.add_file(entry(1, "B.wav", date(2026, 3, 2), 0));
    player.add_file(entry(0, "C.wav", date(2026, 3, 1), 0));
    player.add_file(entry(2, "A.wav", date(2026, 3, 3), 0));
    player.scan_finished(true);
    app
}

fn names(app: &BatApp) -> Vec<String> {
    app.player().files().iter().map(|e| e.name().to_string()).collect()
}

#[test]
fn the_list_is_in_browse_order_and_keeps_only_what_fits() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 3];
    let mut app = listed(&mut storage, &mut files);
    assert_eq!(names(&app), ["A.wav", "B.wav", "C.wav"]);
    app.player_mut().add_file(entry(3, "D.wav", date(2026, 3, 4), 0));
    assert_eq!(app.player().files().len(), 3);
    assert_eq!(app.player().dropped(), 1);
}

#[test]
fn the_first_list_asks_the_host_to_read_the_card() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_kind(InputKind::Playback);
    // PLAY with no file chosen opens the list.
    app.button(Button::Play);
    assert_eq!(app.screen, Screen::Files);
    assert_eq!(app.player_mut().take_request(), Some(Request::Scan));
    assert_eq!(app.player().scan(), Scan::Running);
}

#[test]
fn a_tap_on_a_row_plays_it_once_the_host_has_opened_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.open_files();
    let y = layout::LIST_TOP + layout::LIST_ROW_H + 10;
    app.touch(160, y);
    assert_eq!(app.screen, Screen::Files, "the row is chosen when the finger lifts");
    app.release();
    assert_eq!(app.screen, Screen::Main);
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id: 1, frame: 0 }));
    assert!(!app.player().playing());
    app.player_mut().opened();
    assert!(app.player().playing());
    app.button(Button::Play);
    assert_eq!(app.player().transport(), Transport::Paused);
}

#[test]
fn a_finger_that_moves_scrolls_the_list_and_chooses_nothing() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 20];
    let mut app = listed(&mut storage, &mut files);
    for i in 3..20 {
        app.player_mut().add_file(entry(i, &format!("{i}.wav"), date(2025, 1, 1), i));
    }
    app.open_files();
    let y = layout::LIST_TOP + 300;
    app.touch(160, y);
    app.drag(160, y - 2 * layout::LIST_ROW_H);
    app.release();
    assert_eq!(app.screen, Screen::Files);
    assert_eq!(app.player().list.top, 2);
    assert_eq!(app.player_mut().take_request(), None);
}

#[test]
fn files_that_cannot_be_played_are_not_chosen() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().set_info(2, FileInfo::NotPlayable(NotPlayable::Unsupported));
    assert!(!app.player_mut().choose(0));
    assert_eq!(app.player_mut().take_request(), None);
}

#[test]
fn the_seek_bar_moves_on_release_and_keeps_playing_or_paused() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(0);
    app.player_mut().take_request();
    app.player_mut().opened();
    let cy = layout::SEEK.center().y;
    app.touch(layout::SEEK_X0, cy);
    app.drag((layout::SEEK_X0 + layout::SEEK_X1) / 2, cy);
    assert!((app.seek_shown().unwrap() - 0.5).abs() < 0.01, "the knob follows the finger");
    assert_eq!(app.player_mut().take_request(), None);
    app.release();
    let Some(Request::Open { id: 2, frame }) = app.player_mut().take_request() else { panic!() };
    assert!((frame as f32 - 192_000.0 * 5.0).abs() < 192_000.0 * 0.05, "frame {frame}");
    app.player_mut().opened();
    assert!(app.player().playing());
}

#[test]
fn play_after_the_end_starts_again_and_after_closing_reopens() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(1);
    app.player_mut().take_request();
    app.player_mut().opened();
    app.player_mut().ended();
    assert_eq!(app.player().transport(), Transport::Ended);
    app.button(Button::Play);
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id: 1, frame: 0 }));
    app.player_mut().opened();
    app.player_mut().set_position(1000);
    app.player_mut().close();
    assert_eq!(app.player_mut().take_request(), Some(Request::Close));
    assert_eq!(app.player().transport(), Transport::Paused);
    app.button(Button::Play);
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id: 1, frame: 1000 }));
}

#[test]
fn home_opens_the_list_and_rec_does_nothing() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.button(Button::Home);
    assert_eq!(app.screen, Screen::Files);
    app.button(Button::Home);
    assert_eq!(app.screen, Screen::Main);
    app.button(Button::Rec);
    assert!(!app.is_recording());
}

#[test]
fn the_wheel_moves_the_highlight_and_play_plays_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.open_files();
    app.wheel(2);
    app.button(Button::Play);
    assert_eq!(app.screen, Screen::Main);
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id: 0, frame: 0 }));
}

#[test]
fn reading_the_card_again_keeps_the_chosen_file_and_its_place() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(1);
    app.player_mut().take_request();
    app.player_mut().opened();
    app.player_mut().set_position(5000);
    app.player_mut().close();
    app.player_mut().take_request();
    app.player_mut().rescan();
    assert_eq!(app.player_mut().take_request(), Some(Request::Scan));
    let player = app.player_mut();
    player.scan_started();
    player.add_file(entry(10, "B.wav", date(2026, 3, 2), 0));
    player.add_file(entry(11, "D.wav", date(2026, 3, 9), 0));
    player.scan_finished(true);
    assert_eq!(player.current().map(|e| e.id), Some(10));
    assert_eq!(player.position(), 5000);
    app.button(Button::Play);
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id: 10, frame: 5000 }));
}

#[test]
fn a_chosen_file_that_is_gone_after_reading_again_is_dropped() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(1);
    app.player_mut().opened();
    app.player_mut().play_pause();
    let player = app.player_mut();
    player.scan_started();
    player.add_file(entry(10, "Z.wav", date(2026, 3, 2), 0));
    player.scan_finished(true);
    assert!(player.current().is_none());
    assert_eq!(player.transport(), Transport::NoFile);
}

fn folder_names(app: &BatApp) -> Vec<String> {
    let player = app.player();
    (0..player.top_folder_count()).map(|i| player.top_folder(i).unwrap().to_string()).collect()
}

#[test]
fn folder_opens_the_card_s_top_folders_with_sound_first() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 3];
    let mut app = listed(&mut storage, &mut files);
    assert_eq!(app.player_mut().take_request(), None);
    app.open_files();
    app.touch(layout::FILES_FOLDER.center().x, layout::FILES_FOLDER.center().y);
    app.release();
    assert_eq!(app.screen, Screen::Folders);
    assert_eq!(app.player().folder_scan(), Scan::Running);
    assert_eq!(app.player_mut().take_request(), Some(Request::ListFolders));
    let player = app.player_mut();
    for name in ["bats", "PortaX8", "SOUND", "A folder name much too long for the folder list"] {
        player.add_top_folder(name.as_bytes());
    }
    player.folders_finished(true);
    assert_eq!(folder_names(&app), ["SOUND", "bats", "PortaX8"]);
    // The highlight starts on the folder listed.
    assert_eq!(app.player().folder_list.highlight, 0);
    assert!(app.player().is_root(0));
    // HOME goes back to the list of recordings.
    app.button(Button::Home);
    assert_eq!(app.screen, Screen::Files);
}

#[test]
fn choosing_another_folder_lists_it_and_stops_the_file() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 3];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(0);
    app.player_mut().opened();
    assert_eq!(app.player().transport(), Transport::Playing);
    app.player_mut().take_request();
    app.open_files();
    app.touch(layout::FILES_FOLDER.center().x, layout::FILES_FOLDER.center().y);
    app.release();
    app.player_mut().take_request();
    for name in ["SOUND", "BATS"] {
        app.player_mut().add_top_folder(name.as_bytes());
    }
    app.player_mut().folders_finished(true);
    // The folder listed already: back to its list, nothing read again.
    app.button(Button::Play);
    assert_eq!(app.screen, Screen::Files);
    assert_eq!(app.player_mut().take_request(), None);
    assert_eq!(app.player().transport(), Transport::Playing);
    // Another folder, by touch.
    app.touch(layout::FILES_FOLDER.center().x, layout::FILES_FOLDER.center().y);
    app.release();
    app.player_mut().take_request();
    for name in ["SOUND", "BATS"] {
        app.player_mut().add_top_folder(name.as_bytes());
    }
    app.player_mut().folders_finished(true);
    let y = layout::LIST_TOP + layout::LIST_ROW_H + 10;
    app.touch(160, y);
    app.release();
    assert_eq!(app.screen, Screen::Files);
    assert_eq!(app.player().root(), "BATS");
    assert_eq!(app.player_mut().take_request(), Some(Request::Scan));
    assert_eq!(app.player().transport(), Transport::NoFile);
    assert!(app.player().files().is_empty());
}

#[test]
fn te_in_bat_playback_plays_the_slowed_file_and_analyses_it_at_real_speed() {
    use crate::{Mode, TeStatus};
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_kind(InputKind::Playback);
    app.set_mode(Mode::Het);
    assert_eq!(app.playback_slowing(), 1);
    app.set_mode(Mode::Te);
    let slowing = app.playback_slowing();
    assert_eq!(slowing, 10);
    // A 40 kHz tone slowed 10 times is 4 kHz.
    let n = 25_600;
    let tone: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * 4_000.0 / FS * core::f32::consts::TAU).sin()).collect();
    let (mut out_l, mut out_r) = (vec![0.0f32; n], vec![0.0f32; n]);
    let before = app.samples;
    app.process(&tone, &tone, &mut out_l, &mut out_r);
    // Every 10th sample is analysed.
    assert_eq!(app.samples - before, (n / slowing as usize) as u64);
    // The slowed file is heard, without waiting for a call.
    let heard = out_l[n / 2..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(heard > 0.01, "peak {heard}");
    assert_eq!(app.te_status(), TeStatus::Waiting);
    // Live input keeps the expander.
    app.set_input_kind(InputKind::Live);
    assert_eq!(app.playback_slowing(), 1);
}

#[test]
fn reopen_keeps_the_place_and_whether_it_plays() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 3];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().choose(0);
    app.player_mut().opened();
    app.player_mut().take_request();
    app.player_mut().set_position(5_000);
    app.player_mut().reopen();
    let id = app.player().current().unwrap().id;
    assert_eq!(app.player_mut().take_request(), Some(Request::Open { id, frame: 5_000 }));
    app.player_mut().opened();
    assert_eq!(app.player().transport(), Transport::Playing);
    app.player_mut().play_pause();
    app.player_mut().reopen();
    app.player_mut().opened();
    assert_eq!(app.player().transport(), Transport::Paused);
}

/// BROWSE's order as one list: in each folder its folders first, by name, then its files, by name,
/// with capitals before small letters and digits before both. The newest file is highlighted.
#[test]
fn the_list_follows_browse_with_folders_first_and_the_newest_highlighted() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_kind(InputKind::Playback);
    let player = app.player_mut();
    player.set_storage(&mut files);
    player.scan_started();
    player.add_folder(1, b"FOLDER_0001");
    player.add_folder(2, b"FOLDER_0001/below");
    player.add_folder(3, b"BATS");
    let in_folder = |id, folder, name: &str, day| {
        let mut e = entry(id, name, date(2026, 3, day), 0);
        e.folder = folder;
        e
    };
    player.add_file(in_folder(0, 0, "b.wav", 1));
    player.add_file(in_folder(1, 0, "B.wav", 2));
    player.add_file(in_folder(2, 0, "_x.wav", 3));
    player.add_file(in_folder(3, 0, "1.wav", 4));
    player.add_file(in_folder(4, 1, "Z.wav", 5));
    player.add_file(in_folder(5, 2, "y.wav", 9));
    player.add_file(in_folder(6, 3, "c.wav", 6));
    player.scan_finished(true);
    assert_eq!(names(&app), ["c.wav", "y.wav", "Z.wav", "1.wav", "B.wav", "_x.wav", "b.wav"]);
    // y.wav, in FOLDER_0001/below, is the newest.
    assert_eq!(app.player().list.highlight, 1);
}
