# GUANO metadata in X8 bat takes

The bat firmware adds [GUANO](https://github.com/riggsd/guano-spec) metadata to the WAV files of a take
recorded from BAT DETECTOR. This page lists every field it writes. How the chunk gets into the file, and how
each setting was found in the stock firmware, is in [HOMEBREW.md](HOMEBREW.md#guano-metadata-2026-11-08).

Field list as of build B280. B267 to B280 have not yet run on a unit.

## Which files get it

- Takes started from the BAT DETECTOR tile, with SETTINGS, Record tab, `GUANO metadata` on (the default).
- Every WAV file of the take: each track's file, and the MIX file if one is written. Whether the earlier
  files of a split take get it has not been checked on a unit.
- Takes from MANUAL's own tile and from the other stock apps get none. Nor do MP3 takes, overdub or mix down.

## Where it is in the file

A `guan` chunk after the audio, counted in the RIFF size. The text is UTF-8 with `\n` line ends, at most
about 600 bytes. The X8's own header (`bext`, marks, `fmt `) is unchanged, so the X8 and audio apps open the
file as before.

## Standard fields

| Field | Example | Notes |
|---|---|---|
| `GUANO|Version` | `1.0` | Always first |
| `Timestamp` | `2026-11-06T19:07:10` or `2026-11-06T20:09:10+11:00` | The start of the file, to about 1 s: the time the file closed less its length. Local time. It has a UTC offset only once a phone has set the X8's clock, because the X8 does not know its time zone. From B290 the offset is kept over power-off, until the clock is set on the X8 itself. Left out if the clock's date is not a real date |
| `Length` | `58.080000` | Seconds of audio in the file |
| `Samplerate` | `192000` | Hz |
| `Make` | `TASCAM` | |
| `Model` | `Portacapture X8` | |
| `Firmware Version` | `V2.03 with bat patch B280` | The stock firmware's version and the bat firmware's build |
| `Original Filename` | `261106_0090_3-4.wav` | The name the X8 gave the file |
| `Loc Position` | `-36.123456 146.000007` | Latitude and longitude from a phone chime, WGS84, 6 decimal places. Only when the phone sent a location in the hour before the take started |

## `X8Bat|` fields

The bat firmware's own fields, after the standard ones, in this order.

| Field | Example | Written | Notes |
|---|---|---|---|
| `X8Bat|Pauses` | `0` | Always | How many times the take was paused. 0 means the audio has no gaps |
| `X8Bat|Paused` | `34.567` | Always | Seconds paused in all |
| `X8Bat|Input Source` | `3:EXT MIC,4:EXT MIC` | Track files | `INT MIC`, `PLUG IN`, `EXT MIC`, `LINE IN`, `EXT IN` or `USB` |
| `X8Bat|Input Gain` | `3:HIGH,4:HIGH` | Track files | `LOW` or `HIGH` |
| `X8Bat|Input Level` | `3:55,4:55` | Track files | The input's level setting as the X8 shows it: 0 to 70, or 0 to 76 for EXT IN |
| `X8Bat|Auto Level` | `3:OFF,4:OFF` | Track files | `ON` or `OFF` |
| `X8Bat|Phantom` | `3:48V,4:OFF` | Track files, when phantom power is on for an input the file records | `48V`, `24V` or `OFF` for each track |
| `X8Bat|Mic Trim` | `L:-2 dB,R:0 dB` | Track files of the built-in mics, when MIC TRIM is not 0 dB for both | MIC L and MIC R, -3 to 3 dB |
| `X8Bat|MS Decode` | `3-4` | The MIX file, when MS DECODE is on | The pair of inputs decoded: `1-2`, `3-4` or `5-6`. MS DECODE changes only the stereo mix, so track files are not affected and do not have the field |

**The number before each `:` is a track**, and it is the number in the file's name: `_3-4.wav` holds tracks 3
and 4, in that order, as its left and right channels. A track normally records the input with the same number,
but MANUAL's INPUT SELECT can give a track another pair of inputs. `X8Bat|Phantom` reports the input the track
really records.

**`L` and `R` in `X8Bat|Mic Trim` are the two built-in mics**, not the file's channels.

**Values are the settings when the file closed.** A setting changed during the take is reported as it was at
the end.

**The first four input fields are copies** of SOURCE, INPUT GAIN, LEVEL and AUTO_LEVEL in the XRI text that the
stock firmware writes in the file's `bext` chunk (CodingHistory). They are in GUANO so that bat software, which
reads GUANO and not XRI, has them. A file with no XRI text has none of the seven input fields.

**Not written:** low cut, EQ and reverb (the X8 does not offer them at 192 kHz), and the mixer's
level and pan (they change the MIX file only; they are in the take's `.bin` file).

## Example

A stereo take on inputs 3 and 4, with phantom power on input 3 and the phone's location known:

```
GUANO|Version: 1.0
Timestamp: 2026-11-06T20:09:10+11:00
Length: 58.080000
Samplerate: 192000
Make: TASCAM
Model: Portacapture X8
Firmware Version: V2.03 with bat patch B280
Original Filename: 261106_0090_3-4.wav
Loc Position: -36.123456 146.000007
X8Bat|Pauses: 0
X8Bat|Paused: 0.000
X8Bat|Input Source: 3:EXT MIC,4:EXT MIC
X8Bat|Input Gain: 3:HIGH,4:HIGH
X8Bat|Input Level: 3:19,4:19
X8Bat|Auto Level: 3:OFF,4:OFF
X8Bat|Phantom: 3:48V,4:OFF
```

## Reading it

- **BAT PLAYBACK** on the X8: hold a file's row for its file information, which lists the XRI text and the
  GUANO fields. A GUANO field that repeats a value shown under XRI is listed once, under XRI.
- **Oversample** reads GUANO from a file's end as well as its header.
- Any GUANO reader that looks for the chunk after the audio. A reader that only looks before the `data`
  chunk will not find it.

## Code

- [crates/guano](../crates/guano/src/lib.rs): reads a WAV header and writes or reads a `guan` chunk
  (`no_std`).
- [crates/x8-patch/src/take_guano.rs](../crates/x8-patch/src/take_guano.rs): chooses the fields. Its tests
  run on a computer and show each field's exact text.
- [crates/x8-patch/src/patch/guano_file.rs](../crates/x8-patch/src/patch/guano_file.rs): the hook that adds
  the chunk as the X8 closes a file.
