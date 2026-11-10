//! BAT PLAYBACK's file player: the list of recordings on the card, which one plays, and where.
//!
//! The host scans the card, reads each file's header, opens the chosen file and feeds its audio
//! through [`BatApp::process`](crate::BatApp::process). This module keeps the list in the order
//! the screen shows it (BROWSE's order), the transport state the screen draws, and the requests
//! the host carries out ([`Request`]).

/// Bytes kept of a file's name. Longer names are cut short in the list, and cannot be opened.
pub const NAME_BYTES: usize = 48;
/// Folders the list can name. Files in further folders are listed without a folder name.
pub const FOLDERS: usize = 32;
/// Bytes kept of a folder's name.
pub const FOLDER_NAME_BYTES: usize = 24;
/// Folders at the top of the card that the folder choice lists.
pub const TOP_FOLDERS: usize = 24;
/// Bytes kept of the name of a folder at the top of the card. Longer names are left out.
pub const TOP_NAME_BYTES: usize = 40;
/// The folder listed until another is chosen: the one the X8 records into.
pub const DEFAULT_FOLDER: &str = "SOUND";
/// Bytes the file information view keeps of a file's XRI text and GUANO text together, in the
/// storage given with [`Player::set_details_storage`].
pub const DETAILS_BYTES: usize = 4096;
/// Bytes kept of the XRI text, which comes first. The X8 writes about 140.
pub const XRI_BYTES: usize = 512;

pub use bat_dsp::wav::Encoding;

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

    /// The card's date and time, for comparing which file is newer.
    fn when(&self) -> u32 {
        ((self.date as u32) << 16) | self.time as u32
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
    /// List the WAV files in the folder [`Player::root`] names, at the top of the card, and in
    /// its folders: call `scan_started`, then `add_folder` (the root is folder 0) and `add_file`
    /// for what it finds, `set_info` for each header, and `scan_finished`.
    Scan,
    /// List the folders at the top of the card: call `add_top_folder` for each, then
    /// `folders_finished`.
    ListFolders,
    /// Open file `id` (or keep it open) and go to `frame`, then call `opened`. Feed its audio
    /// while [`Player::playing`] is true.
    Open { id: u16, frame: u32 },
    /// Close the file: the bat screen is closing, or the list is being read again.
    Close,
    /// Read file `id`'s XRI text (the `bext` chunk's CodingHistory) and GUANO text, and its
    /// sample format, then call `details_read`.
    Details { id: u16 },
}

/// How far the host has got with reading the file the information view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailsState {
    Reading,
    Read,
    /// The card gave an error, or the file is no longer in the list.
    Failed,
}

/// The file the information view shows, and what was read from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Details {
    id: u16,
    state: DetailsState,
    encoding: Option<Encoding>,
    xri_len: u16,
    guano_len: u16,
}

/// A list's highlighted row (the one the wheel moves and PLAY chooses) and its first row shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListView {
    pub highlight: usize,
    pub top: usize,
}

impl ListView {
    /// Move the highlight by `steps` rows of `count`, keeping it on screen with `rows` rows shown.
    pub fn move_highlight(&mut self, steps: i32, count: usize, rows: usize) {
        if count == 0 {
            return;
        }
        self.highlight = (self.highlight as i32 + steps).clamp(0, count as i32 - 1) as usize;
        if self.highlight < self.top {
            self.top = self.highlight;
        } else if self.highlight >= self.top + rows {
            self.top = self.highlight + 1 - rows;
        }
    }

    /// Scroll so row `top` of `count` is first, keeping `rows` rows filled where possible.
    pub fn scroll_to(&mut self, top: i32, count: usize, rows: usize) {
        let last = count.saturating_sub(rows) as i32;
        self.top = top.clamp(0, last.max(0)) as usize;
    }
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
    /// The list of recordings, as rows of `files`.
    pub(crate) list: ListView,
    /// The folder listed, at the top of the card.
    root: [u8; TOP_NAME_BYTES],
    root_len: u8,
    /// The folders at the top of the card, for choosing the folder listed: [`DEFAULT_FOLDER`]
    /// first, then in alphabetical order.
    top_folders: [[u8; TOP_NAME_BYTES]; TOP_FOLDERS],
    top_lens: [u8; TOP_FOLDERS],
    top_count: usize,
    top_scan: Scan,
    pub(crate) folder_list: ListView,
    request: Option<Request>,
    /// [`Request::ListFolders`], kept apart so it never replaces another request.
    folders_wanted: bool,
    /// The file information view's file, its texts in `details_store`, and whether
    /// [`Request::Details`] is waiting to be taken.
    details: Option<Details>,
    details_store: &'a mut [u8],
    details_wanted: bool,
    /// The information view's first row shown.
    pub(crate) details_top: usize,
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
            list: ListView { highlight: 0, top: 0 },
            root: default_root(),
            root_len: DEFAULT_FOLDER.len() as u8,
            top_folders: [[0; TOP_NAME_BYTES]; TOP_FOLDERS],
            top_lens: [0; TOP_FOLDERS],
            top_count: 0,
            top_scan: Scan::NotStarted,
            folder_list: ListView { highlight: 0, top: 0 },
            request: None,
            folders_wanted: false,
            details: None,
            details_store: &mut [],
            details_wanted: false,
            details_top: 0,
        }
    }

    /// Where the list is kept. Anything listed before is forgotten.
    pub fn set_storage(&mut self, files: &'a mut [FileEntry]) {
        self.files = files;
        self.forget_list();
    }

    /// Where the information view keeps a file's XRI and GUANO texts: [`DETAILS_BYTES`].
    pub fn set_details_storage(&mut self, store: &'a mut [u8]) {
        self.details_store = store;
        self.details = None;
    }

    fn forget_list(&mut self) {
        // File ids are given again by each scan.
        self.details = None;
        self.details_wanted = false;
        self.count = 0;
        self.dropped = 0;
        self.folder_lens = [0; FOLDERS];
        self.list = ListView::default();
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
        if let Some(request) = self.request.take() {
            return Some(request);
        }
        if core::mem::take(&mut self.folders_wanted) {
            return Some(Request::ListFolders);
        }
        let details = self.details.filter(|_| core::mem::take(&mut self.details_wanted))?;
        Some(Request::Details { id: details.id })
    }

    /// The folder listed, at the top of the card.
    pub fn root(&self) -> &str {
        core::str::from_utf8(&self.root[..self.root_len as usize]).unwrap_or("?")
    }

    /// The folders at the top of the card, as found by the last [`Request::ListFolders`].
    pub fn top_folder(&self, row: usize) -> Option<&str> {
        let len = *self.top_lens[..self.top_count].get(row)? as usize;
        Some(core::str::from_utf8(&self.top_folders[row][..len]).unwrap_or("?"))
    }

    pub fn top_folder_count(&self) -> usize {
        self.top_count
    }

    /// How far the host has got with listing the folders at the top of the card.
    pub fn folder_scan(&self) -> Scan {
        self.top_scan
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

    /// A folder at the top of the card. Names longer than [`TOP_NAME_BYTES`] are left out.
    pub fn add_top_folder(&mut self, name: &[u8]) {
        if self.top_count == TOP_FOLDERS || name.is_empty() || name.len() > TOP_NAME_BYTES {
            return;
        }
        let before = |a: &[u8], b: &[u8]| {
            let default = |n: &[u8]| n.eq_ignore_ascii_case(DEFAULT_FOLDER.as_bytes());
            if default(a) != default(b) {
                return default(a);
            }
            a.iter().map(u8::to_ascii_uppercase).lt(b.iter().map(u8::to_ascii_uppercase))
        };
        let at = (0..self.top_count)
            .position(|i| before(name, &self.top_folders[i][..self.top_lens[i] as usize]))
            .unwrap_or(self.top_count);
        self.top_folders.copy_within(at..self.top_count, at + 1);
        self.top_lens.copy_within(at..self.top_count, at + 1);
        self.top_folders[at] = [0; TOP_NAME_BYTES];
        self.top_folders[at][..name.len()].copy_from_slice(name);
        self.top_lens[at] = name.len() as u8;
        self.top_count += 1;
        // The highlight starts on the folder listed now.
        if let Some(row) = (0..self.top_count).find(|&i| self.is_root(i)) {
            self.folder_list = ListView::default();
            self.folder_list.move_highlight(row as i32, self.top_count, crate::layout::LIST_ROWS);
        }
    }

    /// Whether folder choice row `row` is the folder listed now.
    pub fn is_root(&self, row: usize) -> bool {
        row < self.top_count
            && self.top_folders[row][..self.top_lens[row] as usize] == self.root[..self.root_len as usize]
    }

    pub fn folders_finished(&mut self, ok: bool) {
        self.top_scan = if ok { Scan::Done } else { Scan::Failed };
    }

    /// A file's place in BROWSE's order: its folders from the top, then its name. BROWSE lists a
    /// folder's folders before its files, each by name compared character by character, so capitals
    /// come before small letters (docs/HOMEBREW.md, "BROWSE's order"). As one list, that puts the
    /// files of a folder's folders before the folder's own files. A folder past [`FOLDERS`] has no
    /// name kept, and its files sort with the folder listed.
    fn browse_key<'s>(&'s self, e: &'s FileEntry) -> impl Iterator<Item = (bool, &'s [u8])> + 's {
        let folders = if e.folder == 0 { None } else { self.folder_name(e.folder) };
        folders
            .into_iter()
            .flat_map(|path| path.split('/'))
            .map(|folder| (false, folder.as_bytes()))
            .chain(core::iter::once((true, &e.name[..e.name_len as usize])))
    }

    /// Add a file, in BROWSE's order.
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
        let at = self.files[..self.count]
            .iter()
            .position(|e| self.browse_key(&entry).cmp(self.browse_key(e)).is_lt())
            .unwrap_or(self.count);
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
        // With no file chosen yet, the newest file is highlighted, as BROWSE highlights the latest
        // take, so the take just recorded is one press away. A highlight already moved stays.
        if self.current.is_none() && self.keep.is_none() && self.list == ListView::default() {
            let newest = self.files[..self.count].iter().enumerate().max_by_key(|&(i, e)| (e.when(), i));
            if let Some((row, _)) = newest {
                self.list.move_highlight(row as i32, self.count, crate::layout::LIST_ROWS);
            }
        }
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

    /// What the host read for [`Request::Details`]: the file's sample format if it can be
    /// played, and its XRI and GUANO texts (empty if it has none), or None if it could not be
    /// read. Texts longer than the storage are cut: GUANO at the end of its last whole line.
    pub fn details_read(&mut self, id: u16, read: Option<(Option<Encoding>, &[u8], &[u8])>) {
        let Some(details) = self.details.as_mut().filter(|d| d.id == id && d.state == DetailsState::Reading) else {
            return;
        };
        let Some((encoding, xri, guano)) = read else {
            details.state = DetailsState::Failed;
            return;
        };
        let store = &mut *self.details_store;
        let xri = &xri[..xri.len().min(XRI_BYTES).min(store.len())];
        store[..xri.len()].copy_from_slice(xri);
        let room = store.len() - xri.len();
        let guano = if guano.len() <= room {
            guano
        } else {
            let cut = &guano[..room];
            &cut[..cut.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1)]
        };
        store[xri.len()..xri.len() + guano.len()].copy_from_slice(guano);
        *details = Details {
            id,
            state: DetailsState::Read,
            encoding,
            xri_len: xri.len() as u16,
            guano_len: guano.len() as u16,
        };
    }

    /// The file the information view shows, if it is still in the list.
    pub fn details_file(&self) -> Option<&FileEntry> {
        let id = self.details?.id;
        self.files().iter().find(|e| e.id == id)
    }

    pub fn details_state(&self) -> Option<DetailsState> {
        Some(self.details?.state)
    }

    /// The shown file's sample format, once read, if it can be played.
    pub fn details_encoding(&self) -> Option<Encoding> {
        self.details.filter(|d| d.state == DetailsState::Read)?.encoding
    }

    /// The shown file's XRI text, once read: None if it has none.
    pub fn xri(&self) -> Option<&[u8]> {
        let d = self.details.filter(|d| d.state == DetailsState::Read && d.xri_len > 0)?;
        Some(&self.details_store[..d.xri_len as usize])
    }

    /// The shown file's GUANO text, once read: None if it has none.
    pub fn guano(&self) -> Option<&[u8]> {
        let d = self.details.filter(|d| d.state == DetailsState::Read && d.guano_len > 0)?;
        let start = d.xri_len as usize;
        Some(&self.details_store[start..start + d.guano_len as usize])
    }

    // The user's side.

    /// Show the information view for the file at list row `row`, asking the host to read it.
    /// Returns whether there is a file at that row.
    pub fn show_details(&mut self, row: usize) -> bool {
        let Some(id) = self.files().get(row).map(|e| e.id) else { return false };
        self.list.highlight = row;
        self.show_details_of(id);
        true
    }

    /// The same, for the chosen file. Returns whether there is one.
    pub fn show_current_details(&mut self) -> bool {
        let Some(id) = self.current().map(|e| e.id) else { return false };
        self.show_details_of(id);
        true
    }

    fn show_details_of(&mut self, id: u16) {
        self.details_top = 0;
        if self.details.is_some_and(|d| d.id == id && d.state == DetailsState::Read) {
            return;
        }
        self.details = Some(Details { id, state: DetailsState::Reading, encoding: None, xri_len: 0, guano_len: 0 });
        self.details_wanted = true;
    }

    /// Choose the file the information view shows and play it from the start, as [`choose`]
    /// does. Returns whether it was chosen.
    ///
    /// [`choose`]: Self::choose
    pub fn choose_details_file(&mut self) -> bool {
        let Some(id) = self.details.map(|d| d.id) else { return false };
        match self.files().iter().position(|e| e.id == id) {
            Some(row) => self.choose(row),
            None => false,
        }
    }

    /// Ask the host to list the folders at the top of the card again.
    pub fn list_folders(&mut self) {
        self.top_count = 0;
        self.top_scan = Scan::Running;
        self.folder_list = ListView::default();
        self.folders_wanted = true;
    }

    /// List the folder at folder choice row `row` instead. The chosen file stops, unless the
    /// folder is the one listed now. Returns whether there is a folder at that row.
    pub fn choose_folder(&mut self, row: usize) -> bool {
        if row >= self.top_count {
            return false;
        }
        self.folder_list.highlight = row;
        if self.is_root(row) && self.scan != Scan::Failed {
            return true;
        }
        self.root = self.top_folders[row];
        self.root_len = self.top_lens[row];
        self.forget_list();
        self.keep = None;
        self.request = Some(Request::Scan);
        self.scan = Scan::Running;
        true
    }

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
        self.list.highlight = row;
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

    /// Open the chosen file again at the place reached, playing or paused as before: for the host
    /// when the way it plays the file changes (TE's factor, or TE chosen or left).
    pub fn reopen(&mut self) {
        let Some(id) = self.current else { return };
        let play = match self.transport {
            Transport::Playing => true,
            Transport::Opening => self.play_after_open,
            Transport::Paused if !self.closed => false,
            _ => return,
        };
        self.open(id, self.position, play);
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
        self.list.move_highlight(steps, self.count, rows);
    }

    /// Scroll the list so row `top` is first, keeping `rows` rows filled where possible.
    pub fn scroll_to(&mut self, top: i32, rows: usize) {
        self.list.scroll_to(top, self.count, rows);
    }

    pub fn move_folder_highlight(&mut self, steps: i32, rows: usize) {
        self.folder_list.move_highlight(steps, self.top_count, rows);
    }

    pub fn scroll_folders_to(&mut self, top: i32, rows: usize) {
        self.folder_list.scroll_to(top, self.top_count, rows);
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

/// The fields of the XRI text the X8 writes into a take's `bext` chunk, as (key, value): for
/// example `T=XRI 008a;XRI VER=0002;MAKER=TASCAM;...;SOURCE=3:EXT MIC,4:EXT MIC;`. Parts without
/// `=`, or that are not UTF-8, are skipped.
pub fn xri_fields(text: &[u8]) -> impl Iterator<Item = (&str, &str)> {
    text.split(|&b| b == b';').filter_map(|part| {
        let (key, value) = core::str::from_utf8(part).ok()?.split_once('=')?;
        let key = key.trim();
        (!key.is_empty()).then(|| (key, value.trim()))
    })
}

fn default_root() -> [u8; TOP_NAME_BYTES] {
    let mut root = [0; TOP_NAME_BYTES];
    root[..DEFAULT_FOLDER.len()].copy_from_slice(DEFAULT_FOLDER.as_bytes());
    root
}

impl Default for Player<'_> {
    fn default() -> Self {
        Self::new()
    }
}
