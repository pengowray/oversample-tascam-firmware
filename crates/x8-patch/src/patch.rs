use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use core::ptr;

/// The launcher glyph for the bat tiles.
mod bat_glyph;
/// The audio hook, the cycle counter and `process_input`, which feeds the app from the input ring.
mod audio;
/// Which inputs the app hears, from the track settings.
mod inputs;
/// Copies the hot code into ITCM and checks that it stays unchanged.
mod itcm;
/// The key and touch hooks, and passing their events on to the app.
mod keys;
/// The bat tiles on the LAUNCHER: slots, texts, title position, delete mode, badges and icons.
mod launcher;
/// Opening and closing the bat screen, and drawing it with the debug strips.
mod screen;
/// SDRAM after Qt's framebuffers, 0x87ea6004 to 0x87ffffff: zeroed by no init
/// table entry and referred to by no stock code (checked in Ghidra). Mapped
/// uncached, so the display controller sees what is written without cache
/// maintenance.
mod spare;
/// The statics that the audio interrupt, the tasks and the hooks share. Reordering them moves the
/// stacks and rings relative to each other, and changes the offsets and register use in the generated
/// code, because LLVM merges the small ones into blocks in the order they are defined here.
mod state;
/// Stock firmware addresses, V2.03 build 217. See docs/HOMEBREW.md.
mod stock;
/// The DSP task, the patch stack and the bat app they run.
mod task;

use keys::{key_hook, rec_key_hook, touch_hook};
use launcher::{
    badge_hook, delete_hook, delete_mode_hook, free_slot_hook, icon_height_hook, icon_hook, icon_image_hook,
    icon_width_hook, slot_hook, text_hook, tile_hook, title_y_hook,
};

type Handler = unsafe extern "C" fn(usize, usize, usize, usize) -> usize;

/// What `tools/x8patch.py` reads to find the entry points: it
/// must stay at the start of the patch.
#[repr(C)]
pub struct Header {
    magic: [u8; 4],
    version: u32,
    /// Replaces the stock key handler of the launcher and app screens.
    key_hook: Handler,
    /// Replaces the recording screen's key handler.
    rec_key_hook: Handler,
    /// Replaces the touch poll's call that passes a touch to Qt.
    touch_hook: Handler,
    /// Replaces the USER SETTINGS 2 and 3 tiles' call that loads a preset.
    tile_hook: Handler,
    /// Replace the bridge's slot getter and first free slot (whole functions).
    slot_hook: Handler,
    free_slot_hook: Handler,
    /// Wrap the launcher's title and description line bindings.
    title_hook: Handler,
    line_hooks: [Handler; 6],
    /// Wraps the delete mode label binding.
    label_hook: Handler,
    /// Wraps the title's y binding.
    title_y_hook: Handler,
    /// Replaces the USER SETTINGS 2 and 3 tiles' tap handlers' read of the
    /// delete mode property.
    delete_mode_hook: Handler,
    /// Replaces the delete listener's tail call that deletes a preset.
    delete_hook: Handler,
    /// Wrap the USER SETTINGS 2 and 3 tiles' delete badge bindings.
    badge_hooks: [Handler; 2],
    /// Wrap the USER SETTINGS 2 and 3 tiles' icon visibility bindings.
    icon_hooks: [Handler; 2],
    /// Replace those tiles' icon image, height and width bindings.
    icon_image_hooks: [Handler; 2],
    icon_height_hooks: [Handler; 2],
    icon_width_hooks: [Handler; 2],
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".header")]
pub static X8_PATCH_HEADER: Header = Header {
    magic: *b"BATX",
    version: 14,
    key_hook,
    rec_key_hook,
    touch_hook,
    tile_hook,
    slot_hook,
    free_slot_hook,
    title_hook: text_hook::<0>,
    line_hooks: [text_hook::<1>, text_hook::<2>, text_hook::<3>, text_hook::<4>, text_hook::<5>, text_hook::<6>],
    label_hook: text_hook::<7>,
    title_y_hook,
    delete_mode_hook,
    delete_hook,
    badge_hooks: [badge_hook::<0>, badge_hook::<1>],
    icon_hooks: [icon_hook::<0>, icon_hook::<1>],
    icon_image_hooks: [icon_image_hook::<0>, icon_image_hook::<1>],
    icon_height_hooks: [icon_height_hook::<0>, icon_height_hook::<1>],
    icon_width_hooks: [icon_width_hook::<0>, icon_width_hook::<1>],
};

/// Statics written by one context at a time, as the comments on each use say.
struct Slot<T>(UnsafeCell<MaybeUninit<T>>);
// SAFETY: see each use.
unsafe impl<T> Sync for Slot<T> {}

fn system_mode() -> u8 {
    // SAFETY: a byte the stock firmware sets before the RTOS starts.
    unsafe { ptr::read_volatile(stock::SYSTEM_MODE) }
}

fn rate_class() -> u8 {
    // SAFETY: a byte in the stock DSP state.
    unsafe { ptr::read_volatile(stock::RATE_CLASS) }
}

fn blocks_per_second() -> u32 {
    if rate_class() == 0 { 24_000 } else { 48_000 }
}

/// Bytes the stock firmware keeps in RAM; single byte reads.
fn read_byte(at: *const u8) -> u8 {
    // SAFETY: addresses named in `stock`.
    unsafe { ptr::read_volatile(at) }
}

fn app_byte() -> u8 {
    read_byte(stock::APP)
}

fn system_state() -> u8 {
    read_byte(stock::SYSTEM_STATE)
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
