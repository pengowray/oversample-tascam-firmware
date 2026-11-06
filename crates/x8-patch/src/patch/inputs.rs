use core::ptr;

use bat_app::InputName;

use super::{read_byte, stock};

/// An input the app can hear: its word in an input frame and its name.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Input {
    pub(super) word: u32,
    pub(super) name: InputName,
}

/// Names for inputs 1 to 6, EXT IN and the X8's playback.
const INPUT_NAMES: [InputName; 6] = [
    InputName { short: "1", long: "Input 1" },
    InputName { short: "2", long: "Input 2" },
    InputName { short: "3", long: "Input 3" },
    InputName { short: "4", long: "Input 4" },
    InputName { short: "5", long: "Input 5" },
    InputName { short: "6", long: "Input 6" },
];
const EXT_IN_NAMES: [InputName; 2] =
    [InputName { short: "EL", long: "EXT IN left" }, InputName { short: "ER", long: "EXT IN right" }];
pub(super) const PLAYBACK_LEFT: InputName = InputName { short: "L", long: "Left" };
pub(super) const PLAYBACK_RIGHT: InputName = InputName { short: "R", long: "Right" };

/// The input a track hears, from the pair chosen for it in INPUT SELECT, or
/// `None` for a source this firmware does not name.
fn track_input(track: usize) -> Option<Input> {
    let side = track % 2;
    // SAFETY: a byte in the settings record, inside the track's record.
    let source = read_byte(unsafe { stock::TRACK_SOURCES.add(track * stock::TRACK_SOURCE_STEP) });
    let (input, name) = match source {
        1 | 2 | 3 => {
            let input = (source as usize - 1) * 2 + side;
            (input, INPUT_NAMES[input])
        }
        4 => (4 + side, EXT_IN_NAMES[side]),
        _ => return None,
    };
    let swapped = input < 2 && read_byte(stock::SWAP_INPUTS_1_2) == 1;
    Some(Input { word: stock::INPUT_WORDS[if swapped { 1 - input } else { input }], name })
}

/// Whether the firmware has muted the input of every track turned on, so
/// MANUAL's mix is silent. False with none on, which has its own notice.
pub(super) fn on_tracks_muted() -> bool {
    let on = tracks_on();
    let mut any = false;
    for track in (0..stock::TRACKS).filter(|track| on >> track & 1 != 0) {
        // SAFETY: a byte in the settings record, inside the track's record.
        let source = read_byte(unsafe { stock::TRACK_SOURCES.add(track * stock::TRACK_SOURCE_STEP) });
        let channel = match source {
            1 | 2 | 3 => (source as usize - 1) * 2 + track % 2,
            4 => 4 + track % 2,
            _ => continue,
        };
        // SAFETY: the word of one of six channels, in the stock DSP state.
        let muted = unsafe { ptr::read_volatile(stock::CHANNEL_MUTED.byte_add(channel * stock::CHANNEL_MUTED_STEP)) };
        if muted != 1 {
            return false;
        }
        any = true;
    }
    any
}

/// Tracks turned on in the app, as a bit for each of tracks 1 to 6.
pub(super) fn tracks_on() -> u32 {
    // SAFETY: words in the settings record.
    let [a, b] = stock::TRACKS_ON.map(|at| unsafe { ptr::read_volatile(at) });
    (a | b) & ((1 << stock::TRACKS) - 1)
}

/// The inputs the app hears: the first two different inputs among the tracks
/// turned on in MANUAL, taking inputs 3 to 6 and EXT IN before inputs 1 and 2
/// (the included mics' jacks). All six tracks are on by default, so with the
/// default assignment that means inputs 3 and 4. With none on, the X8 powers
/// the inputs down.
pub(super) fn selected_inputs() -> [Option<Input>; 2] {
    let on = tracks_on();
    let mut picked: [Option<Input>; 2] = [None; 2];
    for wanted_jacks_1_2 in [false, true] {
        for track in 0..stock::TRACKS {
            let Some(input) = track_input(track).filter(|_| on >> track & 1 != 0) else { continue };
            let jacks_1_2 = input.word == stock::INPUT_WORDS[0] || input.word == stock::INPUT_WORDS[1];
            if jacks_1_2 != wanted_jacks_1_2 || picked.contains(&Some(input)) {
                continue;
            }
            if let Some(free) = picked.iter_mut().find(|slot| slot.is_none()) {
                *free = Some(input);
            }
        }
    }
    picked
}
