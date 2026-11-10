//! BAT PLAYBACK's player, in the context running the app (the DSP task): passes the app's
//! requests to the card reader (reader.rs) and what the reader finds back to the app, and plays
//! the chosen file through the app. The file is decoded from the read ring, resampled to
//! 192 kHz so it plays at its real speed, and processed by the app into the output ring, which
//! paces it. While nothing plays, the output ring is kept topped up with silence, so the audio
//! hook keeps playing.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::{self, addr_of_mut};
use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::player::{FileEntry, FileInfo, NotPlayable, Request, Scan, Transport};
use bat_app::{BatApp, InputKind};
use bat_dsp::resample::Resampler;
use bat_dsp::wav::{self, Format};
use x8_patch::card::{header_info, heard_frame, pack_root};

use super::audio::PRIME_FRAMES;
use super::reader::{
    CLOSE, DETAILS_COMMANDS, Event, FILE_COMMANDS, FOLDER_REQUESTS, OPEN, RING_FAILED, RING_TAKEN, RING_WRITTEN,
    SCAN_COMMANDS, events, running,
};
use super::state::{
    BLOCKS, IN_DROPPED, IN_DROPPED_SEEN, IN_READ, IN_WRITE, OUT_READ, OUT_WRITE, PLAYBACK, SOURCE_PEAK,
};
use super::{Slot, spare};

const OUT_RATE: u32 = 192_000;
/// Output frames made at a time, and file frames decoded at a time at most.
const OUT_STEP: usize = 512;
const IN_STEP: usize = 512;
/// Bytes of the read ring copied out at a time at most: 512 frames of 24-bit stereo, or fewer
/// frames of larger ones. Two words more, for a start and an end partway through a word.
const STAGE_BYTES: u32 = 4_096;
const STAGE_WORDS: usize = STAGE_BYTES as usize / 4 + 2;
const _: () = assert!(STAGE_BYTES >= x8_patch::card::MOST_BLOCK_ALIGN as u32);
// The app's output goes back into the buffers the file's frames were decoded into.
const _: () = assert!(IN_STEP == OUT_STEP);
/// Frames kept in the output ring while a file plays (64 ms): more than the 43 ms the audio hook
/// waits for before it starts, so a slow pass does not run it dry, and few enough that PLAY and
/// a seek are heard soon. While nothing plays, a little over what the hook waits for.
const PLAY_FILL: u32 = 12_288;
const SILENCE_FILL: u32 = PRIME_FRAMES + 1_024;
const _: () = assert!(PLAY_FILL <= spare::OUT_FRAMES);
/// While a file plays, the app skips its analysis when the output ring falls below this (21 ms
/// left), until it is back above the second figure, so the sound keeps going.
const ANALYSIS_OFF_BELOW: u32 = 4_096;
const ANALYSIS_ON_ABOVE: u32 = 8_192;

/// The player's state, in DTCM: the resampler's history is read for every output sample.
struct Play {
    resampler: Resampler,
    /// The scan the app's list comes from, the latest file command posted, the latest request to
    /// list the folders at the top of the card, and the latest file to read for the information
    /// view.
    scan: u32,
    command: u32,
    folders: u32,
    details: u32,
    /// The reader has opened the file of `command` and is filling the ring.
    ready: bool,
    format: Option<Format>,
    /// The file frame playing started at, and the end of its audio, in bytes from the start of
    /// the ring (see `Event::Opened`).
    start: u32,
    limit: u32,
    /// Output frames made since playing started at `start`, and the number that covers the rest
    /// of the file.
    out_done: u64,
    out_total: u64,
    /// How many times slower the open file plays (TE in BAT PLAYBACK), as set when it opened.
    slowing: u32,
    /// The ring has given its last frame: silence follows, to play out the resampler's delay.
    at_end: bool,
    /// The app is being fed from the file (rather than nothing, while the output gets silence).
    feeding: bool,
}

#[unsafe(link_section = ".dtcm")]
static PLAY: Slot<Play> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

/// Set up the player's state and give the app its list storage (task 1, once, when the app is
/// built, before the DSP task first runs it).
pub(super) fn init(app: &mut BatApp<'static>) {
    let p = PLAY.0.get().cast::<Play>();
    // SAFETY: nothing else uses PLAY yet. Zero is a valid value for every field but the resampler
    // (which holds a reference) and the format, which are written next.
    unsafe {
        ptr::write_bytes(p, 0, 1);
        addr_of_mut!((*p).resampler).write(Resampler::new());
        addr_of_mut!((*p).format).write(None);
    }
    let list = spare::FILE_LIST as *mut FileEntry;
    for i in 0..spare::LIST_FILES {
        // SAFETY: the list range in `spare`, which the assert below shows is large enough.
        unsafe { list.add(i).write(FileEntry::new(0, 0, b"", 0, 0, 0)) };
    }
    // SAFETY: just written, and used only by the app from now on.
    app.player_mut().set_storage(unsafe { core::slice::from_raw_parts_mut(list, spare::LIST_FILES) });
    // SAFETY: the app's range for the information view's texts in `spare`, used only by the app.
    app.player_mut().set_details_storage(unsafe {
        core::slice::from_raw_parts_mut(spare::APP_DETAILS as *mut u8, spare::DETAILS_BYTES)
    });
}
const _: () = assert!(core::mem::size_of::<FileEntry>() * spare::LIST_FILES <= spare::FILE_LIST_BYTES);

fn state() -> &'static mut Play {
    // SAFETY: written by `init`; used only by the context running the app.
    unsafe { &mut *PLAY.0.get().cast::<Play>() }
}

/// Each opening of the bat screen (from `new_session`). In BAT PLAYBACK: the reader closed its
/// file when the screen last closed, so the app is told; the card is listed again, so takes
/// recorded since show; and with no file chosen the list opens. Not inlined, like the two calls
/// below: the patch region has little room.
#[inline(never)]
pub(super) fn start_session(app: &mut BatApp<'static>) {
    // Before the first actions and touches of the session, which depend on it.
    if PLAYBACK.load(Relaxed) {
        app.set_input_kind(InputKind::Playback);
        // What the reader found before the screen closed, including the answer to an Open.
        take_events(app);
    } else {
        app.set_input_kind(InputKind::Live);
    }
    let p = state();
    p.ready = false;
    p.feeding = false;
    if !PLAYBACK.load(Relaxed) {
        return;
    }
    let player = app.player_mut();
    // The reader stops listing when the screen closes.
    if player.scan() == Scan::Running {
        player.scan_finished(false);
    }
    if player.folder_scan() == Scan::Running {
        player.list_folders();
    }
    // The player keeps one request at a time, so each is passed on before the next.
    player.close();
    pass_requests(app);
    app.player_mut().rescan();
    pass_requests(app);
    if app.player().transport() == Transport::NoFile {
        app.open_files();
        pass_requests(app);
    }
}

/// Pass the app's requests to the reader.
#[inline(never)]
fn pass_requests(app: &mut BatApp<'static>) {
    let p = state();
    while let Some(request) = app.player_mut().take_request() {
        match request {
            Request::Scan => {
                p.scan = SCAN_COMMANDS.post(pack_root(app.player().root().as_bytes()));
                app.player_mut().scan_started();
                if !running() {
                    app.player_mut().scan_finished(false);
                }
            }
            Request::Open { id, frame } => {
                p.ready = false;
                p.feeding = false;
                p.command = FILE_COMMANDS.post([OPEN, id.into(), frame]);
                if !running() {
                    app.player_mut().open_failed();
                }
            }
            Request::ListFolders => {
                p.folders = FOLDER_REQUESTS.load(Relaxed).wrapping_add(1);
                FOLDER_REQUESTS.store(p.folders, Release);
                if !running() {
                    app.player_mut().folders_finished(false);
                }
            }
            Request::Close => {
                p.ready = false;
                p.feeding = false;
                p.command = FILE_COMMANDS.post([CLOSE, 0, 0]);
            }
            Request::Details { id } => {
                p.details = DETAILS_COMMANDS.post([id.into()]);
                if !running() {
                    app.player_mut().details_read(id, None);
                }
            }
        }
    }
}

/// Give the app what the reader has found, leaving out answers to scans and commands since
/// replaced.
#[inline(never)]
fn take_events(app: &mut BatApp<'static>) {
    let p = state();
    let ring = events();
    while let Some(event) = ring.pop() {
        let slowing = app.playback_slowing();
        let player = app.player_mut();
        match event {
            Event::Folder { scan, index, name, len } if scan == p.scan => {
                player.add_folder(index, &name[..len as usize])
            }
            Event::File { scan, entry } if scan == p.scan => player.add_file(entry),
            Event::Info { scan, id, info } if scan == p.scan => player.set_info(id, info),
            Event::ScanDone { scan, ok } if scan == p.scan => player.scan_finished(ok),
            Event::TopFolder { request, name, len } if request == p.folders => {
                player.add_top_folder(&name[..len as usize])
            }
            Event::FoldersDone { request, ok } if request == p.folders => player.folders_finished(ok),
            Event::Opened { command, id, format, frame, skip, limit } if command == p.command => {
                // Slowing the file is converting it to a rate that many times higher.
                p.slowing = slowing;
                p.resampler.set_rates(format.sample_rate, OUT_RATE * p.slowing);
                p.format = Some(format);
                p.start = frame;
                p.limit = limit;
                p.at_end = false;
                p.out_done = 0;
                p.out_total = p.resampler.out_frames(u64::from(format.frames().saturating_sub(frame)));
                p.ready = true;
                p.feeding = false;
                RING_TAKEN.store(skip, Release);
                // The header pass may not have reached this file yet. A list read again since the
                // Open may have given the id to another file.
                if player.current().is_some_and(|e| e.id == id) {
                    player.set_info(id, header_info(Ok(format)));
                }
                player.opened();
            }
            Event::Details { command, id, ok, encoding, xri, guano } if command == p.details => {
                // SAFETY: what the reader wrote before posting this event; it writes there again
                // only for a later request, which this task has not posted yet.
                let found =
                    unsafe { core::slice::from_raw_parts(spare::DETAILS_FOUND as *const u8, spare::DETAILS_BYTES) };
                let (xri, guano) = (usize::from(xri), usize::from(guano));
                let texts = (xri + guano <= found.len()).then(|| (&found[..xri], &found[xri..xri + guano]));
                player.details_read(id, ok.then_some(()).and(texts).map(|(x, g)| (encoding, x, g)));
            }
            Event::OpenFailed { command, id, why } if command == p.command => {
                let current = player.current().is_some_and(|e| e.id == id);
                player.open_failed();
                if current && why != NotPlayable::Unreadable {
                    player.set_info(id, FileInfo::NotPlayable(why));
                }
            }
            _ => {}
        }
    }
}

/// BAT PLAYBACK's pass (the context running the app): for up to `time_blocks` of wall time,
/// play the file into the output ring, or keep it topped up with silence. Not inlined, so the
/// DSP task and task 1 share one copy.
#[inline(never)]
pub(super) fn play(app: &mut BatApp<'static>, time_blocks: u32) {
    let start = BLOCKS.load(Relaxed);
    // The inputs are not heard here; drop them, so the input ring does not fill and count them
    // as lost.
    IN_READ.store(IN_WRITE.load(Acquire), Release);
    IN_DROPPED_SEEN.store(IN_DROPPED.load(Acquire), Relaxed);
    take_events(app);
    pass_requests(app);
    let p = state();
    if p.ready && app.playback_slowing() != p.slowing {
        // TE chosen or left, or its factor changed: open the file again at the place reached.
        p.ready = false;
        app.player_mut().reopen();
        pass_requests(app);
    }
    if p.ready && app.player().playing() {
        if !p.feeding {
            // A new place in the file, or after a pause: the app's history does not lead into it.
            app.input_lost(0);
            p.feeding = true;
        }
        make_sound(app, p, start, time_blocks);
        if let Some(format) = p.format {
            let queued = OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Acquire));
            let heard = heard_frame(p.start, p.out_done, queued, format.sample_rate, OUT_RATE * p.slowing);
            app.player_mut().set_position(heard.min(format.frames()));
        }
        if p.out_done >= p.out_total {
            app.player_mut().ended();
            p.feeding = false;
        } else if !p.at_end && RING_FAILED.load(Acquire) && ring_frames(p) == 0 {
            // The card failed partway: stop where it got to, closed, so PLAY opens it again there.
            app.player_mut().close();
            pass_requests(app);
        }
    } else {
        p.feeding = false;
        top_up_silence();
    }
}

/// Whole frames in the read ring, not yet decoded.
fn ring_frames(p: &Play) -> u32 {
    let Some(format) = p.format else { return 0 };
    let taken = RING_TAKEN.load(Relaxed);
    let available = RING_WRITTEN.load(Acquire).saturating_sub(taken).min(p.limit.saturating_sub(taken));
    available / u32::from(format.block_align)
}

/// Resample the file into the output ring, through the app, until the ring holds PLAY_FILL
/// frames, the time is up, the read ring runs out or the file has been played to its end.
///
/// Frames are decoded from the read ring each time the resampler needs them, and only the ones
/// it takes are counted as taken, so nothing decoded is kept between calls. The few it leaves
/// are decoded again next time.
fn make_sound(app: &mut BatApp<'static>, p: &mut Play, start: u32, time_blocks: u32) {
    let Some(format) = p.format else { return };
    let out = spare::OUT_RING as *mut f32;
    let mut stage = [0u32; STAGE_WORDS];
    // The file's frames, then the app's output.
    let mut in_l = [0.0f32; IN_STEP];
    let mut in_r = [0.0f32; IN_STEP];
    let mut file_l = [0.0f32; OUT_STEP];
    let mut file_r = [0.0f32; OUT_STEP];
    let align = u32::from(format.block_align);
    while BLOCKS.load(Relaxed).wrapping_sub(start) < time_blocks {
        let write = OUT_WRITE.load(Relaxed);
        let used = write.wrapping_sub(OUT_READ.load(Acquire));
        let left = p.out_total.saturating_sub(p.out_done);
        if used >= PLAY_FILL || left == 0 {
            break;
        }
        if used < ANALYSIS_OFF_BELOW {
            app.set_skip_analysis(true);
        } else if used >= ANALYSIS_ON_ABOVE {
            app.set_skip_analysis(false);
        }
        let want = ((PLAY_FILL - used) as usize).min(OUT_STEP).min(left as usize);
        let mut made = 0;
        while made < want {
            // About what the resampler needs for the rest of this step, and one more.
            let out_rate = u64::from(OUT_RATE * p.slowing);
            let needed = ((want - made) as u64 * u64::from(format.sample_rate)).div_ceil(out_rate) + 1;
            let needed = (needed as usize).min(IN_STEP);
            let taken_before = RING_TAKEN.load(Relaxed);
            if !p.at_end && p.limit.saturating_sub(taken_before) < align {
                p.at_end = true;
            }
            let frames = if p.at_end {
                in_l[..needed].fill(0.0);
                in_r[..needed].fill(0.0);
                needed
            } else {
                decode(p, &format, &mut stage, &mut in_l, &mut in_r, needed)
            };
            if frames == 0 {
                // Waiting for the card.
                break;
            }
            let (taken, written) =
                p.resampler.process(&in_l[..frames], &in_r[..frames], &mut file_l[made..want], &mut file_r[made..want]);
            if !p.at_end {
                RING_TAKEN.store(taken_before.wrapping_add(taken as u32 * align), Release);
            }
            made += written;
            if taken == 0 && written == 0 {
                break;
            }
        }
        if made == 0 {
            break;
        }
        app.process(&file_l[..made], &file_r[..made], &mut in_l[..made], &mut in_r[..made]);
        let mut at = (write % spare::OUT_FRAMES) as usize;
        for (&l, &r) in in_l[..made].iter().zip(&in_r[..made]) {
            // SAFETY: `at` is below OUT_FRAMES, so both samples are inside the output ring.
            unsafe {
                ptr::write_volatile(out.add(2 * at), l);
                ptr::write_volatile(out.add(2 * at + 1), r);
            }
            at += 1;
            if at == spare::OUT_FRAMES as usize {
                at = 0;
            }
        }
        // Both samples of every frame are written before the interrupt can read them.
        OUT_WRITE.store(write.wrapping_add(made as u32), Release);
        p.out_done += made as u64;
    }
}

/// Decode up to `most` frames from the read ring, from RING_TAKEN, without taking them. Returns
/// the frames decoded: 0 if the ring has none yet.
fn decode(
    p: &Play,
    format: &Format,
    stage: &mut [u32; STAGE_WORDS],
    left: &mut [f32; IN_STEP],
    right: &mut [f32; IN_STEP],
    most: usize,
) -> usize {
    let align = u32::from(format.block_align);
    let frames = ring_frames(p).min(most as u32).min((STAGE_BYTES - 8) / align);
    if frames == 0 {
        return 0;
    }
    let bytes = frames * align;
    let offset = copy_from_ring(stage, RING_TAKEN.load(Relaxed), bytes);
    // SAFETY: the staging words as bytes; `copy_from_ring` filled `offset..offset + bytes`.
    let raw = unsafe { core::slice::from_raw_parts(stage.as_ptr().cast::<u8>(), STAGE_WORDS * 4) };
    let decoded = wav::decode(format, &raw[offset..offset + bytes as usize], left, right);
    let peak = left[..decoded].iter().chain(&right[..decoded]).fold(0.0f32, |m, &x| m.max(x.abs()));
    let peak = (peak.min(1.0) * 2_147_483_647.0) as u32;
    SOURCE_PEAK.store(SOURCE_PEAK.load(Relaxed).max(peak), Relaxed);
    decoded
}

/// Copy the read ring's bytes `from..from + bytes` (counted as RING_TAKEN counts them) into
/// `stage`, in whole words, so the uncached ring is read in bursts and never at an address
/// that is not a multiple of 4. Returns where in `stage` the first byte is.
fn copy_from_ring(stage: &mut [u32; STAGE_WORDS], from: u32, bytes: u32) -> usize {
    let ring_words = spare::READ_RING_BYTES / 4;
    let first = from / 4;
    let words = (from + bytes).div_ceil(4) - first;
    let at = first % ring_words;
    let before_end = words.min(ring_words - at);
    let ring = spare::READ_RING as *const u32;
    // SAFETY: both runs are inside the ring and inside `stage` (at most STAGE_BYTES / 4 + 1
    // words), and do not overlap it.
    unsafe {
        ptr::copy_nonoverlapping(ring.add(at as usize), stage.as_mut_ptr(), before_end as usize);
        ptr::copy_nonoverlapping(ring, stage.as_mut_ptr().add(before_end as usize), (words - before_end) as usize);
    }
    (from % 4) as usize
}

/// Keep the output ring at SILENCE_FILL frames of silence while nothing plays.
fn top_up_silence() {
    let out = spare::OUT_RING as *mut f32;
    let write = OUT_WRITE.load(Relaxed);
    let used = write.wrapping_sub(OUT_READ.load(Acquire));
    let missing = SILENCE_FILL.saturating_sub(used);
    for i in 0..missing {
        let at = (write.wrapping_add(i) % spare::OUT_FRAMES) as usize;
        // SAFETY: `at` is below OUT_FRAMES, so both samples are inside the output ring.
        unsafe {
            ptr::write_volatile(out.add(2 * at), 0.0);
            ptr::write_volatile(out.add(2 * at + 1), 0.0);
        }
    }
    OUT_WRITE.store(write.wrapping_add(missing), Release);
}

/// Bytes of the file waiting in the read ring, for the debug figures.
pub(super) fn ring_bytes() -> u32 {
    if !state().ready {
        return 0;
    }
    RING_WRITTEN.load(Relaxed).wrapping_sub(RING_TAKEN.load(Relaxed))
}
