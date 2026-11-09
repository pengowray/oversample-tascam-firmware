//! The QR code on the settings screen's Phone page, for the phone page's address.
//!
//! The code is fixed, so it is stored here as bits rather than made on the X8. A test makes it
//! again with a QR encoder and checks that it matches: it holds `HTTPS://REMOTE.OVERSAMPLE.COM/#x8`.

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

/// Whether the module at `x`, `y` is dark.
pub(crate) const fn dark(x: usize, y: usize) -> bool {
    ROWS[y] >> (SIZE - 1 - x) & 1 == 1
}

#[cfg(test)]
mod tests {
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};

    use super::*;

    /// What the code holds, in two segments. Capitals, digits and `:/.` use QR's alphanumeric mode,
    /// 5.5 bits a character, so the code fits version 2 (25 x 25 modules) with error correction
    /// level M. The scheme and host do not depend on case. `#x8` opens the page on its Tascam X8
    /// tab.
    const ALPHANUMERIC_PART: &str = "HTTPS://REMOTE.OVERSAMPLE.COM/";
    const BYTE_PART: &str = "#x8";

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
        let rows: Vec<u32> = (0..SIZE as i32)
            .map(|y| (0..SIZE as i32).fold(0, |row, x| row << 1 | code.get_module(x, y) as u32))
            .collect();
        let listing: String = rows.iter().map(|row| format!("    0b{row:025b},\n")).collect();
        assert_eq!(rows, ROWS, "replace ROWS with:\n{listing}");
    }

    #[test]
    fn address_matches_the_code() {
        let url = format!("{ALPHANUMERIC_PART}{BYTE_PART}").to_lowercase();
        assert_eq!(url, format!("https://{ADDRESS}/#x8"));
    }
}
