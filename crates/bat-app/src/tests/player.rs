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

const XRI: &[u8] = b"T=XRI 008a;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=3:EXT MIC,4:EXT MIC;\
INPUT GAIN=3:LOW,4:LOW;LEVEL=3:20,4:20;AUTO_LEVEL=3:OFF,4:OFF;,";
const GUANO: &[u8] = b"GUANO|Version: 1.0\nTimestamp: 2026-03-15T20:50:48+11:00\nMake: TASCAM\n\
Firmware Version: V2.03 with bat patch B258\nLoc Position: -36.123456 146.000007\n\
Note: Gatehouse bat hole, SO.4 mics on inputs 3 and 4, Marshmallow on input 5\n";

/// Touch down on list row `row` (counting from the first shown) and keep the finger there.
fn press_row(app: &mut BatApp, row: i32) {
    app.touch(160, layout::LIST_TOP + row * layout::LIST_ROW_H + 10);
}

fn take_requests(app: &mut BatApp) -> Vec<Request> {
    core::iter::from_fn(|| app.player_mut().take_request()).collect()
}

#[test]
fn a_long_press_on_a_row_shows_its_information_and_does_not_choose_it() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut details = vec![0u8; crate::player::DETAILS_BYTES];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().set_details_storage(&mut details);
    app.open_files();
    let _ = take_requests(&mut app);
    // B.wav, id 1, the second row.
    press_row(&mut app, 1);
    app.hold();
    assert_eq!(app.screen, Screen::FileInfo);
    app.release();
    assert_eq!(app.screen, Screen::FileInfo, "lifting the finger does not choose the row");
    assert_eq!(app.player().transport(), Transport::NoFile);
    assert_eq!(take_requests(&mut app), [Request::Details { id: 1 }]);
    assert_eq!(app.player().list.highlight, 1);
    assert_eq!(app.player().details_state(), Some(crate::player::DetailsState::Reading));
    assert!(screen::details_lines(&app).contains(&"Reading the file".to_string()));

    app.player_mut().details_read(1, Some((Some(crate::player::Encoding::Float32), XRI, GUANO)));
    let lines = screen::details_lines(&app);
    for expected in [
        "B.wav",
        "Folder: SOUND",
        "Date: 2 Mar 2026 00:00",
        "Size: 1 KB",
        "Length: 0:10",
        "Sample rate: 192 kHz",
        "Channels: 2 (stereo)",
        "Bit depth: 32-bit float",
        "XRI",
        "Maker: TASCAM",
        "Firmware: 2.03",
        "Source: 3: EXT MIC",
        ": 4: EXT MIC",
        "Auto level: 3: OFF",
        "GUANO",
        "Timestamp: 2026-03-15T20:50:48+11:00",
        "Loc Position: -36.123456 146.000007",
    ] {
        assert!(lines.iter().any(|l| l == expected), "{expected:?} in {lines:?}");
    }
    // The XRI text's length, its version and GUANO's version are left out; a label too wide for
    // its column has its value on the rows below.
    assert!(!lines.iter().any(|l| l.starts_with("T:") || l.contains("XRI VER") || l.contains("GUANO|Version")));
    let firmware = lines.iter().position(|l| l == "Firmware Version: ").unwrap();
    assert_eq!(lines[firmware + 1], "V2.03 with bat patch B258");
    let note = lines.iter().position(|l| l.starts_with("Note: ")).unwrap();
    assert!(lines.len() > note + 1, "a long note takes more than one row: {lines:?}");
}

#[test]
fn a_finger_that_moved_or_lifted_before_the_long_press_opens_nothing() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    app.open_files();
    let _ = take_requests(&mut app);
    press_row(&mut app, 1);
    app.drag(160, layout::LIST_TOP + layout::LIST_ROW_H + 30);
    app.hold();
    assert_eq!(app.screen, Screen::Files);
    app.release();
    // A hold after the finger has lifted (the host's timer firing late) does nothing either.
    app.hold();
    assert_eq!(app.screen, Screen::Files);
    assert!(take_requests(&mut app).is_empty());
    // Below the rows.
    app.touch(160, layout::LIST_TOP + 3 * layout::LIST_ROW_H + 10);
    app.hold();
    assert_eq!(app.screen, Screen::Files);
}

#[test]
fn the_information_view_s_play_and_back() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut details = vec![0u8; crate::player::DETAILS_BYTES];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().set_details_storage(&mut details);
    app.open_files();
    press_row(&mut app, 2);
    app.hold();
    app.release();
    let _ = take_requests(&mut app);
    // Back, beside Play, returns to the list; HOME does too.
    let back = layout::info_back(true).center();
    app.touch(back.x, back.y);
    assert_eq!(app.screen, Screen::Files);
    press_row(&mut app, 2);
    app.hold();
    app.release();
    app.button(Button::Home);
    assert_eq!(app.screen, Screen::Files);
    // Play plays it from the start (C.wav, id 0), as a tap on its row does. The file read before
    // is shown again without reading it again.
    press_row(&mut app, 2);
    app.hold();
    app.release();
    let play = layout::INFO_PLAY.center();
    app.touch(play.x, play.y);
    assert_eq!(app.screen, Screen::Main);
    assert!(take_requests(&mut app).contains(&Request::Open { id: 0, frame: 0 }));

    // The title bar's information button shows the file playing, with Back only.
    let info = layout::TITLE_INFO_CENTRE;
    app.touch(info.x, info.y);
    assert_eq!(app.screen, Screen::FileInfo);
    assert!(!app.details_play_shown());
    assert!(screen::details_lines(&app)[0] == "C.wav");
    // PLAY there pauses and plays, as on the main screen.
    app.player_mut().opened();
    app.button(Button::Play);
    assert_eq!(app.player().transport(), Transport::Paused);
    let back = layout::info_back(false).center();
    app.touch(back.x, back.y);
    assert_eq!(app.screen, Screen::Main);
}

#[test]
fn the_title_bar_information_button_needs_a_chosen_file() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = listed(&mut storage, &mut files);
    let info = layout::TITLE_INFO_CENTRE;
    app.touch(info.x, info.y);
    // With no file chosen, the title opens the list as before.
    assert_eq!(app.screen, Screen::Files);
}

#[test]
fn details_for_another_file_or_a_failed_read_and_texts_that_do_not_fit() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut details = vec![0u8; 600];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().set_details_storage(&mut details);
    assert!(app.player_mut().show_details(0));
    // An answer for a file no longer shown is ignored.
    app.player_mut().details_read(2 + 1, Some((None, XRI, GUANO)));
    assert_eq!(app.player().details_state(), Some(crate::player::DetailsState::Reading));
    app.player_mut().details_read(2, None);
    assert_eq!(app.player().details_state(), Some(crate::player::DetailsState::Failed));
    assert!(screen::details_lines(&app).contains(&"Couldn't read the file".to_string()));

    // Too long for the storage: GUANO is cut after its last whole line.
    assert!(app.player_mut().show_details(1));
    app.player_mut().details_read(1, Some((None, XRI, GUANO)));
    assert_eq!(app.player().xri(), Some(XRI));
    let guano = app.player().guano().unwrap();
    assert!(guano.len() <= 600 - XRI.len() && guano.ends_with(b"\n"));
    assert!(GUANO.starts_with(guano));

    // None of either.
    assert!(app.player_mut().show_details(0));
    app.player_mut().details_read(2, Some((None, b"", b"")));
    let lines = screen::details_lines(&app);
    assert!(lines.contains(&"No XRI in this file".to_string()) && lines.contains(&"No GUANO in this file".to_string()));

    // Reading the card again gives files new ids: the view forgets its file, and the host closes
    // the view if it is showing.
    app.player_mut().scan_started();
    app.close_file_info();
    assert_ne!(app.screen, Screen::FileInfo);
    assert_eq!(app.player().details_state(), None);
    assert_eq!(screen::details_lines(&app), ["This file is no longer in the list"]);
}

#[test]
fn the_wheel_scrolls_the_information_view_as_far_as_its_last_row() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut details = vec![0u8; crate::player::DETAILS_BYTES];
    let mut app = listed(&mut storage, &mut files);
    app.player_mut().set_details_storage(&mut details);
    app.open_files();
    press_row(&mut app, 0);
    app.hold();
    app.player_mut().details_read(2, Some((None, XRI, GUANO)));
    let rows = screen::details_row_count(&app);
    assert!(rows > layout::INFO_ROWS);
    app.wheel(100);
    assert_eq!(app.player().details_top, rows - layout::INFO_ROWS);
    app.wheel(-3);
    assert_eq!(app.player().details_top, rows - layout::INFO_ROWS - 3);
    app.wheel(-100);
    assert_eq!(app.player().details_top, 0);
}

#[test]
fn sizes_in_kb_mb_and_gb() {
    let mut storage = vec![0.0f32; 1024];
    let mut files = vec![FileEntry::new(0, 0, b"", 0, 0, 0); 8];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_kind(InputKind::Playback);
    let player = app.player_mut();
    player.set_storage(&mut files);
    player.scan_started();
    let sizes = [999, 1_000, 999_949_999, 999_950_000, 1_158_092_908, u32::MAX];
    for (id, bytes) in sizes.into_iter().enumerate() {
        player.add_file(FileEntry::new(id as u16, 0, format!("{id}.wav").as_bytes(), 0, 0, bytes));
    }
    player.scan_finished(true);
    let shown: Vec<String> = (0..sizes.len())
        .map(|row| {
            app.player_mut().show_details(row);
            screen::details_lines(&app).into_iter().find(|l| l.starts_with("Size: ")).unwrap()
        })
        .collect();
    assert_eq!(
        shown,
        ["Size: 999 bytes", "Size: 1 KB", "Size: 999.9 MB", "Size: 1.00 GB", "Size: 1.16 GB", "Size: 4.29 GB"]
    );
}

#[test]
fn xri_fields_split_at_semicolons_and_the_first_equals() {
    let fields: Vec<_> = crate::player::xri_fields(XRI).collect();
    assert_eq!(fields[0], ("T", "XRI 008a"));
    assert_eq!(fields[5], ("SOURCE", "3:EXT MIC,4:EXT MIC"));
    assert_eq!(fields.len(), 9, "the trailing comma is not a field: {fields:?}");
}
