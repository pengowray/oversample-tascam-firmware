use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use core::ptr;

/// The audio hook, the cycle counter and `process_input`, which feeds the app from the input ring.
mod audio;
/// BAT DETECTOR's launcher glyph.
mod bat_glyph;
/// Sets the X8's clock from the phone's chimes, and keeps the phone's time for GUANO metadata.
mod clock;
/// Adds GUANO metadata to the WAV files of takes from a bat tile as the stock recorder closes them.
mod guano_file;
/// Which inputs the app hears, from the track settings.
mod inputs;
/// Copies the hot code into ITCM and checks that it stays unchanged.
mod itcm;
/// The key and touch hooks, and passing their events on to the app.
mod keys;
/// The bat tiles on the LAUNCHER: slots, texts, title position, delete mode, badges and icons.
mod launcher;
/// Keeps the phone's location from chimes for GUANO metadata, and logs it to the card.
mod location;
/// The logo screen's lower image: "Portacapture X8", the bat and "Oversample Edition".
mod logo;
/// The logo screen's layout and new masks, from tools/make_splash.py.
mod logo_pixels;
/// Leaves the MIX file out of takes from a bat tile, when the app's switch is off.
mod mix_file;
/// Keeps REC from pausing bat takes unless allowed, and counts the pauses that happen.
mod pauses;
/// BAT PLAYBACK's launcher glyph.
mod play_glyph;
/// BAT PLAYBACK's player, in the DSP task: passes requests to the card reader and plays the file.
mod playback;
/// Qt for MCUs image descriptors.
mod qt;
/// BAT PLAYBACK's card reader task: lists the WAV files, reads their headers and the file playing.
mod reader;
/// Carries out the phone remote's commands by posting button presses to task 1.
mod remote;
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
/// 192 kHz stereo over USB: the rate list, INPUT 3 and 4 into the USB ring, and the 8-channel guard.
mod usb;

use guano_file::guano_hook;
use keys::{key_hook, rec_key_hook, touch_hook};
use launcher::{
    badge_hook, delete_hook, delete_mode_hook, free_slot_hook, icon_height_hook, icon_hook, icon_image_hook,
    icon_width_hook, power_off_save_hook, slot_hook, text_hook, tile_hook, title_y_hook,
};
use mix_file::{file_mask_hook, open_take_hook};
use usb::{usb_alt_hook, usb_init_hook};

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
    /// Replaces the power-off sequence's call that saves the settings.
    power_off_save_hook: Handler,
    /// Replaces the call that opens a take's files for recording.
    open_take_hook: Handler,
    /// Replaces the file mask function's call to the remaining time function.
    file_mask_hook: Handler,
    /// Replaces the WAV header rewrite's call to the file size function, at the end of a take or a split.
    guano_hook: Handler,
    /// Replaces the USB task's call to the device stack's class init.
    usb_init_hook: Handler,
    /// Replaces the device callback's call to interface 2's SET_INTERFACE handler.
    usb_alt_hook: Handler,
    /// The handle that the logo screen's lower image item gets, and the y positions of its upper and
    /// lower items. tools/x8patch.py reads the lower item's height from the image.
    logo_lower: &'static &'static qt::QtImage,
    logo_upper_y: u32,
    logo_lower_y: u32,
    /// The build number on the update screen. 0 here: `tools/x8patch.py` writes it into the
    /// update file, so read it with [`patch_build`].
    build: u32,
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".header")]
pub static X8_PATCH_HEADER: Header = Header {
    magic: *b"BATX",
    version: 20,
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
    power_off_save_hook,
    open_take_hook,
    file_mask_hook,
    guano_hook,
    usb_init_hook,
    usb_alt_hook,
    logo_lower: &logo::LOWER_HANDLE,
    logo_upper_y: logo_pixels::UPPER_Y,
    logo_lower_y: logo_pixels::LOWER_Y,
    build: 0,
};

/// The build number `tools/x8patch.py` wrote into the header, as the update screen shows it.
fn patch_build() -> u32 {
    // SAFETY: a word of the patch's own header. Volatile, because the compiler sees only the 0
    // above, and the packer changes it after linking.
    unsafe { ptr::read_volatile(&raw const X8_PATCH_HEADER.build) }
}

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
