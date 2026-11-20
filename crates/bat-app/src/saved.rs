//! The settings a host keeps across power-off, packed into a few bytes.
//!
//! Saved: the listening mode with HET's and COMB's tuning, Lowest bat call, Auto volume, the
//! sounds to mark, and the switches of the Record and Phone pages with the phone code. Not saved:
//! Listen to, which depends on the inputs turned on in the host, and the Tests page's switches.

use crate::{AutoVolume, BatApp, Mode, Setting};

/// Bytes of [`BatApp::saved_settings`].
pub const SAVED_BYTES: usize = 16;
/// Changes whenever the layout below does, so bytes from another build are ignored.
const VERSION: u8 = 1;
const HAS_CODE: u8 = 0x80;
const CODE_REQUIRED: u8 = 0x40;

/// The last byte: the sum of the others, inverted, so that 16 zero bytes do not pass.
fn check(bytes: &[u8; SAVED_BYTES]) -> u8 {
    let mut sum = 0u8;
    for byte in &bytes[..SAVED_BYTES - 1] {
        sum = sum.wrapping_add(*byte);
    }
    !sum
}

impl BatApp<'_> {
    /// The settings to keep across power-off, for [`load_settings`](Self::load_settings).
    #[inline(never)]
    pub fn saved_settings(&self) -> [u8; SAVED_BYTES] {
        let mut out = [0; SAVED_BYTES];
        let switches = [
            self.mix_file,
            self.guano,
            self.pause_with_rec,
            self.mute_while_recording,
            self.speaker_switch,
            self.cells_on_weak_usb,
            self.clock_from_remote,
            self.remote.on,
        ];
        out[0] = VERSION;
        for (bit, on) in switches.into_iter().enumerate() {
            out[1] |= (on as u8) << bit;
        }
        out[2] = self.marked & 0x3f;
        out[14] = self.level.volume() as u8;
        if self.remote.code_required {
            out[2] |= CODE_REQUIRED;
        }
        if let Some(code) = self.remote.code {
            out[2] |= HAS_CODE;
            out[8..14].copy_from_slice(&code);
        }
        out[3] = self.min_bat_index as u8;
        out[4] = self.mode as u8;
        let het = (self.het_lo_hz() / 100.0) as u16;
        out[5] = het as u8;
        out[6] = (het >> 8) as u8;
        out[7] = (self.comb_first_hz / 1000.0) as u8;
        out[SAVED_BYTES - 1] = check(&out);
        out
    }

    /// Put back what [`saved_settings`](Self::saved_settings) packed. Returns false, and changes
    /// nothing, for bytes that are not from this version or do not pass their check. Values out
    /// of range are clamped by the setters.
    #[inline(never)]
    pub fn load_settings(&mut self, saved: &[u8; SAVED_BYTES]) -> bool {
        if saved[0] != VERSION || saved[SAVED_BYTES - 1] != check(saved) {
            return false;
        }
        let on = |bit: u8| saved[1] >> bit & 1 != 0;
        self.mix_file = on(0);
        self.guano = on(1);
        self.pause_with_rec = on(2);
        self.mute_while_recording = on(3);
        self.speaker_switch = on(4);
        self.cells_on_weak_usb = on(5);
        self.clock_from_remote = on(6);
        self.remote.on = on(7);
        self.marked = saved[2] & 0x3f;
        self.level.set_volume(AutoVolume::ALL[saved[14].min(2) as usize]);
        if saved[2] & HAS_CODE != 0 && saved[8..14].iter().all(u8::is_ascii_digit) {
            let mut code = [0; 6];
            code.copy_from_slice(&saved[8..14]);
            self.remote.code = Some(code);
            self.remote.code_required = saved[2] & CODE_REQUIRED != 0;
        }
        self.min_bat_index = 0;
        self.change_setting(Setting::LowestBatCall, saved[3] as i32);
        self.set_het_lo((saved[5] as u16 | (saved[6] as u16) << 8) as f32 * 100.0);
        self.set_comb_first(saved[7] as f32 * 1000.0);
        if let Some(&mode) = Mode::ALL.get(saved[4] as usize) {
            if self.modes.contains(&mode) {
                self.set_mode(mode);
            }
        }
        true
    }
}
