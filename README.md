# Portacapture X8 bat detector

Work towards using the TASCAM Portacapture X8 as a bat detector and recorder: live
heterodyne, pitch shift, frequency division and time expansion while recording
192 kHz WAV, plus a spectrogram waterfall on the unit's screen. A collaboration
between Oversample (`~/projects/batchi`) and TASCAM.

- [docs/FEASIBILITY.md](docs/FEASIBILITY.md): hardware, frequency range, features, ways to build it, questions for TASCAM.
- [docs/HOMEBREW.md](docs/HOMEBREW.md): firmware update format, flash layout, patching plan, recovery.
- [tools/x8fw.py](tools/x8fw.py): unpack, check and repack firmware update files (Python 3, standard library only).
- [tools/fwdis.py](tools/fwdis.py): quick disassembly and cross-reference helpers for the main application (needs `capstone` and `numpy`).

`Tascam_X8_official/` holds TASCAM's firmware, manuals and drivers. It is not
committed. Put unpacked images and other working files in `work/`, which is also
not committed.
