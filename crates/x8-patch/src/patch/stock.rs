/// The system mode: 0 or 1 for a normal start, 3 in firmware update mode.
pub const SYSTEM_MODE: *const u8 = 0x8196_9dd9 as *const u8;
/// The key handler the launcher and app screens share (Thumb address).
pub const KEY_HANDLER: usize = 0x80da_c0d5;
/// Qt's two framebuffers: 320 x 480, RGB565, mapped uncached by the MPU.
pub const FRAMEBUFFERS: [usize; 2] = [0x87e0_0000, 0x87e4_b000];
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 480;

/// Literal that Qt's present function (0x80e2b574) uses as the LCDIF
/// register base for its writes, NEXT_BUF among them (base + 0x4c).
/// Nothing else reads it.
pub const PRESENT_LCDIF_LITERAL: *mut usize = 0x80e2_b71c as *mut usize;
pub const LCDIF_CTRL_SET: usize = 0x402b_8004;
/// The display controller's current and next buffer registers.
pub const LCDIF_CUR_BUF: *const usize = 0x402b_8040 as *const usize;
pub const LCDIF_NEXT_BUF: *mut usize = 0x402b_8050 as *mut usize;
/// `FUN_80e0364a(x, y, pressed)`: posts a touch to Qt's event queue;
/// the touch poll (task 9) calls it on every change (Thumb address).
pub const QT_TOUCH: usize = 0x80e0_364b;

/// Where `EDMA_SetCallback` stored the audio callback: the channel 9
/// (SAI1 receive) eDMA handle's first word.
pub const AUDIO_CALLBACK_SLOT: *mut usize = 0x2001_024c as *mut usize;
/// The stock audio callback (ITCM, Thumb address).
pub const AUDIO_CALLBACK: usize = 0x0002_adfd;
/// Ring index of the input block the callback is about to process.
pub const AUDIO_INDEX: *const u32 = 0x2001_0268 as *const u32;
/// Three input blocks of 0x180 bytes: per frame 12 signed 32-bit words.
pub const INPUT_BLOCKS: usize = 0x2000_e340;
/// The word in an input frame that carries each of inputs 1 to 6 (the
/// high gain path of its dual-gain pair). EXT IN uses the words of inputs
/// 5 and 6.
pub const INPUT_WORDS: [u32; 6] = [2, 5, 0, 6, 8, 11];
/// DSP state + 0x622: 1 when tracks hear inputs 1 and 2 swapped.
pub const SWAP_INPUTS_1_2: *const u8 = 0x0002_a2ce as *const u8;
/// Settings + 0x30, one byte every 0x20 bytes for tracks 1 to 6: the pair
/// of inputs the track hears (INPUT SELECT), 1 INPUT 1-2, 2 INPUT 3-4,
/// 3 INPUT 5-6, 4 EXT IN. An odd track hears the left of the pair.
pub const TRACK_SOURCES: *const u8 = 0x81e8_c8ac as *const u8;
pub const TRACK_SOURCE_STEP: usize = 0x20;
pub const TRACKS: usize = 6;
/// Settings + 0x200 and + 0x204: bit 0 to 5 set in either word when track
/// 1 to 6 is turned on (red on MANUAL's INPUT screen).
pub const TRACKS_ON: [*const u32; 2] = [0x81e8_ca7c as *const u32, 0x81e8_ca80 as *const u32];
/// Three output blocks of 0x40 bytes: per frame left, right.
pub const OUTPUT_BLOCKS: usize = 0x2001_04c0;
/// Rate class: 0 for 44.1/48 kHz, 1 for 88.2/96, 2 for 176.4/192.
pub const RATE_CLASS: *const u8 = 0x0002_a2cb as *const u8;
/// The output gain (f32), smoothed once per block towards -10^(-v/20)
/// for the 0 to 10 dB output level setting (settings + 0x253). It falls
/// towards 0 only while a sample rate change mutes the output.
pub const OUTPUT_GAIN: *const f32 = 0x0002_9ed8 as *const f32;
/// DSP state + 0x62d: 1 while the transport plays a file into the mix.
pub const DSP_PLAYING: *const u8 = 0x0002_a2d9 as *const u8;

/// Message fields (a copy on task 1's stack): +0 category (0 = input),
/// +4 kind (0 button, 2 wheel, 9 swallowed), +8 button code or count.
pub const KIND_BUTTON: u16 = 0;
pub const KIND_WHEEL: u16 = 2;
pub const KIND_SWALLOWED: u16 = 9;
pub const NO_KEY: i32 = 0x4d;
pub const PLAY: i32 = 0x05;

/// The RTOS's uITRON calls (ITCM, Thumb addresses). `cre_tsk(id, pk)`
/// accepts ids 1 to 23 (the maximum is at 0x20004148), returns -41 when
/// the id is taken, and starts the task at once with attribute TA_ACT.
/// `dly_tsk(ms)` sleeps the calling task (the kernel tick is 2 ms).
pub const CRE_TSK: usize = 0x0002_d4a3;
pub const DLY_TSK: usize = 0x0001_a7f5;
pub const HOME: i32 = 0x06;
pub const MARK: i32 = 0x2b;

/// The recording screen's key handler, installed from record standby to
/// the end of a take in place of KEY_HANDLER (Thumb address).
pub const REC_KEY_HANDLER: usize = 0x80dd_3f9d;
/// Qt's bridge setter `FUN_80d6946c(bridge, property, value, from_c)`;
/// property 6 opens an app by its Qt number (Thumb address).
pub const BRIDGE_SET: usize = 0x80d6_946d;
pub const APP_PROPERTY: usize = 6;
pub const QT_MANUAL: usize = 1;
/// `FUN_80d6a1c0(bridge, slot)`: loads a USER SETTINGS preset, slot 0 to 2
/// (Thumb address).
pub const LOAD_USER_SETTINGS: usize = 0x80d6_a1c1;
/// `FUN_80d6a0f8(bridge, slot, app, from_c)`: with from_c = 0 and app 0,
/// asks the C core to delete a preset (Thumb address).
pub const DELETE_USER_SETTINGS: usize = 0x80d6_a0f9;
/// The bridge's USER SETTINGS slots: per slot, the Qt number of the app
/// whose settings it holds (1 MANUAL, 2 PODCAST, 9 POCKET STUDIO), or 0.
pub const USER_SETTINGS_SLOTS: *const u32 = 0x81e9_da60 as *const u32;
/// The launcher's cursor, an int property at root + 0xd360: 1 MANUAL to
/// 14 POCKET STUDIO, with 11 to 13 for USER SETTINGS 1 to 3.
pub const CURSOR_PROPERTY: usize = 0xd360;
pub const CURSOR_USER_SETTINGS_1: i32 = 11;
/// `FUN_80e02a34(property)`: a pointer to a Qt property's value, noting
/// the read for the binding being evaluated (Thumb address).
pub const PROPERTY_VALUE: usize = 0x80e0_2a35;
/// `FUN_80e3ea10(to, from)` assigns a Qt for MCUs string and
/// `FUN_80e03154(string)` releases one (Thumb addresses).
pub const STRING_ASSIGN: usize = 0x80e3_ea11;
pub const STRING_RELEASE: usize = 0x80e0_3155;
/// The launcher's text bindings `(out, binding, root)`: the title,
/// description lines 1 to 6, and the label shown over the title in delete
/// mode (Thumb addresses).
pub const LAUNCHER_TEXTS: [usize; 8] =
    [0x80f2_326b, 0x80f2_393d, 0x80f2_3da9, 0x80f2_4225, 0x80f2_4695, 0x80f2_4ad9, 0x80f2_4e59, 0x80f2_9139];
/// `FUN_80f23220(binding, root)`: the title's y, (H + 1) / 2 - 100, or
/// - 25 for the USER SETTINGS tiles, which have no description (Thumb).
pub const TITLE_Y: usize = 0x80f2_3221;
pub const USER_SETTINGS_TITLE_DROP: usize = 75;
/// The USER SETTINGS 2 and 3 tiles' delete badges `(binding, root)`:
/// slot in use and delete mode on (Thumb addresses).
pub const DELETE_BADGES: [usize; 2] = [0x80f2_a7e5, 0x80f2_b2b9];
/// The USER SETTINGS 2 and 3 tiles' icon visibility `(binding, root)`:
/// slot in use and delete mode off (Thumb addresses).
pub const ICONS_SHOWN: [usize; 2] = [0x80f2_aa41, 0x80f2_b511];
/// Those tiles' icon `(out, binding, root)`, which writes an image pointer to
/// `out`, and its height and width `(binding, root)`. The icon's x and y centre
/// it from its size (Thumb addresses).
pub const ICON_IMAGES: [usize; 2] = [0x80f2_a80d, 0x80f2_b2e1];
pub const ICON_HEIGHTS: [usize; 2] = [0x80f2_a969, 0x80f2_b439];
pub const ICON_WIDTHS: [usize; 2] = [0x80f2_a9d5, 0x80f2_b4a5];
/// Settings + 0x194: 0 when REC PAUSE is on, so the firmware mutes every
/// input in the mix while the recorder is stopped (ITCM 0x161d8).
pub const MONITOR_WHEN_STOPPED: *const u8 = 0x81e8_ca10 as *const u8;
/// `FUN_80e05870(property, &value)`: sets a Qt property (Thumb address).
pub const SET_PROPERTY: usize = 0x80e0_5871;
/// The audio engine: 1 playing, 2 recording, 0 idle or paused.
pub const ENGINE_STATE: *const u32 = 0x81cb_e780 as *const u32;
/// The open app in the C core's numbering: 0 the LAUNCHER, 2 MANUAL.
pub const APP: *const u8 = 0x81e8_ca90 as *const u8;
pub const C_MANUAL: u8 = 2;
/// Task 1's system state: 1 app screens, 2 recording screen, 6 a pop-up
/// (which installs its own key handler).
pub const SYSTEM_STATE: *const u8 = 0x8196_a3e0 as *const u8;
pub const STATE_POPUP: u8 = 6;
/// The recorder: 0 stopped, 1 and 2 standby, 3 recording, 4 paused,
/// 5 closing the file, 6 paused by an event, 7 leaving standby, 8
/// self-timer.
pub const TRANSPORT: *const u8 = 0x8196_a3e5 as *const u8;
/// Frames in the current take, as `FUN_80dae75c` works it out: the frozen
/// copy when the flag is set, otherwise offset + count - start.
pub const TAKE_FROZEN_FLAG: *const u8 = 0x8191_3f70 as *const u8;
pub const TAKE_FROZEN: *const u32 = 0x8191_448c as *const u32;
pub const TAKE_OFFSET: *const u32 = 0x8191_40c4 as *const u32;
pub const TAKE_COUNT: *const u32 = 0x8191_4178 as *const u32;
pub const TAKE_START: *const u32 = 0x8191_3f60 as *const u32;
/// The rate the converters run at now (Hz), and the record rate setting.
pub const CONVERTER_RATE: *const u32 = 0x8191_4120 as *const u32;
pub const RECORD_RATE: *const u32 = 0x81cb_e678 as *const u32;
