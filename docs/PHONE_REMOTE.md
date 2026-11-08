# Phone remote format

A phone starts and stops recording on an X8 running the bat patch by playing a tone and then an
AudioMoth chime. The chime's deployment ID holds the command. This page describes format version 1,
which builds from B249 read. Builds B239 to B248 read an earlier layout, `BATXREC` followed by a
command byte, which is no longer sent; they ignore version 1 commands.

The X8 acts on these chimes only while Phone remote is on (SETTINGS, Record tab).

## What the phone plays

1. **Wake tone.** An 18 kHz tone for 0.5 s. The phone page uses the AudioMoth library's
   `playTone`, which plays the carrier in 10 ms and 5 ms symbols with the polarity turned over at
   each symbol. The X8's chime decoders are asleep while it records, to save CPU. They wake when the
   18 kHz bin of the bat screen's spectrum is at least 10 dB above the bins 1.5 kHz to either side
   for about 120 ms. Once awake, they read chimes for 6 s, and another tone in that time keeps them
   awake for 6 s more.
2. **Wait.** 0.8 s. The AudioMoth connector then waits up to 1.2 s more, so that the chime starts on
   a whole second.
3. **Chime.** An AudioMoth chime with the time, the UTC offset, optionally the location, and the
   deployment ID below.
4. **Repeat.** The phone page plays the tone and the chime twice, in case the first is not heard.

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

| Byte | Contents |
|---|---|
| 0 to 3 | `BATX` (0x42, 0x41, 0x54, 0x58) |
| 4 | Format version: 1 |
| 5 | Command |
| 6 and 7 | Argument: unsigned 16-bit, low byte first |

For example, Stop recording is sent as `42 41 54 58 01 02 00 00`.

### Commands in version 1

| Code | Command | Argument | What the X8 does |
|---|---|---|---|
| 0 | none | | Never sent. |
| 1 | Start recording | 0 | From stop or standby, presses REC until it records. If it is already recording, it beeps as if it had started. |
| 2 | Stop recording | 0 | Ends the take. If it is already stopped, it beeps as if it had stopped. |

## Rules

- **Commands set a state.** Each command says what state to reach, such as recording, and never
  toggles one. So a chime read twice, or read on two inputs, does no harm. The X8 also treats a
  second reading with the same deployment ID and the same Unix time as the same chime.
- **Adding to the format.** A new command gets a new code and keeps version 1. A command can also
  give a meaning to a new argument value. A new version number means that bytes 5 to 7 have a
  different layout.
- **Unknown messages.** A receiver acts on a message only if it knows the version, the command and
  the argument value. Any other deployment ID that starts with `BATX` is an unknown command: the X8
  plays one low note, shows `Unknown phone command.` and `Update the bat firmware.` on the main screen,
  and does nothing else. So older firmware never acts on a command it does not understand, such as
  a future Start with a duration in its argument.
- **Time and location.** The time, UTC offset and location in a remote chime are used as in any
  other chime: the X8 can set its clock from them (`Set X8 clock from chimes`, Record tab) and puts
  the location in the take's GUANO metadata. An unknown command's chime is still used this way.
- **Other deployment IDs.** A deployment ID that does not start with `BATX` is an ordinary AudioMoth
  deployment ID, and the X8 ignores it. A random AudioMoth deployment ID starts with `BATX` once in
  about 4.3 billion.

## What the X8 sends back

Only sounds in the headphones and a notice on the main screen for 5 s: two rising notes when it
started recording, two falling notes when it stopped, and one low note when it could not do what the
chime asked, with the reason in the notice. The phone does not hear these.

## Limits

- **No address.** Every X8 with Phone remote on that hears the chime acts on it.
- **No authentication.** Anyone who can play the sound near the X8 can start or stop it.
- **Little data.** Each chime carries one command and a 16-bit argument, and lasts up to 3.4 s.

## Code

- The `chime` crate in the Oversample repository: `RemoteMessage`, `RemoteCommand`, `REMOTE_MAGIC`
  and `REMOTE_VERSION` in `chime/src/format.rs`.
- The X8: [crates/bat-app/src/remote.rs](../crates/bat-app/src/remote.rs) reads the tone and the
  chime, and [crates/x8-patch/src/patch/remote.rs](../crates/x8-patch/src/patch/remote.rs) presses
  the buttons.
- The phone page: `remoteDeploymentId` in `setup.js`, in the batchi-chime repository (not yet
  published).
