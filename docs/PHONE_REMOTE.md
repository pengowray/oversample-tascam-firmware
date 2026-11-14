# Phone remote format

A phone sets the clock, starts and stops recording, and changes the listening mode on an X8 running
the bat patch by playing a tone and then an AudioMoth chime. The chime's deployment ID holds the
command. The phone page is at [remote.oversample.com](https://remote.oversample.com/#x8) (source:
[batchi-chime](https://github.com/pengowray/batchi-chime)). This page describes format versions 1
and 2. Builds from B249 read version 1: start and stop are in every version 1 build; set clock and
listening mode are in B257 and later. Builds from B273 also read version 2, which adds a check made
from a 6-digit code that the X8 shows. With their `Phone code` switch on, they act only on
version 2 commands with the right check (see [Phone code](#phone-code)); the switch is off by
default. Builds B239 to B248 read an earlier layout, `BATXREC`
followed by a command byte, which is no longer sent; they ignore version 1 and version 2 commands.

The X8 acts on these chimes on its main screen only while Phone remote is on (SETTINGS, Phone tab;
the Record tab before B257). Its PHONE TIME screen reads every chime and acts on a set clock
command in either version, whether Phone remote is on or not, and never looks at the check.

## What the phone plays

1. **Lead-in.** 1 s of quiet white noise, about 27 dB below the tone, with 50 ms fades. It wakes
   Bluetooth speakers and headphones, which can cut off the start of a sound after silence. It has
   no peak at 18 kHz, so it does not wake the X8's decoders. Pages before 2026-11-11 did not play it.
2. **Wake tone.** An 18 kHz tone for 0.5 s. The phone page uses the AudioMoth library's
   `playTone`, which plays the carrier in 10 ms and 5 ms symbols with the polarity turned over at
   each symbol. The X8's chime decoders are asleep while it records, to save CPU. They wake when the
   18 kHz bin of the bat screen's spectrum is at least 10 dB above the bins 1.5 kHz to either side
   for about 120 ms. Once awake, they read chimes for 6 s, and another tone in that time keeps them
   awake for 6 s more.
3. **Wait.** 0.8 s. The AudioMoth connector then waits up to 1.2 s more, so that the chime starts on
   a whole second.
4. **Chime.** An AudioMoth chime with the time, the UTC offset, optionally the location, and the
   deployment ID below.
5. **Once.** The phone page plays each command once. Pages before 2026-11-10 played the tone and
   the chime twice; a command read twice does no harm (see Rules), so a press that was not heard
   can simply be repeated.

A chime with no tone before it is read only on the PHONE TIME screen (SETTINGS, Tests tab,
`Phone time`), where the decoders run all the time.

## Chime

The chime is AudioMoth's format, unchanged, as `audiomothchime.js` from Open Acoustic Devices sends
it: an 18 kHz carrier, 16 start symbols, data symbols of 5 ms for a 0 and 10 ms for a 1, and 8 stop
symbols. Each byte is sent as two 7-bit Hamming codewords. A CRC-16/XMODEM of the payload follows
it, low byte first. The `chime` crate in the [Oversample](https://github.com/pengowray/oversample)
repository decodes it (`chime/src/format.rs`).

## Payload

| Offset | Bytes | Field |
|---|---|---|
| 0 | 4 | Unix time in seconds at the start of the chime, unsigned, little-endian |
| 4 | 2 | The phone's UTC offset in minutes, signed, little-endian, from -720 to 840 |
| 6 | 7 | Location, if sent: latitude in 28 bits (units of 1/1,000,000 degree), then longitude in 28 bits (units of 1/500,000 degree), both two's complement, least significant bit first |
| 6 or 13 | 8 | Deployment ID, if sent |

The payload's length says which optional fields it has: 6 bytes (time only), 13 (with location),
14 (with deployment ID) or 21 (with both, location first). 21 bytes is the longest payload that
AudioMoth's format allows.

## Deployment ID

The bytes are listed in the order the chime sends them, which is the order the decoder gives them.
AudioMoth's connector (`encodeDeploymentID` in `audiomothchime_connector.js`) sends the bytes of the
ID it is given in reverse order, so a sender that uses the connector passes the bytes reversed.

Version 1:

| Byte | Contents |
|---|---|
| 0 to 3 | `BATX` (0x42, 0x41, 0x54, 0x58) |
| 4 | Format version: 1 |
| 5 | Command |
| 6 and 7 | Argument: unsigned 16-bit, low byte first |

For example, Stop recording is sent as `42 41 54 58 01 02 00 00`.

Version 2, from B273:

| Byte | Contents |
|---|---|
| 0 to 3 | `BATX` |
| 4 | Format version: 2 |
| 5 | Command in the high 4 bits, argument in the low 4 bits |
| 6 and 7 | Check: bits 0 to 7 of the hash in byte 6, bits 8 to 15 in byte 7 |

The commands and their arguments are the same in both versions.

### The check in version 2

The check is the low 16 bits of a SipHash-2-4 hash (64-bit result) of a 12-byte message, with a
16-byte key.

- **Key.** The code's 6 ASCII digits, then 10 zero bytes. For the code 493 817 the key is
  `34 39 33 38 31 37 00 00 00 00 00 00 00 00 00 00`.
- **Message.** `BATX`, 0x02, byte 5 of the deployment ID, the chime's Unix time (4 bytes,
  little-endian), then the chime's UTC offset in minutes (2 bytes, signed, little-endian). The time
  and the offset are the values in the chime's payload.

| Code | Unix time | UTC offset | Command | Hash | Deployment ID |
|---|---|---|---|---|---|
| 493 817 | 1794700800 | 600 | Start recording | 0xc2a8f4562886f3ba | `42 41 54 58 02 10 ba f3` |
| 493 817 | 1794700801 | -300 | Listening mode 1:1 | 0x3f47b56145d0adb0 | `42 41 54 58 02 46 b0 ad` |
| 000 000 | 0 | 0 | Set clock | 0x82c37daff2bd0a6b | `42 41 54 58 02 30 6b 0a` |

The `chime` crate's tests and the phone page's tests use these three, and the `chime` crate's also
check its SipHash against the published vector (key `00 01 .. 0f`, message `00 01 .. 0e`, hash
0xa129ca6149be45e5).

### Commands

| Code | Command | Argument | What the X8 does |
|---|---|---|---|
| 0 | none | | Never sent. |
| 1 | Start recording | 0: leave `Mute while recording` as it is. From B273, 1: turn it on, 2: turn it off | From stop or standby, presses REC until it records. If it is already recording, it beeps as if it had started. With argument 1 or 2 it first sets the X8's `Mute while recording` switch (SETTINGS, Record tab), also when it is already recording; BAT PLAYBACK, which has no such switch, starts and ignores the argument. Builds B249 to B272 answer argument 1 or 2 with the unknown notice and do not start. |
| 2 | Stop recording | 0 | Ends the take. If it is already stopped, it beeps as if it had stopped. |
| 3 | Set clock | 0 | Sets the clock to the chime's time, whatever the `Set X8 clock from chimes` setting. During a take, it sets the clock when the take ends. From B257. |
| 4 | Listening mode | The mode: 1 HET, 2 COMB, 3 FD, 4 TE, 5 PITCH, 6 1:1 | Plays that mode in the headphones, as if its mode button had been pressed. A mode that the build hides, such as PITCH on the X8, gets one low note and changes nothing. From B257. |

## Phone code

From B273 the SETTINGS, Phone tab has a `Phone code` switch, off by default. It is a guard against
other people's phones, for example where several people use X8s near each other. The first time
the switch is turned on after power-on, the X8 makes a 6-digit code, and keeps it until power-off.
While the switch is on, the code is shown beside it in two groups, such as `493 817`. The person
types it into the phone page, which then sends version 2 commands. The X8 makes the code from a
hash of the input samples heard before the switch was turned on and the time of the press, because
it has no random number generator.

While `Phone code` is on, on the main screen:

- **A version 2 command with the right check** is acted on, if its time is later than the time of
  the last version 2 command acted on.
- **A version 1 command** is refused: one low note, and the notice `Phone page needs the X8's code.`
  `Type it, then send again:` and the code on a third line.
- **A version 2 command with another check** is refused: one low note, and the notice
  `Phone page has a different code.` `Type this code, then send again:` and the code on a third
  line.
- **A version 2 command with the right check and a time no later than the last one acted on** is
  refused, because it may be a recording played again: one low note, and the notice
  `Phone command has an old time.` `Send again. Check phone's clock.` A chime with the deployment ID and the
  time of the chime read just before it is the same chime on the other input, and is ignored
  with no note.
- **Any chime that is not acted on gives no time and no location.** That covers a refused command,
  a chime with no deployment ID, another recorder's deployment ID, and an unknown message whose
  check is not right. The clock is not set from it, also with `Set clock from all chimes` on, and
  its location is not put in a take's metadata.
- **An unknown message** (below) gets the unknown notice, whatever its check.

While `Phone code` is off (the default), version 1 commands work as before B273, version 2 commands are acted on
whatever their check and time, and every chime's time and location are used as before.

## Rules

- **Commands set a state.** Each command says what state to reach, such as recording, and never
  toggles one. So a chime read twice, or read on two inputs, does no harm. The X8 also treats a
  second reading with the same deployment ID and the same Unix time as the same chime.
- **Adding to the format.** A new command gets a new code and keeps the version numbers. A command
  can also give a meaning to a new argument value. Version 2 has room for commands 1 to 15 and
  arguments 0 to 15. A new version number means that bytes 5 to 7 have a different layout.
- **Unknown messages.** A receiver acts on a message only if it knows the version, the command and
  the argument value. Any other deployment ID that starts with `BATX` is an unknown command: the X8
  plays one low note, shows `Unknown phone command.` and `Update the bat firmware.` on the main screen,
  and does nothing else. So older firmware never acts on a command it does not understand, such as
  a future Start with a duration in its argument.
- **Time and location.** The time, UTC offset and location in a remote chime are used as in any
  other chime: the X8 can set its clock from them (`Set clock from all chimes`, Phone tab, for
  commands other than set clock) and puts the location in the take's GUANO metadata. An unknown
  command's chime is still used this way, so a B249 to B256 build that hears a set clock command
  beeps "unknown" but still sets its clock if that setting is on. From B273, while `Phone code` is
  on, only a chime with the right check is used this way (see [Phone code](#phone-code)).
- **Other deployment IDs.** A deployment ID that does not start with `BATX` is an ordinary AudioMoth
  deployment ID, and the X8 ignores it. A random AudioMoth deployment ID starts with `BATX` once in
  about 4.3 billion.

## What the X8 sends back

Only sounds in the headphones and a notice on the main screen for 5 s: two rising notes when it
started recording, two falling notes when it stopped, one high note when it set its clock (or will
set it after the take) or changed the listening mode, and one low note when it could not do what
the chime asked or refused it over the phone code, with the reason in the notice. The phone does
not hear these.

## Limits

- **No address.** Every X8 with Phone remote on that hears the chime acts on it, if `Phone code` is
  off or the chime's check was made from that X8's code.
- **The check is 16 bits.** A command sent with a guessed check is right 1 time in 65,536.
- **The code can be worked out from a recording.** There are 1,000,000 codes. Someone who records
  a few of the phone's commands can try every code against them on a computer and find the one
  that gives all their checks. The code keeps out other people's phones and chance commands. It
  does not keep out someone who records the commands and means to get in.
- **The code changes at each power-on.** The X8 stores no settings, so the code has to be typed
  into the phone page again after the X8 has been off, and `Phone code` turned on again.
- **The location is not covered by the check.** The check covers the command, the time and the UTC
  offset. A chime that repeats those and the check with another location is accepted.
- **A recording works once more if the X8 missed the command.** The X8 refuses a time it has
  already acted on, and earlier times. It accepts a recording of a command that it never heard,
  until it acts on a later one.
- **With no input, the code is easier to guess.** The code comes from the inputs' noise. With every
  input off or silent, it depends only on when the `Phone code` switch was turned on.
- **PHONE TIME reads everything.** That screen sets the clock from a set clock command with any
  check, or none.
- **`Phone code` off, the default, means no authentication.** Anyone who can play the sound near
  the X8 can start or stop it.
- **Little data.** Version 1 carries one command and a 16-bit argument, version 2 one command and a
  4-bit argument. A chime lasts up to 3.4 s.
- **No answer.** The page cannot tell whether the X8 heard a command. The person listens for the
  beeps.

## Code

- The `chime` crate in the Oversample repository: `RemoteMessage`, `RemoteCommand`, `REMOTE_MAGIC`,
  `REMOTE_VERSION`, `REMOTE_VERSION_CODED` and `remote_check` in `chime/src/format.rs`.
- The X8: [crates/bat-app/src/remote.rs](../crates/bat-app/src/remote.rs) reads the tone and the
  chime, and [crates/x8-patch/src/patch/remote.rs](../crates/x8-patch/src/patch/remote.rs) presses
  the buttons.
- The phone page: `remoteDeploymentId` in `setup.js`, in the
  [batchi-chime](https://github.com/pengowray/batchi-chime) repository, published at
  [remote.oversample.com](https://remote.oversample.com/#x8). The X8's SETTINGS, Phone tab shows a
  QR code for it.
