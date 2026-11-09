//! BAT PLAYBACK's player: the bar in the status bar's place (play or pause, time played, seek
//! bar, length), and the list of recordings.

use core::fmt;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle, Rectangle, Triangle};

use super::{BODY, BODY_BOLD, CENTER, Fb, LEFT, RIGHT, SMALL, fill, fit, rounded, text, width};
use crate::BatApp;
use crate::layout::*;
use crate::palette::*;
use crate::player::{DEFAULT_FOLDER, FileEntry, FileInfo, NotPlayable, Player, Scan, Transport};

pub(crate) const TITLE: &str = "BAT PLAYBACK";
pub(super) const NO_FILE: &str = "Choose a recording: tap here or press PLAY";
pub(super) const BACK: &str = "Back";
pub(super) const READING: &str = "Reading the card";
pub(super) const READ_FAILED: &str = "Couldn't read the SD card";
pub(super) const READ_FAILED_HELP: &str = "Check the card, then tap Back and open this list again.";
/// "No WAV files in SOUND or its subfolders", with the folder listed.
pub(super) const NO_FILES: (&str, &str) = ("No WAV files in ", " or its subfolders");
pub(super) const FOLDER_BUTTON: &str = "Folder: ";
pub(super) const READING_FOLDERS: &str = "Reading the card";
pub(super) const CHOOSE_FOLDER: &str = "Choose a folder";
pub(super) const READ_FOLDERS_FAILED_HELP: &str = "Check the card, tap Back, then tap Folder";
pub(super) const NO_FOLDERS: &str = "No folders on the card";
pub(super) const NO_FOLDERS_HELP: &str = "WAV files need to be in a folder to play here";
pub(super) const DEFAULT_FOLDER_NOTE: &str = "Where the X8 saves its recordings";
/// The list is in the order the X8's BROWSE shows the same files.
pub(super) const LIST_ORDER: &str = "in BROWSE order";
pub(super) const NOT_PLAYABLE_NOT_WAV: &str = "Can't play: named .wav but not a WAV file";
pub(super) const NOT_PLAYABLE_UNSUPPORTED: &str = "Can't play: unsupported WAV (8-bit, compressed or RF64)";
pub(super) const NOT_PLAYABLE_NO_AUDIO: &str = "Can't play: no audio in the file";
pub(super) const NOT_PLAYABLE_UNREADABLE: &str = "Can't play: card read error, or header too long";

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The title: the chosen file's name, or the app's name.
pub(super) fn title<'a>(app: &'a BatApp) -> &'a str {
    app.player().current().map_or(TITLE, FileEntry::name)
}

/// A time in tenths of a second as m:ss, or h:mm:ss from an hour.
struct Clock(u32);

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0 / 10;
        if s >= 3600 {
            write!(f, "{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        } else {
            write!(f, "{}:{:02}", s / 60, s % 60)
        }
    }
}

/// The player bar, in the status bar's place.
pub(super) fn bar(app: &BatApp, fb: Fb) {
    let player = app.player();
    rounded(fb, STATUS, 6, PANEL);
    let cy = STATUS.top_left.y + STATUS.size.height as i32 / 2;
    let icon_x = PLAY_BUTTON.top_left.x + 18;
    if player.transport() == Transport::Playing {
        for x in [icon_x - 6, icon_x + 2] {
            fill(fb, Rectangle::new(Point::new(x, cy - 7), Size::new(4, 14)), TEXT);
        }
    } else {
        let colour = if player.transport() == Transport::Opening { TEXT_MUTED } else { TEXT };
        let _ =
            Triangle::new(Point::new(icon_x - 5, cy - 8), Point::new(icon_x - 5, cy + 8), Point::new(icon_x + 8, cy))
                .into_styled(PrimitiveStyle::with_fill(colour))
                .draw(fb);
    }
    let baseline = cy + 5;
    let Some(audio) = player.current_audio() else {
        // A file chosen but its header not read again yet (after the card is read again) shows
        // an empty bar.
        if player.current().is_none() {
            text(fb, &SMALL, NO_FILE, PLAY_BUTTON.top_left.x + 38, baseline - 1, LEFT, TEXT_MUTED);
        }
        return;
    };
    let fraction = app.seek_shown().unwrap_or(0.0);
    let length = audio.tenths();
    let at = (fraction * length as f32) as u32;
    text(fb, &BODY, format_args!("{}", Clock(at)), POSITION_RIGHT, baseline, RIGHT, TEXT);
    text(
        fb,
        &BODY,
        format_args!("{}", Clock(length)),
        STATUS.top_left.x + STATUS.size.width as i32 - 8,
        baseline,
        RIGHT,
        TEXT_MUTED,
    );
    // The seek bar: the part played in the accent colour, a knob at the place shown.
    let knob = SEEK_X0 + (fraction * (SEEK_X1 - SEEK_X0) as f32) as i32;
    fill(fb, Rectangle::new(Point::new(SEEK_X0, cy - 1), Size::new((SEEK_X1 - SEEK_X0) as u32, 3)), LINE);
    fill(fb, Rectangle::new(Point::new(SEEK_X0, cy - 1), Size::new((knob - SEEK_X0).max(0) as u32, 3)), ACCENT);
    let _ = Circle::with_center(Point::new(knob, cy), 12).into_styled(PrimitiveStyle::with_fill(TEXT)).draw(fb);
}

/// The list of recordings.
pub(super) fn files(app: &BatApp, fb: Fb) {
    let player = app.player();
    let count = player.files().len();
    let heading_x = 10;
    match player.scan() {
        Scan::Failed => {
            text(fb, &BODY_BOLD, READ_FAILED, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
            text(fb, &SMALL, READ_FAILED_HELP, heading_x, LIST_HEADING_BASELINE + 16, LEFT, TEXT_MUTED);
        }
        Scan::Running | Scan::NotStarted => {
            let heading = format_args!("{READING}: {} so far", Recordings(count));
            text(fb, &BODY_BOLD, heading, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
        }
        Scan::Done if count == 0 => {
            // The folder's name is cut short, so the end of the sentence stays.
            let (before, after) = NO_FILES;
            let room = 300 - width(&BODY_BOLD, before) - width(&BODY_BOLD, after);
            let shown = Fitted(fit(&BODY_BOLD, player.root(), room));
            let heading = format_args!("{before}{shown}{after}");
            text(fb, &BODY_BOLD, heading, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
        }
        Scan::Done => {
            let total = count + player.dropped();
            let x = text(
                fb,
                &BODY_BOLD,
                format_args!("{}", Recordings(total)),
                heading_x,
                LIST_HEADING_BASELINE,
                LEFT,
                TEXT,
            );
            // The order says nothing about one row.
            if total > 1 {
                text(fb, &BODY, format_args!(", {LIST_ORDER}"), x, LIST_HEADING_BASELINE, LEFT, TEXT_MUTED);
            }
        }
    }
    let current = player.current().map(|e| e.id);
    for (row, entry) in player.files().iter().enumerate().skip(player.list.top).take(LIST_ROWS) {
        let y = LIST_TOP + (row - player.list.top) as i32 * LIST_ROW_H;
        list_row(player, fb, entry, y, row == player.list.highlight, Some(entry.id) == current);
    }
    rounded(fb, FILES_BACK, 4, BUTTON);
    text(fb, &BODY_BOLD, BACK, FILES_BACK.center().x, FILES_BACK.top_left.y + 28, CENTER, TEXT);
    rounded(fb, FILES_FOLDER, 4, BUTTON);
    let room = FILES_FOLDER.size.width as i32 - 16 - width(&BODY_BOLD, FOLDER_BUTTON);
    let label = format_args!("{FOLDER_BUTTON}{}", Fitted(fit(&BODY_BOLD, player.root(), room)));
    text(fb, &BODY_BOLD, label, FILES_FOLDER.center().x, FILES_FOLDER.top_left.y + 28, CENTER, TEXT);
}

/// What [`fit`] returns, with "..." after a name it cut short.
struct Fitted<'a>((&'a str, bool));

impl fmt::Display for Fitted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (shown, cut) = self.0;
        f.write_str(shown)?;
        if cut { f.write_str("...") } else { Ok(()) }
    }
}

/// The choice of the folder listed: the folders at the top of the card.
pub(super) fn folders(app: &BatApp, fb: Fb) {
    let player = app.player();
    let count = player.top_folder_count();
    let heading_x = 10;
    let help = |fb: Fb, line: &str| text(fb, &SMALL, line, heading_x, LIST_HEADING_BASELINE + 16, LEFT, TEXT_MUTED);
    match player.folder_scan() {
        Scan::Failed => {
            text(fb, &BODY_BOLD, READ_FAILED, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
            help(fb, READ_FOLDERS_FAILED_HELP);
        }
        Scan::Running | Scan::NotStarted => {
            let heading = format_args!("{READING_FOLDERS}: {} so far", Folders(count));
            text(fb, &BODY_BOLD, heading, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
        }
        Scan::Done if count == 0 => {
            text(fb, &BODY_BOLD, NO_FOLDERS, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
            help(fb, NO_FOLDERS_HELP);
        }
        Scan::Done => {
            text(fb, &BODY_BOLD, CHOOSE_FOLDER, heading_x, LIST_HEADING_BASELINE, LEFT, TEXT);
        }
    }
    let view = player.folder_list;
    for row in (view.top..count).take(LIST_ROWS) {
        let y = LIST_TOP + (row - view.top) as i32 * LIST_ROW_H;
        let Some(name) = player.top_folder(row) else { break };
        if row == view.highlight {
            rounded(fb, Rectangle::new(Point::new(4, y + 1), Size::new(312, LIST_ROW_H as u32 - 2)), 6, PANEL);
        }
        let current = player.is_root(row);
        let mut x = 12;
        if current {
            let _ = Triangle::new(Point::new(12, y + 9), Point::new(12, y + 19), Point::new(20, y + 14))
                .into_styled(PrimitiveStyle::with_fill(ACCENT))
                .draw(fb);
            x = 26;
        }
        text(fb, &BODY_BOLD, name, x, y + 19, LEFT, if current { ACCENT } else { TEXT });
        if name.eq_ignore_ascii_case(DEFAULT_FOLDER) {
            text(fb, &SMALL, DEFAULT_FOLDER_NOTE, 12, y + 38, LEFT, TEXT_MUTED);
        }
    }
    rounded(fb, LIST_BACK, 4, BUTTON);
    text(fb, &BODY_BOLD, BACK, LIST_BACK.center().x, LIST_BACK.top_left.y + 28, CENTER, TEXT);
}

/// "1 folder" or "N folders".
struct Folders(usize);

impl fmt::Display for Folders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 1 { f.write_str("1 folder") } else { write!(f, "{} folders", self.0) }
    }
}

/// "1 recording" or "N recordings".
struct Recordings(usize);

impl fmt::Display for Recordings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 1 { f.write_str("1 recording") } else { write!(f, "{} recordings", self.0) }
    }
}

/// One file: its name and length, then its date, folder and format, or why it can't be played.
fn list_row(player: &Player, fb: Fb, e: &FileEntry, y: i32, highlighted: bool, current: bool) {
    if highlighted {
        rounded(fb, Rectangle::new(Point::new(4, y + 1), Size::new(312, LIST_ROW_H as u32 - 2)), 6, PANEL);
    }
    let playable = !matches!(e.info, FileInfo::NotPlayable(_));
    let name_colour = if playable { TEXT } else { TEXT_DIM };
    let mut x = 12;
    if current {
        let _ = Triangle::new(Point::new(12, y + 9), Point::new(12, y + 19), Point::new(20, y + 14))
            .into_styled(PrimitiveStyle::with_fill(ACCENT))
            .draw(fb);
        x = 26;
    }
    // The name, cut short if it would reach the length.
    let mut right = 308;
    if let FileInfo::Playable(audio) = e.info {
        let length = Clock(audio.tenths());
        text(fb, &BODY, format_args!("{length}"), right, y + 19, RIGHT, TEXT);
        let mut buf = Buf::new();
        let _ = fmt::write(&mut buf, format_args!("{length}"));
        right -= width(&BODY, buf.as_str()) + 10;
    }
    let colour = if current { ACCENT } else { name_colour };
    text(fb, &BODY_BOLD, format_args!("{}", Fitted(fit(&BODY_BOLD, e.name(), right - x))), x, y + 19, LEFT, colour);
    let second = y + 38;
    match e.info {
        FileInfo::NotPlayable(reason) => {
            let why = match reason {
                NotPlayable::NotWav => NOT_PLAYABLE_NOT_WAV,
                NotPlayable::Unsupported => NOT_PLAYABLE_UNSUPPORTED,
                NotPlayable::NoAudio => NOT_PLAYABLE_NO_AUDIO,
                NotPlayable::Unreadable => NOT_PLAYABLE_UNREADABLE,
            };
            text(fb, &SMALL, why, 12, second, LEFT, TEXT_DIM);
        }
        _ => {
            text(fb, &SMALL, format_args!("{}", Details { player, e }), 12, second, LEFT, TEXT_MUTED);
        }
    }
}

/// The second line of a playable row: date and time, folder, sample rate and channels.
struct Details<'a> {
    player: &'a Player<'a>,
    e: &'a FileEntry,
}

impl fmt::Display for Details<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = "";
        if let Some((year, month, day, hour, minute)) = self.e.date_time() {
            write!(f, "{day} {} {year} {hour:02}:{minute:02}", MONTHS[month as usize - 1])?;
            sep = "   ";
        }
        if let Some(folder) = self.player.folder_name(self.e.folder) {
            write!(f, "{sep}{folder}")?;
            sep = "   ";
        }
        if let FileInfo::Playable(audio) = self.e.info {
            let khz = (audio.sample_rate + 500) / 1000;
            let channels = match audio.channels {
                1 => "mono",
                2 => "stereo",
                _ => "",
            };
            write!(f, "{sep}{khz} kHz {channels}")?;
        }
        Ok(())
    }
}

/// A short text built with `write!`, for measuring.
struct Buf {
    bytes: [u8; 16],
    len: usize,
}

impl Buf {
    fn new() -> Self {
        Self { bytes: [0; 16], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
