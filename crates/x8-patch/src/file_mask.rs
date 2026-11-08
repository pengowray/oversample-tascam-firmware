//! The stock recorder's file mask, a word at `stock::FILE_MASK` with one bit
//! for each file a take writes (docs/HOMEBREW.md, "Turning off the MIX file
//! while recording"). This part has no firmware addresses, so the host tests
//! can run it.

/// Slot 6 is a mono `_MIX` file and slot 19 a stereo one.
const MASTER_SLOTS: u32 = 1 << 6 | 1 << 19;

/// Bits 6 and 7 of MANUAL's track on/off words (settings + 0x200 and + 0x204) are the master
/// track: bit 6 of the first for a mono master, bits 6 and 7 of the second for a stereo one. With
/// none set, REC READY is off for MASTER and takes have no MIX file.
const MASTER_TRACK_BITS: u32 = 0xc0;

/// Whether REC READY is on for MASTER in MANUAL, from the two track on/off words.
pub fn master_rec_ready(words: [u32; 2]) -> bool {
    (words[0] | words[1]) & MASTER_TRACK_BITS != 0
}

/// Whether a file mask includes a MIX file.
pub fn has_master(mask: u32) -> bool {
    mask & MASTER_SLOTS != 0
}

/// The mask without the master file, and its lowest slot, which the stock code
/// keeps in a separate byte: the end of a take looks at that slot's file handle
/// and stops early if it is null, so the byte must name a file that was opened.
/// None when no master file is set, or when the master is the only file: the
/// take would then have no first file, so the stock mask stays.
pub fn without_master(mask: u32) -> Option<(u32, u8)> {
    let rest = mask & !MASTER_SLOTS;
    (rest != mask && rest != 0).then(|| (rest, rest.trailing_zeros() as u8))
}

#[cfg(test)]
mod tests {
    use super::without_master;

    const STEREO_3_4: u32 = 1 << 17;
    const MONO_MIX: u32 = 1 << 6;
    const STEREO_MIX: u32 = 1 << 19;

    #[test]
    fn stereo_tracks_with_a_stereo_mix() {
        assert_eq!(without_master(STEREO_3_4 | STEREO_MIX), Some((STEREO_3_4, 17)));
    }

    /// Slot 6 comes before the stereo track slots, so the stock first slot is the
    /// master here, and it must move to the track file.
    #[test]
    fn stereo_tracks_with_a_mono_mix_moves_the_first_slot() {
        assert_eq!(without_master(STEREO_3_4 | MONO_MIX), Some((STEREO_3_4, 17)));
    }

    #[test]
    fn mono_tracks_keep_their_first_slot() {
        assert_eq!(without_master(1 << 2 | 1 << 3 | MONO_MIX), Some((1 << 2 | 1 << 3, 2)));
    }

    #[test]
    fn a_master_on_its_own_stays() {
        assert_eq!(without_master(MONO_MIX), None);
        assert_eq!(without_master(STEREO_MIX), None);
    }

    /// MANUAL's defaults: 0x7c and 3 with a mono master, 0x3c and 0xc3 with a stereo one.
    #[test]
    fn rec_ready_for_master() {
        assert!(super::master_rec_ready([0x7c, 3]));
        assert!(super::master_rec_ready([0x3c, 0xc3]));
        assert!(!super::master_rec_ready([0x3c, 3]));
        assert!(super::has_master(STEREO_3_4 | MONO_MIX));
        assert!(!super::has_master(STEREO_3_4));
    }

    #[test]
    fn no_master_leaves_nothing_to_do() {
        assert_eq!(without_master(STEREO_3_4), None);
        assert_eq!(without_master(0), None);
    }
}
