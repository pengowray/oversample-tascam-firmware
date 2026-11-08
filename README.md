# Portacapture X8 bat detector

Work towards using the TASCAM Portacapture X8 as a bat detector and recorder: live
heterodyne, pitch shift, frequency division and time expansion while recording
192 kHz WAV, plus a spectrogram waterfall on the unit's screen. A collaboration
between Oversample (`~/projects/batchi`) and TASCAM.

- [docs/FEASIBILITY.md](docs/FEASIBILITY.md): hardware, frequency range, features, ways to build it, questions for TASCAM.
- [docs/HOMEBREW.md](docs/HOMEBREW.md): firmware update format, flash layout, patching plan, recovery.
- [crates/bat-dsp](crates/bat-dsp/README.md): the real-time bat DSP library (Rust, `no_std`): heterodyne, comb, frequency division, time expansion, pitch shift, spectrum, call detection.
- The AudioMoth chime decoder, for the phone remote and the PHONE TIME screen, is the `chime` crate in the Oversample repository (`~/projects/batchi/chime`), which Oversample also uses. This workspace depends on it by path; change it there.
- [crates/guano](crates/guano/src/lib.rs): GUANO bat-recording metadata for WAV files (`no_std`): reads a WAV header and writes a `guan` chunk. The X8 patch uses it to add GUANO to takes from BAT DETECTOR.
- [crates/bat-app](crates/bat-app/src/lib.rs): the bat app itself (`no_std`): listening modes, call detection and the 320 × 480 screen.
- [web/](web/README.md): an emulator page that runs the bat app in the browser on WAV files or a microphone. Screens of each mode are in [docs/screens/](docs/screens/).
- [tools/x8fw.py](tools/x8fw.py): unpack, check and repack firmware update files (Python 3, standard library only).
- [tools/fwdis.py](tools/fwdis.py): quick disassembly and cross-reference helpers for the main application (needs `capstone` and `numpy`).

`Tascam_X8_official/` holds TASCAM's firmware, manuals and drivers. It is not
committed. Put unpacked images and other working files in `work/`, which is also
not committed.
