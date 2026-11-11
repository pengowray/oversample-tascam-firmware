use core::ptr;
use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use super::qt::{QtImage, QtTile};
use super::state::{BLOCKS, LAUNCH_AT, LAUNCH_FROM, LAUNCHING, PLAYBACK, PLAYBACK_OPENING, SCREEN_APP};
use super::{Handler, app_byte, bat_glyph, blocks_per_second, play_glyph, stock, system_mode};

/// The bat tiles, in USER SETTINGS slots 2 and 3 (slots 0 to 2 in the
/// firmware's numbering). Slot 1 stays a USER SETTINGS preset.
const RECORDER_SLOT: usize = 1;
const PLAYBACK_SLOT: usize = 2;

/// Each bat tile's launcher texts: its title, then description lines 1 to 6
/// (ASCII; empty for an unused line).
const TILE_TEXTS: [[&[u8]; 7]; 2] = [
    [
        b"BAT DETECTOR",
        b"Hear and record bat calls",
        b"live, and see them on a",
        b"spectrogram. In MANUAL,",
        b"turn off inputs without",
        b"ultrasonic mics. Set the",
        b"sample rate to 192kHz.",
    ],
    [
        b"BAT PLAYBACK",
        b"Hear bat calls in 192kHz",
        b"recordings. PLAY plays or",
        b"pauses the current file in",
        b"MANUAL. To choose another",
        b"file, press HOME briefly.",
        b"Experimental and untested.",
    ],
];

fn bat_tile(slot: usize) -> Option<usize> {
    match slot {
        RECORDER_SLOT => Some(0),
        PLAYBACK_SLOT => Some(1),
        _ => None,
    }
}

/// Replaces the USER SETTINGS 2 and 3 tiles' tail call that loads a preset
/// (on the Qt task, with r0 = bridge and r1 = the slot): opens MANUAL, as
/// its own tile does, so the C core runs a recording app, and has task 1 put
/// the bat screen over it.
pub(super) unsafe extern "C" fn tile_hook(bridge: usize, slot: usize, c: usize, d: usize) -> usize {
    let Some(tile) = bat_tile(slot).filter(|_| system_mode() <= 1) else {
        // SAFETY: the stock function the tile called, with its arguments.
        let load: Handler = unsafe { core::mem::transmute(stock::LOAD_USER_SETTINGS) };
        return unsafe { load(bridge, slot, c, d) };
    };
    PLAYBACK.store(tile == 1, Relaxed);
    LAUNCH_FROM.store(app_byte() as u32, Relaxed);
    LAUNCH_AT.store(BLOCKS.load(Relaxed), Relaxed);
    PLAYBACK_OPENING.store(tile == 1, Release);
    LAUNCHING.store(true, Release);
    // SAFETY: the bridge setter, called as the MANUAL tile calls it.
    let set: Handler = unsafe { core::mem::transmute(stock::BRIDGE_SET) };
    unsafe { set(bridge, stock::APP_PROPERTY, stock::QT_MANUAL, 0) }
}

/// Replaces the power-off sequence's call that saves the settings. If a bat
/// tile opened the current app, the record is saved with the LAUNCHER as the
/// open app, so the next start does not open MANUAL's own screen. The stock
/// firmware saves the same value when powered off on the LAUNCHER.
///
/// The live app byte changes only for the length of the save. Interrupts are
/// off during this call, so no task can read it. The settings save task's own
/// call is left alone because that task runs alongside task 1, which would
/// read the LAUNCHER as the open app and close the bat screen.
///
/// When the patch has set POWER SOURCE SELECT to BATTERY for a weak USB supply, the record is saved
/// with AUTO, the user's setting.
pub(super) unsafe extern "C" fn power_off_save_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    if let Some(source) = super::power::saved_power_source() {
        // SAFETY: the POWER SOURCE SELECT byte in the settings record; interrupts are off, and the
        // X8 is powering off, so the power path that the RAM byte no longer matches does not matter.
        unsafe { ptr::write_volatile(stock::POWER_SOURCE, source) };
    }
    // SAFETY: the stock function the power-off sequence called, with its arguments.
    let save: Handler = unsafe { core::mem::transmute(stock::SAVE_SETTINGS) };
    let app = app_byte();
    if SCREEN_APP.load(Relaxed) != app as u32 {
        return unsafe { save(a, b, c, d) };
    }
    // SAFETY: the app byte in the settings record; nothing else runs until
    // interrupts are back on, by which time it holds its old value again.
    unsafe {
        ptr::write_volatile(stock::APP as *mut u8, stock::C_LAUNCHER);
        let result = save(a, b, c, d);
        ptr::write_volatile(stock::APP as *mut u8, app);
        result
    }
}

/// How long after a tap on BAT PLAYBACK the app change can still come.
const PLAYBACK_OPENING_S: u32 = 5;

/// Replaces the app change's call to `FUN_80dc565c`, which turns phantom power off (in the system
/// task, after the opening app's stored inputs with phantom power have been marked as pending).
/// When BAT PLAYBACK opens MANUAL, the file player needs no phantom power, so the question is not
/// asked: the pending marks are cleared, and MANUAL's stored phantom power, which the stock call
/// writes 0 to, is put back, so MANUAL and BAT DETECTOR still ask when they next open. In the
/// stock firmware only YES puts it back. Phantom power stays off, as after NO.
pub(super) unsafe extern "C" fn phantom_off_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock function the app change called, with its arguments.
    let all_off: Handler = unsafe { core::mem::transmute(stock::PHANTOM_ALL_OFF) };
    let since_tap = BLOCKS.load(Relaxed).wrapping_sub(LAUNCH_AT.load(Relaxed));
    let quiet = PLAYBACK_OPENING.swap(false, Acquire)
        && app_byte() == stock::C_MANUAL
        && system_mode() <= 1
        && since_tap < PLAYBACK_OPENING_S * blocks_per_second();
    if !quiet {
        return unsafe { all_off(a, b, c, d) };
    }
    // SAFETY: MANUAL's four stored phantom bytes and the pending flags (see `stock`), which the
    // system task writes, and this runs in the system task. Byte access: the four bytes are not
    // word aligned.
    unsafe {
        let stored: [u8; 4] = core::array::from_fn(|i| ptr::read_volatile(stock::MANUAL_PHANTOM.add(i)));
        let result = all_off(a, b, c, d);
        for (i, &on) in stored.iter().enumerate() {
            ptr::write_volatile(stock::MANUAL_PHANTOM.add(i), on);
        }
        ptr::write_volatile(stock::PHANTOM_PENDING, 0);
        result
    }
}

/// What a USER SETTINGS slot holds, as the launcher sees it: the bat tiles'
/// slots hold MANUAL settings, so the launcher shows them, and the icon hooks
/// give their images; slot 0 is the stock slot.
fn slot_value(slot: usize) -> u32 {
    if bat_tile(slot).is_some() && system_mode() <= 1 {
        return stock::QT_MANUAL as u32;
    }
    // SAFETY: the bridge's slot words (stock callers pass slots 0 to 2).
    unsafe { ptr::read_volatile(stock::USER_SETTINGS_SLOTS.add(slot)) }
}

/// Replaces the bridge's slot getter `FUN_80d6a238(bridge, slot)`, whose only
/// callers are the launcher's slot properties: slot 3 asks whether any slot
/// is in use.
pub(super) unsafe extern "C" fn slot_hook(_bridge: usize, slot: usize, _: usize, _: usize) -> usize {
    if slot == 3 {
        ((slot_value(0) | slot_value(1) | slot_value(2)) & 0xff) as usize
    } else {
        slot_value(slot) as usize
    }
}

/// Replaces the bridge's first free slot `FUN_80d6a252()`, which the apps'
/// save screens read: 1 to 3, or 0 when every slot is in use. Only slot 0 is
/// offered, so a save never goes to a slot hidden behind a bat tile.
pub(super) unsafe extern "C" fn free_slot_hook(_: usize, _: usize, _: usize, _: usize) -> usize {
    (1..=3).find(|&n| slot_value(n - 1) == 0).unwrap_or(0)
}

/// A Qt for MCUs string as the launcher's bindings build one from a literal:
/// the text at +0, and length << 8 | kind at +0x18 (kind 0: static text,
/// which releasing leaves alone).
#[repr(C)]
struct QtString {
    text: *const u8,
    unused: [u32; 5],
    length_kind: u32,
}

/// The bat tile the launcher's cursor is on, read as the launcher's
/// bindings read it (from a binding, this notes the read).
fn tile_at_cursor(root: usize) -> Option<usize> {
    // SAFETY: the cursor property of the launcher's root.
    let value: unsafe extern "C" fn(usize) -> *const i32 = unsafe { core::mem::transmute(stock::PROPERTY_VALUE) };
    let cursor = unsafe { ptr::read_volatile(value(root + stock::CURSOR_PROPERTY)) };
    usize::try_from(cursor - stock::CURSOR_USER_SETTINGS_1).ok().and_then(bat_tile)
}

/// A zero byte, the text of an empty string.
static EMPTY_TEXT: [u8; 1] = [0];

/// Wraps one of the launcher's text bindings (`LINE` 0 is the title, 1 to 6
/// the description lines, 7 the delete mode label): with the cursor on a bat
/// tile, the text is the tile's instead of "USER SETTINGS 2" or "3" and its
/// description. The delete mode label is drawn where a USER SETTINGS tile's
/// title is, so a bat tile leaves it empty, as the app tiles do.
pub(super) unsafe extern "C" fn text_hook<const LINE: usize>(
    out: usize,
    binding: usize,
    root: usize,
    d: usize,
) -> usize {
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::LAUNCHER_TEXTS[LINE]) };
    let result = unsafe { binding_fn(out, binding, root, d) };
    if system_mode() > 1 {
        return result;
    }
    if let Some(tile) = tile_at_cursor(root) {
        let text = if LINE == 7 { &[][..] } else { TILE_TEXTS[tile][LINE] };
        let pointer = if text.is_empty() { EMPTY_TEXT.as_ptr() } else { text.as_ptr() };
        let from = QtString { text: pointer, unused: [0; 5], length_kind: (text.len() as u32) << 8 };
        // SAFETY: as the bindings assign a literal to their result and then
        // release the temporary.
        unsafe {
            let assign: unsafe extern "C" fn(usize, *const QtString) = core::mem::transmute(stock::STRING_ASSIGN);
            let release: unsafe extern "C" fn(*const QtString) = core::mem::transmute(stock::STRING_RELEASE);
            assign(out, &from);
            release(&from);
        }
    }
    result
}

/// Wraps the title's y binding: on a bat tile, which has a description, the
/// title (and the description lines placed under it) sits where it does on
/// the app tiles.
pub(super) unsafe extern "C" fn title_y_hook(binding: usize, root: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::TITLE_Y) };
    let y = unsafe { binding_fn(binding, root, c, d) };
    if system_mode() <= 1 && tile_at_cursor(root).is_some() {
        y.wrapping_sub(stock::USER_SETTINGS_TITLE_DROP)
    } else {
        y
    }
}

/// An int property value of 0, for `delete_mode_hook`.
static NOT_DELETE_MODE: i32 = 0;

/// Replaces the read of the delete mode property (root + 0xd370) in the
/// USER SETTINGS 2 and 3 tiles' tap handlers: a bat tile is never deleted,
/// so a tap in delete mode ends delete mode, as a tap on an app tile does,
/// and opens the tile.
pub(super) unsafe extern "C" fn delete_mode_hook(property: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        // SAFETY: the stock property setter, called as the app tiles' tap
        // handlers call it on this property.
        let set: unsafe extern "C" fn(usize, *const i32) = unsafe { core::mem::transmute(stock::SET_PROPERTY) };
        unsafe { set(property, &NOT_DELETE_MODE) };
        return &raw const NOT_DELETE_MODE as usize;
    }
    // SAFETY: the stock function the handler called, with its arguments.
    let value: Handler = unsafe { core::mem::transmute(stock::PROPERTY_VALUE) };
    unsafe { value(property, b, c, d) }
}

/// Replaces the delete listener's tail call `FUN_80d6a0f8(bridge, slot, 0, 0)`
/// (after YES in the delete pop-up): a bat tile's slot is never deleted, as
/// that would delete a preset hidden behind the tile.
pub(super) unsafe extern "C" fn delete_hook(bridge: usize, slot: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 && bat_tile(slot).is_some() {
        return 0;
    }
    // SAFETY: the stock function the listener called, with its arguments.
    let delete: Handler = unsafe { core::mem::transmute(stock::DELETE_USER_SETTINGS) };
    unsafe { delete(bridge, slot, c, d) }
}

/// Wraps the delete badge binding of USER SETTINGS 2 (`TILE` 0) or 3 (1):
/// the bat tiles never show it.
pub(super) unsafe extern "C" fn badge_hook<const TILE: usize>(
    binding: usize,
    root: usize,
    c: usize,
    d: usize,
) -> usize {
    if system_mode() <= 1 {
        return 0;
    }
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::DELETE_BADGES[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}

/// Wraps the icon visibility binding of USER SETTINGS 2 (`TILE` 0) or 3 (1),
/// which hides the icon in delete mode for the badge: the bat tiles always
/// show their icon.
pub(super) unsafe extern "C" fn icon_hook<const TILE: usize>(binding: usize, root: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        return 1;
    }
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::ICONS_SHOWN[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}

// The icon height and width hooks give one size for both tiles.
const _: () = assert!(bat_glyph::WIDTH == play_glyph::WIDTH && bat_glyph::HEIGHT == play_glyph::HEIGHT);

const fn glyph_tile(pixels: *const u8) -> QtTile {
    QtTile::alpha8(pixels, 0, 0, bat_glyph::WIDTH, bat_glyph::HEIGHT)
}

const fn glyph_image(tile: *const QtTile) -> QtImage {
    QtImage {
        colour: stock::ICON_COLOUR,
        width: bat_glyph::WIDTH,
        height: bat_glyph::HEIGHT,
        tile_count: 1,
        tiles: tile,
    }
}

static BAT_TILE: QtTile = glyph_tile(bat_glyph::PIXELS.as_ptr());
static PLAY_TILE: QtTile = glyph_tile(play_glyph::PIXELS.as_ptr());
/// The tiles' icons: BAT DETECTOR, then BAT PLAYBACK.
static TILE_IMAGES: [QtImage; 2] = [glyph_image(&raw const BAT_TILE), glyph_image(&raw const PLAY_TILE)];

/// Replaces the icon binding of USER SETTINGS 2 (`TILE` 0) or 3 (1): BAT DETECTOR
/// shows the bat, and BAT PLAYBACK shows a play button in the same colour.
pub(super) unsafe extern "C" fn icon_image_hook<const TILE: usize>(
    out: usize,
    binding: usize,
    root: usize,
    d: usize,
) -> usize {
    if system_mode() <= 1 {
        // SAFETY: `out` is where the stock binding writes the image pointer.
        unsafe { ptr::write(out as *mut *const QtImage, &raw const TILE_IMAGES[TILE]) };
        return 0;
    }
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::ICON_IMAGES[TILE]) };
    unsafe { binding_fn(out, binding, root, d) }
}

/// Replaces those tiles' icon height binding.
pub(super) unsafe extern "C" fn icon_height_hook<const TILE: usize>(
    binding: usize,
    root: usize,
    c: usize,
    d: usize,
) -> usize {
    if system_mode() <= 1 {
        return bat_glyph::HEIGHT as usize;
    }
    // SAFETY: as above.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::ICON_HEIGHTS[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}

/// Replaces those tiles' icon width binding.
pub(super) unsafe extern "C" fn icon_width_hook<const TILE: usize>(
    binding: usize,
    root: usize,
    c: usize,
    d: usize,
) -> usize {
    if system_mode() <= 1 {
        return bat_glyph::WIDTH as usize;
    }
    // SAFETY: as above.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::ICON_WIDTHS[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}
