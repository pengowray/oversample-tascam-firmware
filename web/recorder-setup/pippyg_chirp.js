// pippyg_chirp.js builds the audio chirp that sends settings to a PippyG bat
// recorder. It is a classic script with no dependencies. It defines the global
// PippygChirp, in a browser and in Node.
//
// This is an independent implementation of the chirp format, not the maker's
// code. The format comes from reading the maker's chirpwav tool, for
// interoperability with the owner's device. The device's software is
// (c) Phil Atkin.
//
// The device notes in the comments below come from static analysis of firmware
// v8.02 and v10 (September 2026). Nobody has tested this encoder on a PippyG yet.
//
// A chirp is 34 bytes, sent as phase shifts on a 12 kHz carrier:
//
//   6 x 0xFF, 0x7F   lead-in. The device tunes to the carrier here.
//   0xA5             start of frame
//   24 payload bytes
//   0x5A             end of frame
//   CRC              CRC-8 of 0xA5, the payload and 0x5A
//
// Payload layout. Multi-byte values are little-endian.
//
//   0-3    latitude, int32, degrees x 2^22
//   4-7    longitude, int32, degrees x 2^22
//   8      device ID. 0 keeps the ID the device already has.
//   9-13   clock time in seconds, see localEpoch
//   14     wake time: hour in bits 0-4, quarter of an hour in bits 5-6,
//          low bit of the recording length code in bit 7
//   15     sleep time: hour in bits 0-4, quarter of an hour in bits 5-6,
//          high bit of the recording length code in bit 7
//   16     flags: bit 0 suppressInterference, bit 1 birdMode, bits 2-3 trigger
//          code, bit 4 always 0, bits 5-6 birdModeSelect, bit 7 always 0
//   17-23  reserved, always 0

(function (root) {
    'use strict';

    // Sample rate of the output in Hz. Neither firmware adjusts its timing after
    // it locks on to the carrier, so the chirp must play at this true rate. Our
    // models of the decoders fail at a rate error of about 300 ppm.
    const SAMPLE_RATE = 48000;

    // One cycle of the 12 kHz carrier at 48 kHz, as 16-bit sample values.
    const CARRIER_CYCLE = [0, 25000, 0, -25000];

    // Each bit lasts 4 carrier cycles. A 1 bit is the carrier as it stands and a
    // 0 bit is the carrier inverted. Bits go out lowest bit first.
    const CYCLES_PER_BIT = 4;

    // Sample values are divided by this to get the floats that Web Audio plays.
    const FLOAT_SCALE = 32768;

    // Number of 0xFF lead-in bytes. Firmware v10 needs at least 2. Our model of
    // firmware v8.02 decodes with none. The chirpwav tool sends 6, and 6 works on
    // both firmware versions.
    const START_BYTE_COUNT = 6;

    // Lead-in byte: all 1 bits, so plain carrier.
    const LEAD_IN_BYTE = 0xFF;

    // Last byte of the lead-in. Its top bit is 0, and that first phase flip tells
    // the device that the frame is about to start.
    const LAST_LEAD_IN_BYTE = 0x7F;

    // Bytes on either side of the payload. The CRC covers both of them.
    const START_OF_FRAME = 0xA5;
    const END_OF_FRAME = 0x5A;

    // Number of payload bytes.
    const PAYLOAD_LENGTH = 24;

    // Generator polynomial of the CRC-8.
    const CRC_POLYNOMIAL = 0x1D;

    // Latitude and longitude are sent as degrees multiplied by this number.
    const DEGREE_SCALE = 4194304;

    // Largest clock time that fits in the 5 bytes of the payload. The firmware uses
    // only the low 4 bytes, so the time it reads wraps around in the year 2106.
    const MAX_EPOCH_SECONDS = 1099511627775;

    // Number of copies of the chirp that chirpSamples plays. The Pipistrelle app
    // has sent three since version 2.1. The device acts on the first copy that it
    // decodes and ignores the rest.
    const DEFAULT_REPEATS = 3;

    // Silence between copies, in samples at 48 kHz: 80 ms. In our model of firmware
    // v10, the decoder can lock on to a run of 0 bytes in a copy that it missed the
    // start of, and then it reads a false frame. A frame lasts 72 ms (216 bits at
    // 1/3 ms per bit), so the false frame ends during the 80 ms of silence and the
    // decoder is ready for the next lead-in, even if echoes in the room keep the
    // false frame going. The gap the Pipistrelle app uses is not known.
    const REPEAT_GAP_SAMPLES = 3840;

    // Latitude and longitude sent when the location is unknown. The device has no
    // value for "location unknown". It writes what it receives into the GUANO
    // metadata of each recording and into the name of a latlong_*.html file on the
    // card. Recordings made with this value therefore show 0.00000 0.00000, which
    // is a point in the sea off West Africa and is easy to spot.
    const UNKNOWN_LOCATION_DEGREES = 0;

    // Trigger sensitivity codes for triggerCode. The device compares the sound in
    // each FFT frame with a threshold, and a higher threshold is less sensitive.
    const TRIGGER = Object.freeze({
        // Threshold 72, the manual's Trigger Level "Low". The chirpwav tool sends this.
        LOW: 0,
        // Threshold 48, the manual's "Normal".
        NORMAL: 1,
        // Threshold 24, the manual's "High".
        HIGH: 2,
        // The manual's "Continuous". The threshold is 0 on firmware v8.02 and in bird
        // mode on firmware v10, which forces a trigger every 16 FFT frames. In bat
        // mode on firmware v10 the threshold is 8, which is very sensitive but does
        // not force triggers, so recording is not continuous.
        CONTINUOUS: 3,
    });

    // Codes for recordingLengthCode, which sets the length of each recording file.
    const RECORDING_LENGTH = Object.freeze({
        // 3,801,088 bytes: 4.9 s in bat mode, 40 s in bird mode.
        MB_3_8: 0,
        // 7,602,176 bytes: 9.9 s in bat mode, 79 s in bird mode.
        MB_8: 1,
        // 11,403,264 bytes: 14.8 s in bat mode, 119 s in bird mode.
        MB_12: 2,
        // 15,335,424 bytes: 20.0 s in bat mode, 160 s in bird mode.
        MB_16: 3,
    });

    // Codes for birdMode.
    const BIRD_MODE = Object.freeze({
        // Bat mode: recordings at 384 kHz. The device ignores birdModeSelect.
        OFF: 0,
        // Bird mode: recordings at 48 kHz. birdModeSelect sets the hours.
        ON: 1,
    });

    // Codes for birdModeSelect, which apply only in bird mode.
    const BIRD_SELECT = Object.freeze({
        // The manual's "Bird": awake for the whole time from wake to sleep.
        WAKE_TO_SLEEP: 0,
        // "Bird + 2": awake for the 2 hours after wake and the 2 hours before
        // sleep, and asleep in between.
        PLUS_2_HOURS: 1,
        // "Bird + 3": the same with 3 hours at each end.
        PLUS_3_HOURS: 2,
        // "Bird + 4": the same with 4 hours at each end.
        PLUS_4_HOURS: 3,
    });

    // Gains for the four samples of a carrier cycle, in quarters of full level. The
    // first and last cycles of a bit are faded. Our models of the firmware also
    // decode the chirp without the fades. They are here so that the output matches
    // the maker's waveform sample for sample.
    const GAINS_FULL = [4, 4, 4, 4];
    const GAINS_FADE_IN = [1, 2, 3, 4];
    const GAINS_FADE_OUT = [4, 3, 2, 1];

    // Returns the setting after checking that it is a whole number in range.
    function wholeNumber(settings, field, min, max) {
        const value = settings[field];
        if (!Number.isInteger(value) || value < min || value > max) {
            throw new RangeError(`${field} must be a whole number from ${min} to ${max}, not ${value}`);
        }
        return value;
    }

    // Returns the latitude or longitude as the int32 that goes in the payload. The
    // arithmetic is single precision, then truncated, because that gives the same
    // bytes as the chirpwav tool. The device also stores these values in single
    // precision, so this loses no accuracy that the device would keep.
    function encodedDegrees(settings, field, limit) {
        const degrees = settings[field];
        if (degrees === null) {
            return UNKNOWN_LOCATION_DEGREES;
        }
        if (typeof degrees !== 'number' || !(Math.abs(degrees) <= limit)) {
            throw new RangeError(`${field} must be null or a number from ${-limit} to ${limit}, not ${degrees}`);
        }
        return Math.trunc(Math.fround(degrees) * DEGREE_SCALE);
    }

    // Returns the 24 payload bytes for a settings object. All the fields below are
    // required. Each one throws a RangeError that names it if it is out of range.
    //
    //   deviceId              0-255. 0 keeps the ID the device already has.
    //   latitude              degrees, -90 to 90, or null if unknown
    //   longitude             degrees, -180 to 180, or null if unknown
    //   epochSeconds          from localEpoch. 0 is refused, because the device
    //                         reads 0 as "set the clock to July 2006".
    //   wakeHour, sleepHour   0-23
    //   wakeQuarter, sleepQuarter
    //                         0-3, in quarters of an hour
    //   recordingLengthCode   see RECORDING_LENGTH
    //   triggerCode           see TRIGGER
    //   birdMode              see BIRD_MODE
    //   birdModeSelect        see BIRD_SELECT
    //   suppressInterference  boolean. Firmware v8.02 stores it and firmware v10
    //                         ignores it. The chirpwav tool sends false.
    //
    // If the wake and sleep times are equal, the device stays awake all the time. A
    // wake time later than the sleep time gives an overnight recording window.
    function payload(settings) {
        if ((settings.latitude === null) !== (settings.longitude === null)) {
            throw new RangeError('latitude and longitude must both be null or both be numbers');
        }
        const latitude = encodedDegrees(settings, 'latitude', 90);
        const longitude = encodedDegrees(settings, 'longitude', 180);
        const deviceId = wholeNumber(settings, 'deviceId', 0, 255);
        const epochSeconds = wholeNumber(settings, 'epochSeconds', 1, MAX_EPOCH_SECONDS);
        const wakeHour = wholeNumber(settings, 'wakeHour', 0, 23);
        const wakeQuarter = wholeNumber(settings, 'wakeQuarter', 0, 3);
        const sleepHour = wholeNumber(settings, 'sleepHour', 0, 23);
        const sleepQuarter = wholeNumber(settings, 'sleepQuarter', 0, 3);
        const lengthCode = wholeNumber(settings, 'recordingLengthCode', 0, 3);
        const triggerCode = wholeNumber(settings, 'triggerCode', 0, 3);
        const birdMode = wholeNumber(settings, 'birdMode', 0, 1);
        const birdModeSelect = wholeNumber(settings, 'birdModeSelect', 0, 3);
        const suppress = settings.suppressInterference;
        if (typeof suppress !== 'boolean') {
            throw new RangeError(`suppressInterference must be true or false, not ${suppress}`);
        }

        const bytes = new Uint8Array(PAYLOAD_LENGTH);
        const view = new DataView(bytes.buffer);
        view.setInt32(0, latitude, true);
        view.setInt32(4, longitude, true);
        bytes[8] = deviceId;

        // Division, because JavaScript's shift operators stop at 32 bits.
        let remaining = epochSeconds;
        for (let i = 9; i < 14; i += 1) {
            bytes[i] = remaining % 256;
            remaining = Math.floor(remaining / 256);
        }

        bytes[14] = wakeHour | (wakeQuarter << 5) | ((lengthCode & 1) << 7);
        bytes[15] = sleepHour | (sleepQuarter << 5) | ((lengthCode >> 1) << 7);
        bytes[16] = (suppress ? 1 : 0) | (birdMode << 1) | (triggerCode << 2) | (birdModeSelect << 5);
        return bytes;
    }

    // Returns the CRC-8 of a list of bytes. This form shifts each message bit in at
    // the bottom of the register and has no final flush of zero bits, so its results
    // differ from those of a common table-driven CRC-8 with the same polynomial.
    function crc8(bytes) {
        let register = 0;
        for (const byte of bytes) {
            for (let mask = 0x80; mask > 0; mask >>= 1) {
                const carry = register & 0x80;
                register = ((register << 1) & 0xFF) | ((byte & mask) ? 1 : 0);
                if (carry) {
                    register ^= CRC_POLYNOMIAL;
                }
            }
        }
        return register;
    }

    // Returns all the bytes sent on air for 24 payload bytes.
    function frame(payloadBytes) {
        if (payloadBytes.length !== PAYLOAD_LENGTH) {
            throw new RangeError(`payload must be ${PAYLOAD_LENGTH} bytes, not ${payloadBytes.length}`);
        }
        const checked = [START_OF_FRAME, ...payloadBytes, END_OF_FRAME];
        const leadIn = new Array(START_BYTE_COUNT).fill(LEAD_IN_BYTE);
        return Uint8Array.from([...leadIn, LAST_LEAD_IN_BYTE, ...checked, crc8(checked)]);
    }

    // Returns the gains for one carrier cycle. The lead-in stays at full level until
    // the last cycle of bit 6 of the 0x7F byte. From bit 7 of that byte to the end of
    // the frame, every bit fades in on its first cycle and out on its last cycle.
    function cycleGains(byteIndex, lastLeadInIndex, bitIndex, cycleIndex) {
        const isLastLeadInByte = byteIndex === lastLeadInIndex;
        const fadesBothEnds = byteIndex > lastLeadInIndex || (isLastLeadInByte && bitIndex === 7);
        const fadesOutOnly = isLastLeadInByte && bitIndex === 6;
        if (fadesBothEnds && cycleIndex === 0) {
            return GAINS_FADE_IN;
        }
        if ((fadesBothEnds || fadesOutOnly) && cycleIndex === CYCLES_PER_BIT - 1) {
            return GAINS_FADE_OUT;
        }
        return GAINS_FULL;
    }

    // Returns the 16-bit samples at 48 kHz for all the bytes of a frame. The frame
    // must begin with its lead-in: any number of 0xFF bytes, then 0x7F.
    function modulate(frameBytes) {
        let leadInLength = 0;
        while (frameBytes[leadInLength] === LEAD_IN_BYTE) {
            leadInLength += 1;
        }
        if (frameBytes[leadInLength] !== LAST_LEAD_IN_BYTE) {
            throw new RangeError('frameBytes must begin with 0xFF bytes followed by 0x7F');
        }

        const samplesPerCycle = CARRIER_CYCLE.length;
        const samples = new Int16Array(frameBytes.length * 8 * CYCLES_PER_BIT * samplesPerCycle);
        let position = 0;
        frameBytes.forEach((byte, byteIndex) => {
            for (let bitIndex = 0; bitIndex < 8; bitIndex += 1) {
                const polarity = (byte >> bitIndex) & 1 ? 1 : -1;
                for (let cycleIndex = 0; cycleIndex < CYCLES_PER_BIT; cycleIndex += 1) {
                    const gains = cycleGains(byteIndex, leadInLength, bitIndex, cycleIndex);
                    for (let i = 0; i < samplesPerCycle; i += 1) {
                        samples[position] = (polarity * CARRIER_CYCLE[i] * gains[i]) / 4;
                        position += 1;
                    }
                }
            }
        });
        return samples;
    }

    // Returns floats at 48 kHz for playback: the chirp for these settings, repeated
    // with REPEAT_GAP_SAMPLES of silence between copies. There is no silence before
    // the first copy or after the last one.
    function chirpSamples(settings, repeats = DEFAULT_REPEATS) {
        if (!Number.isInteger(repeats) || repeats < 1) {
            throw new RangeError(`repeats must be a whole number from 1 up, not ${repeats}`);
        }
        const chirp = modulate(frame(payload(settings)));
        const output = new Float32Array(repeats * chirp.length + (repeats - 1) * REPEAT_GAP_SAMPLES);
        for (let copy = 0; copy < repeats; copy += 1) {
            const start = copy * (chirp.length + REPEAT_GAP_SAMPLES);
            for (let i = 0; i < chirp.length; i += 1) {
                output[start + i] = chirp[i] / FLOAT_SCALE;
            }
        }
        return output;
    }

    // Returns the time to send as epochSeconds: Unix seconds plus the date's offset
    // from UTC, including daylight saving. The device converts this number to a date
    // with no time zone step, so its clock shows the phone's local time.
    function localEpoch(date) {
        return Math.floor(date.getTime() / 1000 - date.getTimezoneOffset() * 60);
    }

    root.PippygChirp = Object.freeze({
        SAMPLE_RATE,
        CYCLES_PER_BIT,
        FLOAT_SCALE,
        START_BYTE_COUNT,
        PAYLOAD_LENGTH,
        CRC_POLYNOMIAL,
        DEGREE_SCALE,
        MAX_EPOCH_SECONDS,
        DEFAULT_REPEATS,
        REPEAT_GAP_SAMPLES,
        UNKNOWN_LOCATION_DEGREES,
        TRIGGER,
        RECORDING_LENGTH,
        BIRD_MODE,
        BIRD_SELECT,
        payload,
        crc8,
        frame,
        modulate,
        chirpSamples,
        localEpoch,
    });
})(globalThis);
