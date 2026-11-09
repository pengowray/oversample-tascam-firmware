//! BAT PLAYBACK's file player: the list of recordings on the card, which one plays, and where.
//!
//! The host scans the card, reads each file's header, opens the chosen file and feeds its audio
//! through [`BatApp::process`](crate::BatApp::process). This module keeps the list in the order
//! the screen shows it (newest first), the transport state the screen draws, and the requests the
//! host carries out ([`Request`]).

/// Bytes kept of a file's name. Longer names are cut short in the list, and cannot be opened.
pub const NAME_BYTES: usize = 48;
/// Folders the list can name. Files in further folders are listed without a folder name.
pub const FOLDERS: usize = 32;
/// Bytes kept of a folder's name.
pub const FOLDER_NAME_BYTES: usize = 24;

/// One WAV file on the card, as the host found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// The host's number for the file, unique within one scan.
    pub id: u16,
    /// Which of the folders added with [`Player::add_folder`] the file is in.
    pub folder: u8,
    /// The name in ASCII, `name_len` bytes long.
    pub name: [u8; NAME_BYTES],
    pub name_len: u8,
    /// The file's date and time from the card, in FAT's packed form: the date is
    /// (year - 1980) << 9 | month << 5 | day, the time hours << 11 | minutes << 5 | seconds / 2.
    pub date: u16,
    pub time: u16,
    pub bytes: u32,
    pub info: FileInfo,
}

impl FileEntry {
    /// An entry with `name` (cut to [`NAME_BYTES`]) and its header not read yet.
    pub fn new(id: u16, folder: u8, name: &[u8], date: u16, time: u16, bytes: u32) -> Self {
        let len = name.len().min(NAME_BYTES);
        let mut stored = [0; NAME_BYTES];
        stored[..len].copy_from_slice(&name[..len]);
        Self { id, folder, name: stored, name_len: len as u8, date, time, bytes, info: FileInfo::Unknown }
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }

    /// Newer files sort first; files from the same moment, by name, last name first.
    fn newer_than(&self, other: &FileEntry) -> bool {
        let key = |e: &FileEntry| ((e.date as u32) << 16) | e.time as u32;
        match key(self).cmp(&key(other)) {
            core::cmp::Ordering::Equal => self.name[..self.name_len as usize] > other.name[..other.name_len as usize],
            order => order.is_gt(),
        }
    }

    /// (year, month 1 to 12, day, hour, minute), or None if the date is not set.
    pub fn date_time(&self) -> Option<(u16, u8, u8, u8, u8)> {
        let (month, day) = ((self.date >> 5 & 15) as u8, (self.date & 31) as u8);
        if self.date == 0 || month == 0 || month > 12 || day == 0 {
            return None;
        }
        Some((1980 + (self.date >> 9), month, day, (self.time >> 11) as u8, (self.time >> 5 & 63) as u8))
    }
}

/// What a file's header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileInfo {
    /// Not read yet.
    Unknown,
    Playable(Audio),
    NotPlayable(NotPlayable),
}

/// A playable file's audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u32,
}

impl Audio {
    /// The length in tenths of a second, rounded down.
    pub fn tenths(&self) -> u32 {
        if self.sample_rate == 0 { 0 } else { (self.frames as u64 * 10 / self.sample_rate as u64) as u32 }
    }
}

/// Why a file cannot be played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotPlayable {
    /// Not a WAV file, though its name ends in .wav.
    NotWav,
    /// A WAV file in a form the player does not read: 8-bit, RF64, compressed.
    Unsupported,
    /// A WAV file with no audio in it.
    NoAudio,
    /// The card gave an error, or the header is longer than the player reads.
    Unreadable,
}

/// How far the host has got with listing the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    NotStarted,
    Running,
    Done,
    /// The card could not be read, for example with no card in the slot.
    Failed,
}

/// What the player is doing with the chosen file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// No file chosen yet.
    NoFile,
    /// Waiting for the host to open the file or move to a new place in it.
    Opening,
    Playing,
    Paused,
    /// Stopped at the end of the file. Play starts it again from the beginning.
    Ended,
}

/// What the app asks the host to do, from [`Player::take_request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// List the card's WAV files again: call `scan_started`, then `add_folder` and `add_file`
    /// for what it finds, `set_info` for each header, and `scan_finished`.
    Scan,
    /// Open file `id` (or keep it open) and go to `frame`, then call `opened`. Feed its audio
    /// while [`Player::playing`] is true.
    Open { id: u16, frame: u32 },
    /// Close the file: the bat screen is closing, or the list is being read again.
    Close,
}

pub struct Player<'a> {
    files: &'a mut [FileEntry],
    count: usize,
    /// Files found that did not fit in `files`.
    dropped: usize,
    folders: [[u8; FOLDER_NAME_BYTES]; FOLDERS],
    folder_lens: [u8; FOLDERS],
    scan: Scan,
    current: Option<u16>,
    transport: Transport,
    /// Whether to play once the host has opened the file, or stay paused.
    play_after_open: bool,
    /// Frames of the current file played so far.
    position: u32,
    /// The host closed the file (the bat screen closed): open it again before playing.
    closed: bool,
    /// While the card is read again: the chosen file's name and folder name, to find it in the
    /// new list and keep its place.
    keep: Option<([u8; NAME_BYTES], u8, [u8; FOLDER_NAME_BYTES], u8)>,
    /// The list row the wheel moves and PLAY plays, as an index into `files`.
    pub(crate) highlight: usize,
    /// The first list row shown.
    pub(crate) top: usize,
    request: Option<Request>,
}

impl<'a> Player<'a> {
    pub fn new() -> Self {
        Self {
            files: &mut [],
            count: 0,
            dropped: 0,
            folders: [[0; FOLDER_NAME_BYTES]; FOLDERS],
            folder_lens: [0; FOLDERS],
            scan: Scan::NotStarted,
            current: None,
            transport: Transport::NoFile,
            play_after_open: false,
            position: 0,
            closed: false,
            keep: None,
            highlight: 0,
            top: 0,
            request: None,
        }
    }

    /// Where the list is kept. Anything listed before is forgotten.
    pub fn set_storage(&mut self, files: &'a mut [FileEntry]) {
        self.files = files;
        self.forget_list();
    }

    fn forget_list(&mut self) {
        self.count = 0;
        self.dropped = 0;
        self.folder_lens = [0; FOLDERS];
        self.highlight = 0;
        self.top = 0;
        self.current = None;
        self.transport = Transport::NoFile;
        self.position = 0;
        self.closed = false;
    }

    pub fn files(&self) -> &[FileEntry] {
        &self.files[..self.count]
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn scan(&self) -> Scan {
        self.scan
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Whether the host should feed the file's audio now.
    pub fn playing(&self) -> bool {
        self.transport == Transport::Playing
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    /// The chosen file, if it is still in the list.
    pub fn current(&self) -> Option<&FileEntry> {
        let id = self.current?;
        self.files().iter().find(|e| e.id == id)
    }

    pub fn current_audio(&self) -> Option<Audio> {
        match self.current()?.info {
            FileInfo::Playable(audio) => Some(audio),
            _ => None,
        }
    }

    pub fn folder_name(&self, folder: u8) -> Option<&str> {
        let i = folder as usize;
        let len = *self.folder_lens.get(i)? as usize;
        (len > 0).then(|| core::str::from_utf8(&self.folders[i][..len]).unwrap_or("?"))
    }

    /// The file `id`'s folder number and name, for the host to open it.
    pub fn file_path(&self, id: u16) -> Option<(u8, &[u8])> {
        let e = self.files().iter().find(|e| e.id == id)?;
        Some((e.folder, &e.name[..e.name_len as usize]))
    }

    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    // The host's side.

    /// The host has started listing the card. The old list is forgotten. The chosen file stays
    /// chosen, at the same place, if the new list has a file with its name in a folder with the
    /// same name.
    pub fn scan_started(&mut self) {
        self.keep = self.current().map(|e| {
            let mut folder = [0; FOLDER_NAME_BYTES];
            let folder_len = self.folder_name(e.folder).map_or(0, |f| {
                folder[..f.len()].copy_from_slice(f.as_bytes());
                f.len()
            });
            (e.name, e.name_len, folder, folder_len as u8)
        });
        let (transport, position, closed) = (self.transport, self.position, self.closed);
        self.forget_list();
        if self.keep.is_some() {
            // Not playing: a scan is not started while a file plays.
            self.transport = if transport == Transport::Ended { Transport::Ended } else { Transport::Paused };
            self.position = position;
            self.closed = closed || transport == Transport::Opening;
        }
        self.scan = Scan::Running;
    }

    pub fn add_folder(&mut self, folder: u8, name: &[u8]) {
        let i = folder as usize;
        if i < FOLDERS {
            let len = name.len().min(FOLDER_NAME_BYTES);
            self.folders[i][..len].copy_from_slice(&name[..len]);
            self.folder_lens[i] = len as u8;
        }
    }

    /// Add a file, in date order, newest first.
    pub fn add_file(&mut self, entry: FileEntry) {
        if self.count == self.files.len() {
            self.dropped += 1;
            return;
        }
        if let Some((name, name_len, folder, folder_len)) = self.keep
            && entry.name[..entry.name_len as usize] == name[..name_len as usize]
            && self.folder_name(entry.folder).map_or(&[][..], str::as_bytes) == &folder[..folder_len as usize]
        {
            self.current = Some(entry.id);
            self.keep = None;
        }
        let at = self.files[..self.count].iter().position(|e| entry.newer_than(e)).unwrap_or(self.count);
        self.files.copy_within(at..self.count, at + 1);
        self.files[at] = entry;
        self.count += 1;
    }

    pub fn set_info(&mut self, id: u16, info: FileInfo) {
        if let Some(e) = self.files[..self.count].iter_mut().find(|e| e.id == id) {
            e.info = info;
        }
    }

    pub fn scan_finished(&mut self, ok: bool) {
        self.scan = if ok { Scan::Done } else { Scan::Failed };
        if self.keep.take().is_some() {
            // The chosen file is gone.
            self.transport = Transport::NoFile;
            self.position = 0;
            self.closed = false;
        }
    }

    /// The host has opened the file asked for, at the frame asked for.
    pub fn opened(&mut self) {
        if self.transport == Transport::Opening {
            self.transport = if self.play_after_open { Transport::Playing } else { Transport::Paused };
        }
    }

    /// The file asked for could not be opened or read. It is marked unreadable.
    pub fn open_failed(&mut self) {
        if let Some(id) = self.current.take() {
            self.set_info(id, FileInfo::NotPlayable(NotPlayable::Unreadable));
        }
        self.transport = Transport::NoFile;
        self.position = 0;
    }

    /// Frames of the file played so far.
    pub fn set_position(&mut self, frame: u32) {
        self.position = frame;
    }

    /// The host has fed the last frame of the file.
    pub fn ended(&mut self) {
        if let Some(audio) = self.current_audio() {
            self.position = audio.frames;
        }
        self.transport = Transport::Ended;
    }

    // The user's side.

    /// Ask the host to list the card, unless a file is playing.
    pub fn rescan(&mut self) {
        if self.transport != Transport::Playing && self.scan != Scan::Running {
            self.request = Some(Request::Scan);
            self.scan = Scan::Running;
        }
    }

    /// Choose the file at list row `row` and play it from the start. Files that cannot be played
    /// are not chosen. Returns whether it was chosen.
    pub fn choose(&mut self, row: usize) -> bool {
        let Some(e) = self.files().get(row) else { return false };
        if matches!(e.info, FileInfo::NotPlayable(_)) {
            return false;
        }
        let id = e.id;
        self.highlight = row;
        self.current = Some(id);
        self.open(id, 0, true);
        true
    }

    fn open(&mut self, id: u16, frame: u32, play: bool) {
        self.closed = false;
        self.position = frame;
        self.play_after_open = play;
        self.transport = Transport::Opening;
        self.request = Some(Request::Open { id, frame });
    }

    /// PLAY: pause, play, or play again from the start after the end.
    pub fn play_pause(&mut self) {
        match self.transport {
            Transport::Playing => self.transport = Transport::Paused,
            Transport::Paused if self.closed => {
                if let Some(id) = self.current {
                    self.open(id, self.position, true);
                }
            }
            Transport::Paused => self.transport = Transport::Playing,
            Transport::Ended => {
                if let Some(id) = self.current {
                    self.open(id, 0, true);
                }
            }
            Transport::Opening => self.play_after_open = !self.play_after_open,
            Transport::NoFile => {}
        }
    }

    /// Go to `fraction` (0 to 1) of the way through the chosen file, playing or paused as before.
    pub fn seek(&mut self, fraction: f32) {
        let (Some(id), Some(audio)) = (self.current, self.current_audio()) else { return };
        let frame = ((fraction.clamp(0.0, 1.0) * audio.frames as f32) as u32).min(audio.frames.saturating_sub(1));
        let play = match self.transport {
            Transport::Playing => true,
            Transport::Opening => self.play_after_open,
            _ => false,
        };
        self.open(id, frame, play);
    }

    /// Move the list's highlight by `steps` rows, keeping it on screen with `rows` rows shown.
    pub fn move_highlight(&mut self, steps: i32, rows: usize) {
        if self.count == 0 {
            return;
        }
        self.highlight = (self.highlight as i32 + steps).clamp(0, self.count as i32 - 1) as usize;
        if self.highlight < self.top {
            self.top = self.highlight;
        } else if self.highlight >= self.top + rows {
            self.top = self.highlight + 1 - rows;
        }
    }

    /// Scroll the list so row `top` is first, keeping `rows` rows filled where possible.
    pub fn scroll_to(&mut self, top: i32, rows: usize) {
        let last = self.count.saturating_sub(rows) as i32;
        self.top = top.clamp(0, last.max(0)) as usize;
    }

    /// The bat screen is closing: the host closes the file. The list stays.
    pub fn close(&mut self) {
        if self.transport == Transport::Playing || self.transport == Transport::Opening {
            self.transport = Transport::Paused;
        }
        if self.current.is_some() {
            self.request = Some(Request::Close);
            self.closed = true;
        }
    }
}

impl Default for Player<'_> {
    fn default() -> Self {
        Self::new()
    }
}
