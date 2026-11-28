//! The QR code on the settings screen's Phone page, for the phone page's address.
//!
//! Without a phone code the QR code is fixed, so it is stored here as bits rather than made on the
//! X8: it holds `HTTPS://REMOTE.OVERSAMPLE.COM/#x8`. With a phone code it holds
//! `HTTPS://REMOTE.OVERSAMPLE.COM/#x8-493817`, where the digits are the code, and the phone page
//! takes the code from it. That one is stored for the code 000 000, and [`rows`] changes the modules
//! that differ for another code: 3 data bytes and the 10 error correction bytes. Tests make both
//! again with a QR encoder and check that they match.

/// The address as the Phone page shows it.
pub(crate) const ADDRESS: &str = "remote.oversample.com";

/// Modules per side.
pub(crate) const SIZE: usize = 25;

/// The modules, a row each from the top. Bit `SIZE - 1 - x` of a row is the module at `x`; 1 is
/// dark.
pub(crate) const ROWS: [u32; SIZE] = [
    0b1111111010001000001111111,
    0b1000001010101011001000001,
    0b1011101000100101101011101,
    0b1011101010100011101011101,
    0b1011101001100010101011101,
    0b1000001000011011001000001,
    0b1111111010101010101111111,
    0b0000000010110111000000000,
    0b1011011101010100101001011,
    0b1000110011011010011010000,
    0b1111011100101010001011000,
    0b0001110101000101011001101,
    0b1100001100101101100101000,
    0b0001010001110011101101011,
    0b0110001110100110111110001,
    0b1010100010001010111111011,
    0b0010111100010010111111110,
    0b0000000010000101100011011,
    0b1111111010001000101010001,
    0b1000001010101110100011101,
    0b1011101001110111111111111,
    0b1011101010101100011110001,
    0b1011101011110010100110010,
    0b1000001000011001101001001,
    0b1111111010010011001111001,
];

/// The modules with the code 000 000: version 2, error correction level L.
const CODED_ROWS: [u32; SIZE] = [
    0b1111111000101011101111111,
    0b1000001001010100001000001,
    0b1011101010101011101011101,
    0b1011101001010111101011101,
    0b1011101000101010001011101,
    0b1000001001010010101000001,
    0b1111111010101010101111111,
    0b0000000011001000000000000,
    0b1110111110101010111000100,
    0b1101010010010110000110011,
    0b0110001111001010110011111,
    0b0111010000011100101000011,
    0b1110111000001110000110100,
    0b0101000010011100101010011,
    0b1011011101000000110000000,
    0b0100100100100110100011000,
    0b1000101001111010111111001,
    0b0000000011010100100010101,
    0b1111111011101011101011101,
    0b1000001010001111100010101,
    0b1011101010011011111111110,
    0b1011101000110000000010010,
    0b1011101011001010011110101,
    0b1000001010001000011000111,
    0b1111111010110000101100101,
];

/// The first data byte that holds the code's digits. They end 3 bytes on, and 2 more data bytes
/// follow them.
#[cfg(test)]
const CODE_BYTE: usize = 29;
/// The bytes that depend on the code: 3 data bytes, then the error correction bytes.
const CHANGING: usize = 3 + EC_BYTES;
const EC_BYTES: usize = 10;
/// The Reed-Solomon generator for 10 error correction bytes, without its leading 1.
const GENERATOR: [u8; EC_BYTES] = [216, 194, 159, 111, 199, 94, 95, 113, 157, 193];
/// Where each bit of the changing bytes is drawn, first bit first: `y << 5 | x`.
#[rustfmt::skip]
const MODULES: [u16; CHANGING * 8] = [
    267, 236, 235, 172, 171, 140, 139, 108, 107, 76, 75, 44, 43,
    12, 11, 10, 9, 42, 41, 74, 73, 106, 105, 138, 425, 458,
    457, 490, 489, 522, 521, 554, 553, 586, 585, 618, 617, 650, 649,
    682, 681, 714, 713, 746, 745, 778, 777, 520, 519, 488, 487, 456,
    455, 424, 423, 392, 391, 360, 359, 328, 327, 296, 295, 293, 292,
    325, 324, 357, 356, 389, 388, 421, 420, 453, 452, 485, 484, 517,
    516, 515, 514, 483, 482, 451, 450, 419, 418, 387, 386, 355, 354,
    323, 322, 291, 290, 289, 288, 321, 320, 353, 352, 385, 384, 417,
];

/// The modules to draw, a row each from the top: with `code` (6 ASCII digits), the QR code that
/// holds it. Bit `SIZE - 1 - x` of a row is the module at `x`; 1 is dark. Not inlined, so that
/// it stays out of ITCM.
#[inline(never)]
pub(crate) fn rows(code: Option<[u8; 6]>) -> [u32; SIZE] {
    let Some(code) = code else { return ROWS };
    let mut rows = CODED_ROWS;
    // QR's numeric mode: each 3 digits are a 10-bit number. The 20 bits end at a byte's end.
    let mut bits = 0u32;
    for (i, digit) in code.iter().enumerate() {
        // The first 3 digits as a number, moved up 10 bits, then the last 3.
        let place = [100 << 10, 10 << 10, 1 << 10, 100, 10, 1][i];
        bits += (digit - b'0') as u32 * place;
    }
    // Every bit of a QR code is a sum (XOR) of data bits, so the difference from the code
    // 000 000, whose digits are all 0 bits, is this code's bits and their error correction.
    // The data bytes before the digits differ by 0, and so do the 2 after them.
    let mut changing = [0u8; CHANGING + 2];
    changing[0] = (bits >> 16) as u8;
    changing[1] = (bits >> 8) as u8;
    changing[2] = bits as u8;
    // Long division by the generator: each step clears a byte and adds to the 10 after it. The
    // first 3 bytes are then put back.
    for i in 0..5 {
        let factor = changing[i];
        for (byte, &coefficient) in changing[i + 1..].iter_mut().zip(&GENERATOR) {
            *byte ^= multiply(coefficient, factor);
        }
    }
    changing[0] = (bits >> 16) as u8;
    changing[1] = (bits >> 8) as u8;
    changing[2] = bits as u8;
    // The remainder is in the last 10 bytes: move it down over the 2 data bytes after the digits.
    changing.copy_within(5.., 3);
    for (i, &module) in MODULES.iter().enumerate() {
        if changing[i / 8] << (i % 8) & 0x80 != 0 {
            rows[(module >> 5) as usize] ^= 1 << (SIZE - 1 - (module & 31) as usize);
        }
    }
    rows
}

/// The product in QR's field of 256 elements. Not inlined: 10 copies of it are 0.5 KB.
#[inline(never)]
fn multiply(a: u8, b: u8) -> u8 {
    let mut product = 0u8;
    for bit in (0..8).rev() {
        product = product << 1 ^ (product >> 7) * 0x1d;
        product ^= (b >> bit & 1) * a;
    }
    product
}

#[cfg(test)]
mod tests {
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    use qrcodegen::{Mask, QrCode, QrCodeEcc, QrSegment, Version};

    use super::*;

    /// What the code holds, in segments. Capitals, digits and `:/.` use QR's alphanumeric mode,
    /// 5.5 bits a character, so the code fits version 2 (25 x 25 modules): with error correction
    /// level M without the phone code, and level L with it. The scheme and host do not depend on
    /// case. `#x8` opens the page on its Tascam X8 tab.
    const ALPHANUMERIC_PART: &str = "HTTPS://REMOTE.OVERSAMPLE.COM/";
    const BYTE_PART: &str = "#x8";
    /// With the phone code: the 6 digits follow this, in QR's numeric mode.
    const CODED_BYTE_PART: &str = "#x8-";
    /// The mask of the stored modules. The encoder chooses it for the code 000 000; every other
    /// code is drawn with the same one, so that only the digits and the error correction change.
    pub(super) const MASK: u8 = 0;

    fn row_bits(code: &QrCode) -> Vec<u32> {
        (0..SIZE as i32).map(|y| (0..SIZE as i32).fold(0, |row, x| row << 1 | code.get_module(x, y) as u32)).collect()
    }

    fn listing(rows: &[u32]) -> String {
        rows.iter().map(|row| format!("    0b{row:025b},\n")).collect()
    }

    fn coded(digits: &str, mask: Option<Mask>) -> QrCode {
        let segments = [
            QrSegment::make_alphanumeric(ALPHANUMERIC_PART),
            QrSegment::make_bytes(CODED_BYTE_PART.as_bytes()),
            QrSegment::make_numeric(digits),
        ];
        QrCode::encode_segments_advanced(&segments, QrCodeEcc::Low, Version::new(2), Version::new(2), mask, false)
            .expect("fits version 2")
    }

    #[test]
    fn rows_match_the_encoder() {
        let segments = [QrSegment::make_alphanumeric(ALPHANUMERIC_PART), QrSegment::make_bytes(BYTE_PART.as_bytes())];
        let code = QrCode::encode_segments_advanced(
            &segments,
            QrCodeEcc::Medium,
            Version::new(2),
            Version::new(2),
            None,
            true,
        )
        .expect("fits version 2");
        assert_eq!(code.size() as usize, SIZE);
        let rows = row_bits(&code);
        assert_eq!(rows, ROWS, "replace ROWS with:\n{}", listing(&rows));
        assert_eq!(super::rows(None), ROWS);
    }

    #[test]
    fn coded_rows_match_the_encoder() {
        let chosen = coded("000000", None);
        assert_eq!(chosen.mask().value(), MASK, "replace MASK");
        let rows = row_bits(&chosen);
        assert_eq!(rows, CODED_ROWS, "replace CODED_ROWS with:\n{}", listing(&rows));
    }

    /// The modules of a version 2 code that hold no data: the three corner squares with the
    /// format bits round them, the two timing lines and the small square.
    fn fixed(x: usize, y: usize) -> bool {
        let corner = |v: usize| v < 9 || v >= SIZE - 8;
        (corner(x) && corner(y) && !(x >= SIZE - 8 && y >= SIZE - 8))
            || x == 6
            || y == 6
            || ((16..=20).contains(&x) && (16..=20).contains(&y))
    }

    /// The data modules in the order the standard fills them: up and down in pairs of columns
    /// from the right, skipping the timing column.
    fn data_modules() -> Vec<u16> {
        let mut modules = Vec::new();
        let mut right = SIZE as i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for step in 0..SIZE {
                for x in [right as usize, right as usize - 1] {
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { SIZE - 1 - step } else { step };
                    if !fixed(x, y) {
                        modules.push((y << 5 | x) as u16);
                    }
                }
            }
            right -= 2;
        }
        modules
    }

    #[test]
    fn modules_are_where_the_standard_puts_the_changing_bytes() {
        let all = data_modules();
        // 34 data bytes, 10 error correction bytes and 7 spare modules.
        assert_eq!(all.len(), (34 + EC_BYTES) * 8 + 7);
        let wanted: Vec<u16> =
            all[CODE_BYTE * 8..(CODE_BYTE + 3) * 8].iter().chain(&all[34 * 8..(34 + EC_BYTES) * 8]).copied().collect();
        let listing: String =
            wanted.chunks(13).map(|line| format!("    {line:?},\n").replace(['[', ']'], "")).collect();
        assert_eq!(wanted, MODULES, "replace MODULES with:\n{listing}");
    }

    /// The generator is the product of (x - 2^i) for i from 0 to 9, in QR's field.
    #[test]
    fn generator_is_the_standards() {
        let mut generator = [0u8; EC_BYTES];
        generator[EC_BYTES - 1] = 1;
        let mut root = 1u8;
        for _ in 0..EC_BYTES {
            for i in 0..EC_BYTES {
                generator[i] = multiply(generator[i], root);
                if i + 1 < EC_BYTES {
                    generator[i] ^= generator[i + 1];
                }
            }
            root = multiply(root, 2);
        }
        assert_eq!(generator, GENERATOR, "replace GENERATOR with: {generator:?}");
    }

    /// Every code gives the modules the encoder gives for it with the same mask.
    #[test]
    fn every_code_matches_the_encoder() {
        let mask = Some(Mask::new(MASK));
        // 7,919 is prime, so the step visits a spread of codes; the list adds the ends and codes
        // with leading zeros.
        let spread = (0..1_000_000).step_by(7_919);
        for number in spread.chain([1, 9, 10, 999, 1_000, 99_999, 493_817, 999_000, 999_999]) {
            let digits = format!("{number:06}");
            let mut code = [0; 6];
            code.copy_from_slice(digits.as_bytes());
            assert_eq!(super::rows(Some(code)).to_vec(), row_bits(&coded(&digits, mask)), "code {digits}");
        }
    }

    #[test]
    fn address_matches_the_code() {
        let url = format!("{ALPHANUMERIC_PART}{BYTE_PART}").to_lowercase();
        assert_eq!(url, format!("https://{ADDRESS}/#x8"));
        assert_eq!(CODED_BYTE_PART, format!("{BYTE_PART}-"));
    }
}
