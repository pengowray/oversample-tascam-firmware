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

/// An app in BAT PLAYBACK with three files listed: ids 0 (oldest) to 2 (newest).
fn listed<'a>(storage: &'a mut [f32], files: &'a mut [FileEntry]) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers());
    app.set_input_kind(InputKind::Playback);
    let player = app.player_mut();
    player.set_storage(files);
    player.scan_started();
    player.add_file(entry(1, "B.wav", date(2026, 3, 2), 0));
    player.add_file(entry(0, "A.wav", date(2026, 3, 1), 0));
    player.add_file(entry(2, "C.wav", date(2026, 3, 3), 0));
    player.scan_finished(true);
    app
}

fn names(app: &BatApp) -> Vec<String> {
    app.player().files().iter().map(|e| e.name().to_string()).collect()
}

#[test]
fn the_list_is_newest_first_and_keeps_only_what_fits() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 3];
    let mut app = listed(&mut storage, &mut files);
    assert_eq!(names(&app), ["C.wav", "B.wav", "A.wav"]);
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
    assert_eq!(app.player().top, 2);
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
