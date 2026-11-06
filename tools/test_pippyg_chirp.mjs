#!/usr/bin/env node
// Tests for web/recorder-setup/pippyg_chirp.js. Needs only Node:
//
//   node tools/test_pippyg_chirp.mjs
//
// The expected bytes below are facts about the chirp format. They were recovered
// from the output of the maker's chirp tool by demodulating it as data, or worked out
// by hand from the format. The test does not read any of the maker's files.

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const modulePath = new URL('../web/recorder-setup/pippyg_chirp.js', import.meta.url);
vm.runInThisContext(readFileSync(modulePath, 'utf8'), { filename: modulePath.pathname });
const P = globalThis.PippygChirp;

function fromHex(text) {
    return Uint8Array.from(text.replace(/\s+/g, '').match(/../g), (pair) => parseInt(pair, 16));
}

function toHex(bytes) {
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

// The settings held in the output of the maker's chirp tool, which is one test vector.
// The latitude and longitude are the values in the tool's header file.
const TOOL_SETTINGS = Object.freeze({
    deviceId: 31,
    latitude: 51.61926,
    longitude: -1.9883,
    epochSeconds: 1740220054,
    wakeHour: 13,
    wakeQuarter: 2,
    sleepHour: 7,
    sleepQuarter: 1,
    recordingLengthCode: 0,
    triggerCode: 0,
    birdMode: 0,
    birdModeSelect: 0,
    suppressInterference: false,
});

const TOOL_PAYLOAD = 'f0a1e70c b2bf80ff 1f 96a6b96700 4d 27 00 00000000000000';
const TOOL_FRAME = `ffffffffffff 7f a5 ${TOOL_PAYLOAD} 5a 28`;

// SHA-256 of the 4352 samples of that vector as little-endian 16-bit values, measured
// on the maker's chirp.wav. The digest checks the fades without any samples in this file.
const TOOL_SAMPLES_SHA256 = 'f516018090ebaa2b2b7f7d315e41bb0bce1a5eea8ae948ffc0d56d7a3d54bfaf';

const SIGNAL_SAMPLES_PER_BYTE = 128;

// A CRC written separately from the module, over a list of message bits.
function crcOfBits(bytes) {
    const bits = [];
    for (const byte of bytes) {
        for (let i = 7; i >= 0; i -= 1) {
            bits.push((byte >> i) & 1);
        }
    }
    let register = 0;
    for (const bit of bits) {
        const overflow = register >= 0x80;
        register = ((register * 2) % 256) + bit;
        if (overflow) {
            register ^= 0x1d;
        }
    }
    return register;
}

// The correlator used for the round trip tests. It finds where the signal begins,
// then takes the dot product of each 16-sample bit with the carrier. Returns the bytes
// and the index of the sample after them.
function demodulateFrom(samples, searchFrom, frameLength) {
    let first = searchFrom;
    while (first < samples.length && samples[first] === 0) {
        first += 1;
    }
    if (first === samples.length) {
        return null;
    }
    // The carrier starts with a zero sample, so the first non-zero sample is the second.
    const start = first - 1;
    const pattern = [0, 1, 0, -1];
    const bytes = new Uint8Array(frameLength);
    for (let byteIndex = 0; byteIndex < frameLength; byteIndex += 1) {
        for (let bit = 0; bit < 8; bit += 1) {
            const bitStart = start + (byteIndex * 8 + bit) * 16;
            let dot = 0;
            for (let i = 0; i < 16; i += 1) {
                dot += samples[bitStart + i] * pattern[i % 4];
            }
            if (dot > 0) {
                bytes[byteIndex] |= 1 << bit;
            }
        }
    }
    return { bytes, next: start + frameLength * SIGNAL_SAMPLES_PER_BYTE };
}

// Reads the settings back out of a frame, without using the module.
function parseFrame(frameBytes) {
    assert.equal(toHex(frameBytes.subarray(0, 8)), 'ffffffffffff7fa5');
    assert.equal(frameBytes[32], 0x5a);
    assert.equal(frameBytes[33], crcOfBits(frameBytes.subarray(7, 33)), 'CRC byte');
    const p = frameBytes.subarray(8, 32);
    const view = new DataView(p.buffer, p.byteOffset, 24);
    return {
        latitudeRaw: view.getInt32(0, true),
        longitudeRaw: view.getInt32(4, true),
        deviceId: p[8],
        epochSeconds: p[9] + p[10] * 2 ** 8 + p[11] * 2 ** 16 + p[12] * 2 ** 24 + p[13] * 2 ** 32,
        wakeHour: p[14] & 0x1f,
        wakeQuarter: (p[14] >> 5) & 3,
        sleepHour: p[15] & 0x1f,
        sleepQuarter: (p[15] >> 5) & 3,
        recordingLengthCode: (p[14] >> 7) + (p[15] >> 7) * 2,
        suppressInterference: (p[16] & 1) === 1,
        birdMode: (p[16] >> 1) & 1,
        triggerCode: (p[16] >> 2) & 3,
        birdModeSelect: (p[16] >> 5) & 3,
    };
}

test('CRC of short messages, worked out by hand', () => {
    assert.equal(P.crc8([]), 0);
    assert.equal(P.crc8([0x00]), 0);
    assert.equal(P.crc8([0x01]), 0x01);
    assert.equal(P.crc8([0x80]), 0x80);
    // The single 1 bit reaches the top of the register after 7 more shifts, and the
    // eighth shift carries it out, which leaves the polynomial.
    assert.equal(P.crc8([0x01, 0x00]), 0x1d);
});

test('CRC agrees with a second implementation and with the tool output', () => {
    const body = fromHex(`a5 ${TOOL_PAYLOAD} 5a`);
    assert.equal(P.crc8(body), 0x28);
    assert.equal(crcOfBits(body), 0x28);
    const messages = [[1, 2, 3], [0xff, 0xff, 0xff, 0xff], Array.from({ length: 26 }, (_, i) => (i * 37) & 0xff)];
    for (const message of messages) {
        assert.equal(P.crc8(message), crcOfBits(message));
    }
});

test('CRC changes when any single bit of the framed message changes', () => {
    const body = fromHex(`a5 ${TOOL_PAYLOAD} 5a`);
    for (let byteIndex = 0; byteIndex < body.length; byteIndex += 1) {
        for (let bit = 0; bit < 8; bit += 1) {
            const changed = body.slice();
            changed[byteIndex] ^= 1 << bit;
            assert.notEqual(P.crc8(changed), 0x28, `byte ${byteIndex} bit ${bit}`);
        }
    }
});

test('payload and frame reproduce the tool output', () => {
    const payload = P.payload(TOOL_SETTINGS);
    assert.equal(toHex(payload), toHex(fromHex(TOOL_PAYLOAD)));
    assert.equal(payload.length, P.PAYLOAD_LENGTH);
    assert.equal(toHex(P.frame(payload)), toHex(fromHex(TOOL_FRAME)));
});

test('the same payload with an epoch 45 s later matches the payl.oad file of the tool', () => {
    const payload = P.payload({ ...TOOL_SETTINGS, epochSeconds: TOOL_SETTINGS.epochSeconds + 45 });
    assert.equal(toHex(payload), 'f0a1e70cb2bf80ff1fc3a6b967004d270000000000000000');
    // The tool computes its CRC before it adds the 45 s, so the file still holds 0x28.
    // A correct CRC for these bytes is 0x0e, and that is what frame() gives.
    assert.equal(P.frame(payload)[33], 0x0e);
});

test('modulate matches the digest of the tool samples', () => {
    const samples = P.modulate(P.frame(P.payload(TOOL_SETTINGS)));
    assert.equal(samples.length, 34 * SIGNAL_SAMPLES_PER_BYTE);
    const digest = createHash('sha256').update(Buffer.from(samples.buffer)).digest('hex');
    assert.equal(digest, TOOL_SAMPLES_SHA256);
});

test('latitude and longitude that are exact in binary', () => {
    const cases = [
        [-12.5, -0.25, '0000e0fc 0000f0ff'],
        [90, 180, '00008016 0000002d'],
        [-90, -180, '000080e9 000000d3'],
        [1, -1, '00004000 0000c0ff'],
    ];
    for (const [latitude, longitude, expected] of cases) {
        const payload = P.payload({ ...TOOL_SETTINGS, latitude, longitude });
        assert.equal(toHex(payload.subarray(0, 8)), toHex(fromHex(expected)), `${latitude}, ${longitude}`);
    }
});

test('latitude and longitude truncate towards zero', () => {
    // Multiples of 2^-23 land exactly half way between two values of the int32 field.
    const half = 2 ** -23;
    const payload = P.payload({ ...TOOL_SETTINGS, latitude: -half, longitude: 3 * half });
    assert.equal(toHex(payload.subarray(0, 8)), '0000000001000000');
});

test('unknown location sends zeros', () => {
    const payload = P.payload({ ...TOOL_SETTINGS, latitude: null, longitude: null });
    assert.equal(toHex(payload.subarray(0, 8)), '0000000000000000');
    assert.equal(P.UNKNOWN_LOCATION_DEGREES, 0);
});

test('device ID goes in byte 8', () => {
    for (const deviceId of [0, 1, 255]) {
        assert.equal(P.payload({ ...TOOL_SETTINGS, deviceId })[8], deviceId);
    }
});

test('epoch is 5 little-endian bytes, including values above 32 bits', () => {
    const cases = [
        [1, '0100000000'],
        [0x7fffffff, 'ffffff7f00'],
        [2147483648, '0000008000'],
        [0xffffffff, 'ffffffff00'],
        [0x0102030405, '0504030201'],
        [P.MAX_EPOCH_SECONDS, 'ffffffffff'],
    ];
    for (const [epochSeconds, expected] of cases) {
        const payload = P.payload({ ...TOOL_SETTINGS, epochSeconds });
        assert.equal(toHex(payload.subarray(9, 14)), expected, String(epochSeconds));
    }
});

test('wake and sleep bytes hold the hour, the quarter and the recording length bits', () => {
    const times = {
        wakeHour: 23, wakeQuarter: 3, sleepHour: 0, sleepQuarter: 0,
    };
    const expected = { 0: [0x77, 0x00], 1: [0xf7, 0x00], 2: [0x77, 0x80], 3: [0xf7, 0x80] };
    for (const [code, bytes] of Object.entries(expected)) {
        const payload = P.payload({ ...TOOL_SETTINGS, ...times, recordingLengthCode: Number(code) });
        assert.deepEqual([payload[14], payload[15]], bytes, `length code ${code}`);
    }
    const reversed = P.payload({ ...TOOL_SETTINGS, wakeHour: 0, wakeQuarter: 0, sleepHour: 23, sleepQuarter: 3 });
    assert.deepEqual([reversed[14], reversed[15]], [0x00, 0x77]);
});

test('flag bits', () => {
    const flags = (change) => P.payload({ ...TOOL_SETTINGS, ...change })[16];
    assert.equal(flags({}), 0x00);
    assert.equal(flags({ suppressInterference: true }), 0x01);
    assert.equal(flags({ birdMode: P.BIRD_MODE.ON }), 0x02);
    assert.equal(flags({ triggerCode: P.TRIGGER.LOW }), 0x00);
    assert.equal(flags({ triggerCode: P.TRIGGER.NORMAL }), 0x04);
    assert.equal(flags({ triggerCode: P.TRIGGER.HIGH }), 0x08);
    assert.equal(flags({ triggerCode: P.TRIGGER.CONTINUOUS }), 0x0c);
    assert.equal(flags({ birdModeSelect: P.BIRD_SELECT.WAKE_TO_SLEEP }), 0x00);
    assert.equal(flags({ birdModeSelect: P.BIRD_SELECT.PLUS_2_HOURS }), 0x20);
    assert.equal(flags({ birdModeSelect: P.BIRD_SELECT.PLUS_3_HOURS }), 0x40);
    assert.equal(flags({ birdModeSelect: P.BIRD_SELECT.PLUS_4_HOURS }), 0x60);
    // Bits 4 and 7 stay 0 with everything else set.
    assert.equal(flags({ suppressInterference: true, birdMode: 1, triggerCode: 3, birdModeSelect: 3 }), 0x6f);
});

test('reserved bytes stay 0', () => {
    const payload = P.payload({
        ...TOOL_SETTINGS, suppressInterference: true, birdMode: 1, triggerCode: 3, birdModeSelect: 3,
    });
    assert.equal(toHex(payload.subarray(17)), '00000000000000');
});

test('code constants have the values the format uses', () => {
    assert.deepEqual(Object.values(P.TRIGGER), [0, 1, 2, 3]);
    assert.deepEqual(Object.values(P.RECORDING_LENGTH), [0, 1, 2, 3]);
    assert.deepEqual(Object.values(P.BIRD_MODE), [0, 1]);
    assert.deepEqual(Object.values(P.BIRD_SELECT), [0, 1, 2, 3]);
    assert.equal(P.SAMPLE_RATE, 48000);
    assert.equal(P.START_BYTE_COUNT, 6);
});

test('out of range settings throw a RangeError that names the field', () => {
    const bad = {
        deviceId: [-1, 256, 1.5, '1', undefined, null],
        latitude: [-90.01, 90.01, NaN, Infinity, '1', undefined],
        longitude: [-180.01, 180.01, NaN, -Infinity, '1', undefined],
        epochSeconds: [0, -1, 1.5, P.MAX_EPOCH_SECONDS + 1, NaN, undefined],
        wakeHour: [-1, 24, 0.5, undefined],
        wakeQuarter: [-1, 4, 0.5, undefined],
        sleepHour: [-1, 24, 0.5, undefined],
        sleepQuarter: [-1, 4, 0.5, undefined],
        recordingLengthCode: [-1, 4, 0.5, undefined],
        triggerCode: [-1, 4, 7, 0.5, undefined],
        birdMode: [-1, 2, true, undefined],
        birdModeSelect: [-1, 4, 0.5, undefined],
        suppressInterference: [0, 1, 'yes', undefined, null],
    };
    for (const [field, values] of Object.entries(bad)) {
        for (const value of values) {
            assert.throws(
                () => P.payload({ ...TOOL_SETTINGS, [field]: value }),
                (error) => error instanceof RangeError && error.message.includes(field),
                `${field} = ${value}`,
            );
        }
    }
});

test('a location needs both values or neither', () => {
    const named = (error) => error instanceof RangeError && error.message.includes('latitude') && error.message.includes('longitude');
    assert.throws(() => P.payload({ ...TOOL_SETTINGS, latitude: null }), named);
    assert.throws(() => P.payload({ ...TOOL_SETTINGS, longitude: null }), named);
});

test('range limits are accepted', () => {
    const limits = {
        deviceId: [0, 255],
        latitude: [-90, 90],
        longitude: [-180, 180],
        epochSeconds: [1, P.MAX_EPOCH_SECONDS],
        wakeHour: [0, 23],
        wakeQuarter: [0, 3],
        sleepHour: [0, 23],
        sleepQuarter: [0, 3],
        recordingLengthCode: [0, 3],
        triggerCode: [0, 3],
        birdMode: [0, 1],
        birdModeSelect: [0, 3],
    };
    for (const [field, values] of Object.entries(limits)) {
        for (const value of values) {
            assert.equal(P.payload({ ...TOOL_SETTINGS, [field]: value }).length, 24);
        }
    }
});

test('frame and modulate refuse the wrong shape of input', () => {
    assert.throws(() => P.frame(new Uint8Array(23)), RangeError);
    assert.throws(() => P.frame(new Uint8Array(25)), RangeError);
    assert.throws(() => P.modulate(fromHex('a5 00')), RangeError);
    assert.throws(() => P.modulate(fromHex('ff ff')), RangeError);
});

test('fades follow the rule for each part of the frame', () => {
    const carrier = [0, 25000, 0, -25000];
    const full = [4, 4, 4, 4];
    const fadeIn = [1, 2, 3, 4];
    const fadeOut = [4, 3, 2, 1];
    const samples = P.modulate(P.frame(P.payload(TOOL_SETTINGS)));
    const expectCycle = (byteIndex, bit, cycle, polarity, gains, label) => {
        const at = (byteIndex * 8 + bit) * 16 + cycle * 4;
        // Adding 0 turns -0 into 0, because the samples are integers.
        const expected = carrier.map((value, i) => (polarity * value * gains[i]) / 4 + 0);
        assert.deepEqual(Array.from(samples.subarray(at, at + 4)), expected, label);
    };
    // The first 0xFF byte has no fades.
    for (let cycle = 0; cycle < 4; cycle += 1) {
        expectCycle(0, 0, cycle, 1, full, 'lead-in');
    }
    // In the 0x7F byte, bit 5 has no fades, bit 6 only fades out, and bit 7 is a 0 bit
    // that fades at both ends.
    expectCycle(6, 5, 3, 1, full, '0x7F bit 5');
    expectCycle(6, 6, 0, 1, full, '0x7F bit 6 first cycle');
    expectCycle(6, 6, 3, 1, fadeOut, '0x7F bit 6 last cycle');
    expectCycle(6, 7, 0, -1, fadeIn, '0x7F bit 7 first cycle');
    expectCycle(6, 7, 1, -1, full, '0x7F bit 7 second cycle');
    expectCycle(6, 7, 3, -1, fadeOut, '0x7F bit 7 last cycle');
    // 0xA5 starts with a 1 bit followed by a 0 bit. Every bit fades at both ends.
    expectCycle(7, 0, 0, 1, fadeIn, '0xA5 bit 0 first cycle');
    expectCycle(7, 0, 3, 1, fadeOut, '0xA5 bit 0 last cycle');
    expectCycle(7, 1, 0, -1, fadeIn, '0xA5 bit 1 first cycle');
    expectCycle(7, 1, 2, -1, full, '0xA5 bit 1 third cycle');
});

test('chirpSamples gives the requested copies with silence between them', () => {
    const chirp = P.modulate(P.frame(P.payload(TOOL_SETTINGS)));
    const gap = P.REPEAT_GAP_SAMPLES;

    const defaultCopies = P.chirpSamples(TOOL_SETTINGS);
    assert.ok(defaultCopies instanceof Float32Array);
    assert.equal(P.DEFAULT_REPEATS, 3);
    assert.equal(defaultCopies.length, 3 * chirp.length + 2 * gap);

    const one = P.chirpSamples(TOOL_SETTINGS, 1);
    assert.equal(one.length, chirp.length);
    assert.ok(one.every((value, i) => value === Math.fround(chirp[i] / 32768)));

    for (let copy = 0; copy < 3; copy += 1) {
        const start = copy * (chirp.length + gap);
        assert.ok(defaultCopies.subarray(start, start + chirp.length).every((value, i) => value === one[i]), `copy ${copy}`);
        if (copy < 2) {
            assert.ok(defaultCopies.subarray(start + chirp.length, start + chirp.length + gap).every((value) => value === 0), `gap ${copy}`);
        }
    }

    for (const repeats of [0, -1, 1.5, '3', null]) {
        assert.throws(() => P.chirpSamples(TOOL_SETTINGS, repeats), (error) => error instanceof RangeError && error.message.includes('repeats'));
    }
});

test('localEpoch adds the offset from UTC, including daylight saving', () => {
    const at = (milliseconds, timezoneOffsetMinutes) => ({
        getTime: () => milliseconds,
        getTimezoneOffset: () => timezoneOffsetMinutes,
    });
    assert.equal(P.localEpoch(at(1740220054000, 0)), 1740220054);
    // getTimezoneOffset is positive west of UTC. UK summer time is -60.
    assert.equal(P.localEpoch(at(1751328000000, -60)), 1751328000 + 3600);
    assert.equal(P.localEpoch(at(1751328000000, 300)), 1751328000 - 18000);
    assert.equal(P.localEpoch(at(1751328000000, -330)), 1751328000 + 19800);
    assert.equal(P.localEpoch(at(1740220054999, 0)), 1740220054);
    const date = new Date(1751328000000);
    assert.equal(P.localEpoch(date), 1751328000 - date.getTimezoneOffset() * 60);
});

test('bytes round trip through our own correlator', () => {
    const shared = { ...TOOL_SETTINGS };
    const combinations = [
        {},
        { latitude: -12.5, longitude: -0.25, wakeHour: 0, wakeQuarter: 0, sleepHour: 23, sleepQuarter: 3 },
        { latitude: 1, longitude: 180, wakeHour: 23, wakeQuarter: 3, sleepHour: 0, sleepQuarter: 0, recordingLengthCode: 1, birdMode: 1, birdModeSelect: 0, triggerCode: 1 },
        { recordingLengthCode: 2, birdMode: 1, birdModeSelect: 1, triggerCode: 2 },
        { recordingLengthCode: 3, birdMode: 1, birdModeSelect: 2, triggerCode: 3, suppressInterference: true },
        { recordingLengthCode: 3, birdMode: 1, birdModeSelect: 3, deviceId: 255, latitude: null, longitude: null },
        { latitude: null, longitude: null, deviceId: 0, wakeHour: 0, wakeQuarter: 0, sleepHour: 0, sleepQuarter: 0 },
        { epochSeconds: P.MAX_EPOCH_SECONDS },
    ];
    for (const change of combinations) {
        const settings = { ...shared, ...change };
        const expectedFrame = P.frame(P.payload(settings));
        const parsed = parseFrame(expectedFrame);
        for (const field of ['deviceId', 'epochSeconds', 'wakeHour', 'wakeQuarter', 'sleepHour', 'sleepQuarter', 'recordingLengthCode', 'suppressInterference', 'birdMode', 'triggerCode', 'birdModeSelect']) {
            assert.equal(parsed[field], settings[field], field);
        }
        if (settings.latitude === null) {
            assert.equal(parsed.latitudeRaw, 0);
            assert.equal(parsed.longitudeRaw, 0);
        } else {
            assert.ok(Math.abs(parsed.latitudeRaw / 2 ** 22 - settings.latitude) < 1e-5);
            assert.ok(Math.abs(parsed.longitudeRaw / 2 ** 22 - settings.longitude) < 1e-5);
        }

        // Modulated bytes round trip through the correlator.
        const decoded = demodulateFrom(P.modulate(expectedFrame), 0, expectedFrame.length);
        assert.equal(toHex(decoded.bytes), toHex(expectedFrame));

        // Each of the three copies from chirpSamples round trips, with some silence before them.
        const floats = P.chirpSamples(settings);
        const withSilence = new Float32Array(floats.length + 37);
        withSilence.set(floats, 37);
        let searchFrom = 0;
        for (let copy = 0; copy < 3; copy += 1) {
            const result = demodulateFrom(withSilence, searchFrom, expectedFrame.length);
            assert.ok(result, `copy ${copy} found`);
            assert.equal(toHex(result.bytes), toHex(expectedFrame), `copy ${copy}`);
            searchFrom = result.next;
        }
        assert.equal(demodulateFrom(withSilence, searchFrom, expectedFrame.length), null);
    }
});
