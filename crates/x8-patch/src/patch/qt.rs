//! Qt for MCUs image descriptors, as the stock images are stored, for images the patch makes.

use super::stock;

/// A Qt for MCUs image as the launcher's icons are stored (for example
/// lc_manual.png, whose descriptor is at 0x819a188c). Static constructors fill
/// in the stock images' colour and their tiles' layout at start-up, so a dump
/// of the data before they run shows both as 0; these statics must set them
/// here. An image pointer with no flag bits set is not reference counted.
#[repr(C, align(4))]
pub(super) struct QtImage {
    /// 0xAARRGGBB, which the alpha mask is drawn in.
    pub colour: u32,
    pub width: u32,
    pub height: u32,
    pub tile_count: u32,
    pub tiles: *const QtTile,
}

/// One tile of an image. The draw path asserts that `layout` is set, and with
/// `flags`, `cache_size` and `cache_entry` 0 nothing writes to this.
#[repr(C)]
pub(super) struct QtTile {
    pub pixels: *const u8,
    pub layout: *const u8,
    pub flags: u32,
    /// y << 16 | x within the image, each 16 bits and signed.
    pub offset: u32,
    /// height << 16 | width.
    pub size: u32,
    pub stride: u32,
    /// Pixels left out at each side of the tile, one byte each, as in the stock tiles of
    /// "Portacapture X8" (0x01010101, with `flags` 2).
    pub trim: u32,
    pub cache_size: u32,
    pub cache_entry: u32,
}

impl QtTile {
    /// An 8-bit alpha tile of `width` x `height` pixels, one byte each, at `x`, `y` in its image.
    pub const fn alpha8(pixels: *const u8, x: i16, y: i16, width: u32, height: u32) -> Self {
        QtTile {
            pixels,
            layout: stock::ALPHA8_LAYOUT,
            flags: 0,
            offset: (y as u16 as u32) << 16 | x as u16 as u32,
            size: height << 16 | width,
            stride: width,
            trim: 0,
            cache_size: 0,
            cache_entry: 0,
        }
    }
}

// SAFETY: read only, and the pointers are to statics and stock data.
unsafe impl Sync for QtImage {}
unsafe impl Sync for QtTile {}
