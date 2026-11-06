#!/usr/bin/env node
// Renders an AudioMoth chime to a WAV file by running Open Acoustic Devices'
// JavaScript encoder from web/recorder-setup/ in Node, with a stub for the browser's
// AudioContext. The chime crate uses the output as a reference for its tests.
//
// How to run it (Node only, no npm packages):
//
//   node tools/chime_render.mjs --rate 48000 --unix 1793000000 --tz 660 --out chime.wav
//
// Required arguments:
//   --rate N          sample rate in Hz
//   --unix T          Unix time to encode, in seconds
//   --tz MINUTES      time zone offset in minutes, so that UTC+1 is 60
//   --out FILE        output WAV file, mono
//
// Optional arguments:
//   --float               write 32-bit float samples, not 16-bit PCM
//   --location LAT,LON    add a location, in degrees, for a 13-byte payload
//   --deployment HEX16    add a deployment ID of 16 hex digits, for a 14-byte
//                         payload, or 21 bytes together with --location
//
// 16-bit samples are Math.round(x * 32767). The encoder's samples stay within
// +-0.75, so this script never has to clip them.
//
// The script prints the payload bytes as the encoder sends them, without the CRC
// that the encoder appends. The deployment ID appears byte-reversed in the
// payload, because the connector reverses it.
//
// The connector rounds the time it is given up to the next whole second. The
// script gives it a time 500 ms before --unix, so the encoded time is exactly
// --unix. The encoder waits for that time if it is in the future, so the script
// replaces setTimeout during rendering with one that has no delay.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const VENDOR_DIR = join(dirname(fileURLToPath(import.meta.url)), '..', 'web', 'recorder-setup');

function fail(message) {
    console.error(`chime_render: ${message}`);
    process.exit(2);
}

function parseArguments(argv) {
    const args = { float: false };
    for (let i = 0; i < argv.length; i += 1) {
        const name = argv[i];
        if (name === '--float') {
            args.float = true;
        } else if (['--rate', '--unix', '--tz', '--out', '--location', '--deployment'].includes(name)) {
            i += 1;
            if (i >= argv.length) fail(`${name} needs a value`);
            args[name.slice(2)] = argv[i];
        } else {
            fail(`unknown argument ${name}`);
        }
    }
    for (const name of ['rate', 'unix', 'tz', 'out']) {
        if (args[name] === undefined) fail(`--${name} is required`);
    }

    const parsed = {
        float: args.float,
        out: args.out,
        rate: Number(args.rate),
        unix: Number(args.unix),
        tz: Number(args.tz),
        latitude: null,
        longitude: null,
        deployment: null,
    };
    if (!Number.isInteger(parsed.rate) || parsed.rate <= 0) fail('--rate must be a positive whole number');
    if (!Number.isInteger(parsed.unix) || parsed.unix < 0) fail('--unix must be a whole number of seconds, 0 or more');
    if (!Number.isInteger(parsed.tz)) fail('--tz must be a whole number of minutes');

    if (args.location !== undefined) {
        const parts = args.location.split(',').map(Number);
        if (parts.length !== 2 || parts.some(Number.isNaN)) fail('--location must be LAT,LON in degrees');
        [parsed.latitude, parsed.longitude] = parts;
    }
    if (args.deployment !== undefined) {
        if (!/^[0-9a-fA-F]{16}$/.test(args.deployment)) fail('--deployment must be 16 hex digits');
        parsed.deployment = args.deployment.match(/../g).map((pair) => parseInt(pair, 16));
    }
    return parsed;
}

/* Stub for the part of the Web Audio API that the encoder uses. */

function installAudioContextStub(sampleRate, onRendered) {
    class StubAudioContext {
        constructor() {
            this.sampleRate = sampleRate;
            this.state = 'running';
            this.destination = {};
        }

        resume() {
            return Promise.resolve();
        }

        createBuffer(channels, length, rate) {
            const data = new Float32Array(length);
            return {
                numberOfChannels: channels,
                length,
                sampleRate: rate,
                getChannelData: () => data,
            };
        }

        createBufferSource() {
            return {
                buffer: null,
                onended: null,
                connect() {},
                start() {
                    onRendered(this.buffer.getChannelData(0));
                    this.onended();
                },
            };
        }
    }
    globalThis.window = { AudioContext: StubAudioContext };
}

function loadVendoredScript(name) {
    const path = join(VENDOR_DIR, name);
    vm.runInThisContext(readFileSync(path, 'utf8'), { filename: path });
}

/* Runs the encoder and returns the samples and the payload it encoded. */

function render(options) {
    return new Promise((resolve) => {
        const result = { samples: null, payload: null };

        installAudioContextStub(options.rate, (samples) => {
            result.samples = samples;
        });

        loadVendoredScript('audiomothchime.js');

        // The connector does not expose the payload it builds, so this wrapper
        // records the bytes on their way to the encoder.
        const RealAudioMothChime = globalThis.AudioMothChime;
        globalThis.AudioMothChime = function () {
            const chime = new RealAudioMothChime();
            const realChime = chime.chime;
            chime.chime = (sendTime, bytes, notes, callback) => {
                result.payload = bytes.slice();
                realChime(sendTime, bytes, notes, callback);
            };
            return chime;
        };

        loadVendoredScript('audiomothchime_connector.js');

        const realSetTimeout = globalThis.setTimeout;
        const realLog = console.log;
        globalThis.setTimeout = (callback) => realSetTimeout(callback, 0);
        console.log = () => {};

        const finished = () => {
            globalThis.setTimeout = realSetTimeout;
            console.log = realLog;
            resolve(result);
        };

        const connector = new globalThis.AudioMothChimeConnector();
        const date = new Date(options.unix * 1000 - 500);
        if (options.deployment) {
            connector.playTimeAndDeploymentID(date, options.tz, options.latitude, options.longitude, options.deployment, finished);
        } else {
            connector.playTime(date, options.tz, options.latitude, options.longitude, finished);
        }
    });
}

/* WAV writer */

function wavBytes(samples, sampleRate, useFloat) {
    const bytesPerSample = useFloat ? 4 : 2;
    const dataBytes = samples.length * bytesPerSample;
    const buffer = Buffer.alloc(44 + dataBytes);
    buffer.write('RIFF', 0, 'latin1');
    buffer.writeUInt32LE(36 + dataBytes, 4);
    buffer.write('WAVE', 8, 'latin1');
    buffer.write('fmt ', 12, 'latin1');
    buffer.writeUInt32LE(16, 16);
    buffer.writeUInt16LE(useFloat ? 3 : 1, 20);
    buffer.writeUInt16LE(1, 22);
    buffer.writeUInt32LE(sampleRate, 24);
    buffer.writeUInt32LE(sampleRate * bytesPerSample, 28);
    buffer.writeUInt16LE(bytesPerSample, 32);
    buffer.writeUInt16LE(bytesPerSample * 8, 34);
    buffer.write('data', 36, 'latin1');
    buffer.writeUInt32LE(dataBytes, 40);
    for (let i = 0; i < samples.length; i += 1) {
        if (useFloat) {
            buffer.writeFloatLE(samples[i], 44 + i * 4);
        } else {
            const value = Math.max(-32768, Math.min(32767, Math.round(samples[i] * 32767)));
            buffer.writeInt16LE(value, 44 + i * 2);
        }
    }
    return buffer;
}

const options = parseArguments(process.argv.slice(2));
const { samples, payload } = await render(options);
writeFileSync(options.out, wavBytes(samples, options.rate, options.float));

const hex = payload.map((byte) => byte.toString(16).padStart(2, '0')).join('');
console.log(`Payload: ${hex} (${payload.length} bytes, without the CRC)`);
console.log(`Length: ${samples.length} samples`);
console.log(`Duration: ${(samples.length / options.rate).toFixed(6)} s`);
