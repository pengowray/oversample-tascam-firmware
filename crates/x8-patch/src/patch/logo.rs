//! The logo screen at start-up and power-off: TASCAM's logo and "Portacapture X8" as stock, then
//! the Oversample bat and "Oversample Edition" below them. No code here runs. tools/x8patch.py
//! points the literal that the screen's constructor reads the lower image's handle from at
//! [`LOWER_HANDLE`], sets the lower item's height to this image's, and moves both items to
//! [`logo_pixels::UPPER_Y`] and [`logo_pixels::LOWER_Y`]. tools/make_splash.py explains the layout.
//!
//! The descriptors are in the patch region, which the bootloader copies into SDRAM before any mode
//! starts, so they are in place in update mode too.

use super::logo_pixels as px;
use super::qt::{QtImage, QtTile};
use super::stock;

/// The stock lower image's width, which its item centres from; the new image keeps it.
const LOWER_WIDTH: u32 = 243;
/// One for the margin all round the stock tiles of "Portacapture X8".
const STOCK_TRIM: u32 = 0x0101_0101;

/// A copy of a stock tile of "Portacapture X8" (0x819a03b0 and 0x819a03d4), field for field.
const fn stock_tile(pixels: *const u8, x: i16, y: i16, width: u32, height: u32) -> QtTile {
    QtTile { flags: 2, trim: STOCK_TRIM, ..QtTile::alpha8(pixels, x, y, width, height) }
}

static LOWER_TILES: [QtTile; 4] = [
    stock_tile(stock::LOGO_DESCENDER_PIXELS, 116, 18, 6, 7),
    stock_tile(stock::LOGO_PORTACAPTURE_PIXELS, -1, -1, 245, 21),
    QtTile::alpha8(px::BAT.as_ptr(), px::BAT_X as i16, px::BAT_Y as i16, px::BAT_WIDTH, px::BAT_HEIGHT),
    QtTile::alpha8(
        px::EDITION.as_ptr(),
        px::EDITION_X as i16,
        px::EDITION_Y as i16,
        px::EDITION_WIDTH,
        px::EDITION_HEIGHT,
    ),
];

static LOWER_IMAGE: QtImage = QtImage {
    colour: stock::LOGO_COLOUR,
    width: LOWER_WIDTH,
    height: px::LOWER_HEIGHT,
    tile_count: LOWER_TILES.len() as u32,
    tiles: LOWER_TILES.as_ptr(),
};

/// The lower image's handle: a word holding the image's address, as the stock handle at
/// 0x819a040c holds 0x819a03f8.
pub(super) static LOWER_HANDLE: &QtImage = &LOWER_IMAGE;

const _: () = assert!(px::BAT_X + px::BAT_WIDTH <= LOWER_WIDTH && px::EDITION_X + px::EDITION_WIDTH <= LOWER_WIDTH);
const _: () = assert!(px::EDITION_Y + px::EDITION_HEIGHT <= px::LOWER_HEIGHT && px::LOWER_HEIGHT <= 255);
