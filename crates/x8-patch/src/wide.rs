//! File names as the X8's file functions take them: UTF-16LE, ending in a 0 unit. Only the mode
//! argument ("r", "w+") is an 8-bit C string (docs/HOMEBREW.md, "Reading files from the card").

/// `name` as UTF-16, for a name in ASCII ending in a 0 byte, at compile time: `widen(b"A.CSV\0")`.
pub const fn widen<const N: usize>(name: &[u8; N]) -> [u16; N] {
    assert!(N > 0 && name[N - 1] == 0, "the name must end in a 0 byte");
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        assert!(name[i] < 0x80, "the name must be ASCII");
        out[i] = name[i] as u16;
        i += 1;
    }
    out
}

/// The units of `units` before its first 0 (or all of them), as ASCII in `out`, or None if a unit is
/// outside printable ASCII or `out` is too short.
pub fn narrow<'a>(units: &[u16], out: &'a mut [u8]) -> Option<&'a [u8]> {
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let out = out.get_mut(..len)?;
    for (b, &u) in out.iter_mut().zip(units) {
        if !(0x20..0x7f).contains(&u) {
            return None;
        }
        *b = u as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widens_with_the_ending_zero() {
        assert_eq!(widen(b"AB\0"), [0x41, 0x42, 0]);
    }

    #[test]
    fn narrows_up_to_the_first_zero() {
        let mut out = [0u8; 8];
        let units = [b'2' as u16, b'6' as u16, b'.' as u16, 0, b'x' as u16];
        assert_eq!(narrow(&units, &mut out), Some(&b"26."[..]));
    }

    #[test]
    fn refuses_other_characters_and_short_buffers() {
        let mut out = [0u8; 8];
        assert_eq!(narrow(&[0x41, 0x3042, 0], &mut out), None);
        assert_eq!(narrow(&[0x41; 9], &mut out), None);
    }
}
