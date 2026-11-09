use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32};

use bat_app::{BatApp, Buffers};

use super::{Slot, spare, stock};

// Task 1 only.
pub(super) static CALLS: AtomicU32 = AtomicU32::new(0);
pub(super) static CALLS_PER_S: AtomicU32 = AtomicU32::new(0);
pub(super) static LAST_SECOND: AtomicU32 = AtomicU32::new(0);
pub(super) static CALLS_AT_SECOND: AtomicU32 = AtomicU32::new(0);
pub(super) static LAST_EVENT: AtomicU32 = AtomicU32::new(0);
pub(super) static AUDIO_STATE: AtomicU32 = AtomicU32::new(AUDIO_NOT_TRIED);
pub(super) const AUDIO_NOT_TRIED: u32 = 0;
pub(super) const AUDIO_INSTALLED: u32 = 1;
pub(super) const AUDIO_SLOT_UNEXPECTED: u32 = 2;
pub(super) static APP_READY: AtomicBool = AtomicBool::new(false);
pub(super) static LAST_RENDER: AtomicU32 = AtomicU32::new(0);
pub(super) static SAVED_LITERAL: AtomicU32 = AtomicU32::new(0);
pub(super) static SAVED_BUFFER: AtomicU32 = AtomicU32::new(0);
/// Set from task 1 for the work to run on the patch stack.
pub(super) static PENDING: AtomicU32 = AtomicU32::new(0);
pub(super) const WORK_ENTER: u32 = 1;
/// The DSP task's id once created, or the error cre_tsk returned (negative).
pub(super) static TASK: AtomicU32 = AtomicU32::new(0);
/// Counts openings of the bat screen; the context running the app empties
/// the rings when it sees a new one.
pub(super) static SESSION: AtomicU32 = AtomicU32::new(0);
pub(super) static SESSION_SEEN: AtomicU32 = AtomicU32::new(0);
/// Button and wheel actions from task 1 for the app, applied by whichever
/// context runs it: (action << 24) | (value & 0xffffff).
pub(super) static ACTIONS: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
pub(super) static ACTIONS_WRITE: AtomicU32 = AtomicU32::new(0);
pub(super) static ACTIONS_READ: AtomicU32 = AtomicU32::new(0);
pub(super) const ACTION_MARK: u32 = 1;
pub(super) const ACTION_PLAY: u32 = 2;
pub(super) const ACTION_HOME: u32 = 3;
pub(super) const ACTION_WHEEL: u32 = 4;
/// Set when a bat tile was tapped: the bat screen opens once the C core has
/// switched to the app the tile asked for.
pub(super) static LAUNCHING: AtomicBool = AtomicBool::new(false);
/// The last bat tile tapped was BAT PLAYBACK: the app hears the file it plays
/// from the card (playback.rs) instead of the inputs turned on in MANUAL, and
/// PLAY, a short HOME press and REC go to the app, not to MANUAL.
pub(super) static PLAYBACK: AtomicBool = AtomicBool::new(false);
pub(super) static LAUNCH_FROM: AtomicU32 = AtomicU32::new(0);
pub(super) static LAUNCH_AT: AtomicU32 = AtomicU32::new(0);
/// The app (C numbering) the bat screen was opened in, or NO_APP. It stays
/// set while PLAY shows the app's own screen, and is cleared when the app
/// changes.
pub(super) static SCREEN_APP: AtomicU32 = AtomicU32::new(NO_APP);
pub(super) const NO_APP: u32 = 0x100;
/// The DSP task gave the display back for a pop-up; task 1 takes it again
/// once the pop-up has closed.
pub(super) static POPUP_HIDDEN: AtomicBool = AtomicBool::new(false);

// CPU share, in 1/1000 of the blocks in the last second (DSP task only).
pub(super) static DSP_BLOCKS: AtomicU32 = AtomicU32::new(0);
pub(super) static DRAW_BLOCKS: AtomicU32 = AtomicU32::new(0);
pub(super) static DSP_PERMILLE: AtomicU32 = AtomicU32::new(0);
pub(super) static DRAW_PERMILLE: AtomicU32 = AtomicU32::new(0);
/// The part of the drawing spent copying the frame to the display.
pub(super) static COPY_BLOCKS: AtomicU32 = AtomicU32::new(0);
pub(super) static COPY_PERMILLE: AtomicU32 = AtomicU32::new(0);
pub(super) static LOAD_SECOND: AtomicU32 = AtomicU32::new(0);
/// When the window of the latest published CPU share began, in blocks.
pub(super) static LOAD_WINDOW_START: AtomicU32 = AtomicU32::new(0);

// Shared with the audio interrupt.
pub(super) static BAT_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(super) static BLOCKS: AtomicU32 = AtomicU32::new(0);
pub(super) static IN_WRITE: AtomicU32 = AtomicU32::new(0); // interrupt
pub(super) static IN_READ: AtomicU32 = AtomicU32::new(0); // task 1
pub(super) static IN_DROPPED: AtomicU32 = AtomicU32::new(0); // interrupt
/// Where in the input ring the latest frames that did not fit were missing from
/// (a value of IN_WRITE), and how many of IN_DROPPED the context running the app
/// has told it about.
pub(super) static IN_HOLE_AT: AtomicU32 = AtomicU32::new(0); // interrupt
pub(super) static IN_DROPPED_SEEN: AtomicU32 = AtomicU32::new(0); // DSP task
/// Frames written to and read from the output ring. These keep counting past the ring's
/// size, and a frame's place in the ring is the count modulo `OUT_FRAMES`. 2^32 is not a
/// multiple of that, so when a count wraps, after 6.2 hours of sound on the bat screen, its
/// place moves back 4,096 frames. The write count wraps first. In the 8,192 frames until the
/// read count wraps too, about 21 ms of sound plays early, and then plays again in its right
/// place, with a click at each jump. The `%` keeps every place inside the ring, so nothing
/// else happens.
pub(super) static OUT_WRITE: AtomicU32 = AtomicU32::new(0); // DSP task, or task 1 without one
/// `enter_work` also sets these two while BAT_ACTIVE is false, when the interrupt leaves them alone.
pub(super) static OUT_READ: AtomicU32 = AtomicU32::new(0); // interrupt
pub(super) static PRIMED: AtomicBool = AtomicBool::new(false); // interrupt
pub(super) static UNDERRUNS: AtomicU32 = AtomicU32::new(0); // interrupt
/// The last output frame, left then right, as the bits of two f32s, and the frames
/// left of the fade after the output starts again.
pub(super) static OUT_LAST: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2]; // interrupt
pub(super) static OUT_FADE_IN: AtomicU32 = AtomicU32::new(0); // interrupt
pub(super) static BACKLOG_DROPPED: AtomicU32 = AtomicU32::new(0); // DSP task
/// Peak magnitude of each of the 12 words of an input frame since the last
/// redraw: the interrupt raises them, the drawing context reads and clears.
pub(super) static WORD_PEAKS: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
/// The same for the stock output, unscaled, whichever side is louder, when the
/// app hears MANUAL's mix; in BAT PLAYBACK, for the file playing.
pub(super) static SOURCE_PEAK: AtomicU32 = AtomicU32::new(0);
/// The words of an input frame the interrupt passes to the app, first | second
/// << 8, or `NO_INPUT_WORDS` to pass silence. Inputs 3 and 4 until
/// `update_status` first runs.
pub(super) static HEARD_WORDS: AtomicU32 = AtomicU32::new(stock::INPUT_WORDS[2] | stock::INPUT_WORDS[3] << 8);
pub(super) const NO_INPUT_WORDS: u32 = u32::MAX;
/// Listen to has MANUAL's mix chosen: the interrupt passes the stock output to
/// the app instead of those words.
pub(super) static HEAR_MIX: AtomicBool = AtomicBool::new(false);
/// The mix has been seen muted while stopped with REC PAUSE off, and the block count
/// when that was first seen. The context running the app sets them; task 1 clears the
/// first when the bat screen opens.
pub(super) static MUTE_SEEN: AtomicBool = AtomicBool::new(false);
pub(super) static MUTE_SEEN_AT: AtomicU32 = AtomicU32::new(0);
/// The debug figures are on (settings screen); only then does the interrupt
/// measure the input words.
pub(super) static DEBUG_SHOWN: AtomicBool = AtomicBool::new(false);
/// What the debug figures count since: a value for the settings being tested,
/// and the counters at the time they last changed.
pub(super) static TEST_SETTINGS: AtomicU32 = AtomicU32::new(u32::MAX);
pub(super) static TEST_START_BLOCKS: AtomicU32 = AtomicU32::new(0);
pub(super) static TEST_START_DRY: AtomicU32 = AtomicU32::new(0);
pub(super) static TEST_START_LOST: AtomicU32 = AtomicU32::new(0);
pub(super) static TEST_START_BACKLOG: AtomicU32 = AtomicU32::new(0);
pub(super) static TEST_START_SKIPPED: AtomicU32 = AtomicU32::new(0);
pub(super) static TEST_DSP_MAX: AtomicU32 = AtomicU32::new(0);
/// Core cycles in the last second: in all, in the stock audio callback, and in
/// the rest of the audio hook (interrupt; read by the drawing context).
pub(super) static CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
pub(super) static STOCK_CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
pub(super) static HOOK_CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
/// The audio hook that ran last was its copy in ITCM (see `use_itcm_hook`).
pub(super) static HOOK_IN_ITCM: AtomicBool = AtomicBool::new(false);
pub(super) static CYCLES_AT_SECOND: AtomicU32 = AtomicU32::new(0);
pub(super) static BLOCKS_AT_SECOND: AtomicU32 = AtomicU32::new(0);
pub(super) static STOCK_CYCLES: AtomicU32 = AtomicU32::new(0);
pub(super) static HOOK_CYCLES: AtomicU32 = AtomicU32::new(0);
pub(super) static CYCLE_COUNTER_ON: AtomicBool = AtomicBool::new(false);

/// The bat app: built in task 1 on the patch stack, then used only by the
/// DSP task (or, without one, by task 1). Its state is in DTCM; its large
/// buffers are in SDRAM.
#[unsafe(link_section = ".dtcm")]
pub(super) static APP: Slot<BatApp<'static>> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
#[unsafe(link_section = ".noinit")]
pub(super) static BUFFERS: Slot<Buffers> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

/// Checksum of the code copied into ITCM, to stop processing if anything
/// changes it (no stock code is known to use that part of ITCM).
pub(super) static ITCM_SUM: AtomicU32 = AtomicU32::new(0);
pub(super) static ITCM_OK: AtomicBool = AtomicBool::new(false);
pub(super) static ITCM_CHECKED_AT: AtomicU32 = AtomicU32::new(0);

/// The off-screen frame: cached, so drawing is fast, then copied to the
/// display buffer in one pass.
#[unsafe(link_section = ".noinit")]
pub(super) static FRAME: Slot<[u16; stock::WIDTH * stock::HEIGHT]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

/// The input ring: frames of the two inputs the app hears, from the interrupt.
#[unsafe(link_section = ".dtcm")]
pub(super) static IN_RING: Slot<[[i32; 2]; spare::IN_FRAMES as usize]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

pub(super) const DSP_STACK_BYTES: usize = 32 * 1024;
#[repr(C, align(8))]
pub(super) struct DspStack([u8; DSP_STACK_BYTES]);
#[unsafe(link_section = ".dtcm")]
pub(super) static DSP_STACK: Slot<DspStack> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

/// Where Qt's present function writes while the bat screen is up (it writes
/// at offsets 0x0, 0x10 and 0x4c of what it takes for the LCDIF registers).
pub(super) static DUMMY_LCDIF: Slot<[u32; 32]> = Slot(UnsafeCell::new(MaybeUninit::zeroed()));

// The stack task 1 runs the patch's heavy work on is `spare::PATCH_STACK`.

/// Touches for the app, from task 9: pressed << 31 | x << 16 | y.
pub(super) static TOUCHES: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
pub(super) static TOUCH_WRITE: AtomicU32 = AtomicU32::new(0);
pub(super) static TOUCH_READ: AtomicU32 = AtomicU32::new(0);
/// Qt has been sent a press without its release (task 9).
pub(super) static QT_PRESSED: AtomicBool = AtomicBool::new(false);
/// The context running the app: a finger is down, and it went down where a
/// drag tunes.
pub(super) static TOUCH_DOWN: AtomicBool = AtomicBool::new(false);
pub(super) static DRAGGING: AtomicBool = AtomicBool::new(false);
/// The app's MIX file switch is off, while BAT DETECTOR is open. The context running
/// the app sets it; `mix_file::open_take_hook` reads it when a take starts.
pub(super) static MIX_FILE_OFF: AtomicBool = AtomicBool::new(false);
/// The file mask has no master file for the take in progress (see `mix_file`).
pub(super) static TAKE_WITHOUT_MIX: AtomicBool = AtomicBool::new(false);
/// The app's GUANO switch is on (the default). The context running the app sets it;
/// `mix_file::open_take_hook` reads it when a take starts.
pub(super) static GUANO_ON: AtomicBool = AtomicBool::new(true);
/// The take in progress started from a bat tile with the GUANO switch on, so its WAV files get
/// GUANO metadata as they are closed (see `guano_file`). Set when a take's files are opened.
pub(super) static TAKE_GUANO: AtomicBool = AtomicBool::new(false);
/// The most input frames waiting at the start of a chunk since the CPU test last read this.
/// The context running the app both writes and reads it.
pub(super) static IN_WAITING_MOST: AtomicU32 = AtomicU32::new(0);
