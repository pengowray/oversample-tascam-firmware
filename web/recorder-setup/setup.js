'use strict';

/* Plays chimes (unmodified AudioMoth library) and chirps (pippyg_chirp.js) from
   the three recorder tabs. Saved in this browser: the selected tab and the
   PippyG and clock-set choices. Location and deployment ID are never saved,
   because a stale one would go into recordings without anyone noticing. */
(function () {
    const TEXT = {
        play: 'Play chimes for 1 minute',
        playAfterTone: 'Play 10 s tone, then chimes',
        stop: 'Stop chimes',
        stopToneAndChimes: 'Stop tone and chimes',
        waiting: 'Waiting for the first chime to start.',
        stoppedEarly: 'Stopped before the first chime.',
        noAudio: 'This browser cannot play audio from a web page. Open this page in a different browser.',
        failed: 'Audio failed to start: reason unknown. Press the button to try again.',
        triggerVeryHighFirmware8: 'On firmware 8, Very high records continuously.',
        x8Idle: "The phone's clock needs to be right. The chime contains the phone's date and time.",
        chirp: 'Play settings chirp',
        chirpAgain: 'Play settings chirp again',
        chirpIdle: "The phone's clock needs to be right. The chirp contains the phone's local time and the settings above.",
        chirpWaiting: 'Waiting for the next whole second to play the chirp.',
        chirpMisread: 'Older firmware can misread a chirp without warning, so check the first recording\'s time and location.',
        locating: "Getting the phone's location",
        locationRefused: 'Location permission refused: allow location for this page, or type the latitude and longitude.',
        locationInsecure: 'Secure page needed: this browser offers location only on https. Type the latitude and longitude.',
        locationNotFound: 'Location not found: try again, or type the latitude and longitude.',
        latitudeInvalid: 'Latitude not valid: enter a number from -90 to 90.',
        longitudeInvalid: 'Longitude not valid: enter a number from -180 to 180.',
        longitudeMissing: 'Longitude missing: fill in longitude, or clear latitude.',
        latitudeMissing: 'Latitude missing: fill in latitude, or clear longitude.',
        idNotHex: 'Deployment ID not hexadecimal: use only 0 to 9 and A to F.',
        deviceIdInvalid: 'Device ID not valid: enter a whole number from 0 to 255.'
    };

    function chimes(count) {
        return count === 1 ? '1 chime' : count + ' chimes';
    }

    /* The connector starts each chime on the next whole second that is at
       least this far ahead. The page uses the same rule to show the time in
       the chime, because the library does not report it. */
    const MINIMUM_DELAY_MS = 200;
    const GAP_BETWEEN_CHIMES_MS = 1000;
    const RUN_LIMIT_MS = 60000;
    const CHIME_LIMIT = 20;
    const TONE_MS = 10000;
    const AUDIO_START_TIMEOUT_MS = 2000;
    /* The longest chime, with location and deployment ID, lasts under 3.5 s. */
    const CHIME_END_TIMEOUT_MS = 5000;
    /* Longer than the 15 s the browser allows a location lookup, and time to answer the
       permission prompt. Some prompts can be left open and then give no answer at all. */
    const LOCATION_WAIT_MS = 30000;
    const MINUTES_PER_DAY = 1440;

    const STORAGE_KEY = 'bat-recorder-setup';
    const DEFAULT_WAKE_MINUTES = 18 * 60;
    const DEFAULT_SLEEP_MINUTES = 7 * 60;
    const DEFAULT_TRIGGER_CODE = 1; /* The manual calls 48 Normal. */

    /* Recording length per file, by recording length code. */
    const LENGTH_MB = ['3.8', '8', '12', '16'];
    const LENGTH_S_BAT = [5, 10, 15, 20];
    const LENGTH_S_BIRD = [40, 79, 119, 160];

    /* The hours each Recording Mode option keeps at each end of the awake period. */
    const BIRD_EXTRA_HOURS = [0, 0, 2, 3, 4];

    const RealAudioContext = window.AudioContext || window.webkitAudioContext;

    const element = (id) => document.getElementById(id);

    function loadSaved() {
        try {
            const value = JSON.parse(localStorage.getItem(STORAGE_KEY));
            return value !== null && typeof value === 'object' && !Array.isArray(value) ? value : {};
        } catch (error) {
            return {};
        }
    }

    const saved = loadSaved();

    let chirpPlayed = false;

    function save() {
        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(saved));
        } catch (error) {
            /* Private windows can refuse storage. The page still works. */
        }
    }

    /* Text helpers */

    function pad(value) {
        return String(value).padStart(2, '0');
    }

    function dateText(date) {
        return date.getFullYear() + '-' + pad(date.getMonth() + 1) + '-' + pad(date.getDate());
    }

    function timeText(date) {
        return pad(date.getHours()) + ':' + pad(date.getMinutes()) + ':' + pad(date.getSeconds());
    }

    function zoneText(date) {
        const minutes = -date.getTimezoneOffset();
        const size = Math.abs(minutes);
        return 'UTC' + (minutes < 0 ? '-' : '+') + pad(Math.floor(size / 60)) + ':' + pad(size % 60);
    }

    function clockText(minutes) {
        const wrapped = (minutes + MINUTES_PER_DAY) % MINUTES_PER_DAY;
        return pad(Math.floor(wrapped / 60)) + ':' + pad(wrapped % 60);
    }

    function durationText(minutes) {
        const hours = Math.floor(minutes / 60);
        const rest = minutes % 60;
        const parts = [];
        if (hours > 0) parts.push(hours + (hours === 1 ? ' hour' : ' hours'));
        if (rest > 0) parts.push(rest + ' minutes');
        return parts.join(' ');
    }

    function showClock() {
        const now = new Date();
        element('time').textContent = timeText(now);
        element('date').textContent = dateText(now);
        element('zone').textContent = zoneText(now);
        setTimeout(showClock, 1000 - now.getMilliseconds());
    }

    /* Panels. Each has a send button, a status line, and its own idle text. */

    function audiomothIdleText() {
        const hasLocation = element('am-lat').value.trim() !== '' || element('am-lon').value.trim() !== '';
        const hasId = element('am-id').value.trim() !== '';
        let list = "the phone's date, time and UTC offset";
        if (hasLocation && hasId) list = "the phone's date, time, UTC offset, location and deployment ID";
        else if (hasLocation) list = "the phone's date, time, UTC offset and location";
        else if (hasId) list = "the phone's date, time, UTC offset and deployment ID";
        return "The phone's clock needs to be right. The chime contains " + list + '.';
    }

    const panels = {
        x8: {
            section: element('panel-x8'),
            button: element('x8-send'),
            status: element('x8-status'),
            sendLabel: () => TEXT.play,
            idleText: () => TEXT.x8Idle,
            send: sendX8
        },
        audiomoth: {
            section: element('panel-audiomoth'),
            button: element('audiomoth-send'),
            status: element('audiomoth-status'),
            sendLabel: () => element('am-clock-set').checked ? TEXT.playAfterTone : TEXT.play,
            idleText: audiomothIdleText,
            send: sendAudiomoth
        },
        pippyg: {
            section: element('panel-pippyg'),
            button: element('pippyg-send'),
            status: element('pippyg-status'),
            sendLabel: () => chirpPlayed ? TEXT.chirpAgain : TEXT.chirp,
            idleText: () => TEXT.chirpIdle,
            send: sendPippyg
        }
    };

    /* Any new status replaces an error, so it also clears the mark on that error's field. */
    function showStatus(panel, text, isError) {
        panel.status.textContent = text;
        panel.status.classList.toggle('error', Boolean(isError));
        panel.section.querySelectorAll('[aria-invalid]').forEach(function (field) {
            field.removeAttribute('aria-invalid');
        });
    }

    /* The second part stays on one line, so the date never breaks at a hyphen. */
    function showPlayed(panel, count, chimeTime) {
        const latest = document.createElement('span');
        latest.className = 'latest';
        latest.textContent = 'Latest chime: ' + dateText(chimeTime) + ' ' + timeText(chimeTime);
        showStatus(panel, count + ' of up to ' + CHIME_LIMIT + ' chimes played. ');
        panel.status.append(latest);
    }

    function showIdle(panel) {
        showStatus(panel, panel.idleText());
    }

    /* The send button reads the fields once, when it is pressed. So during a run a change
       to a field would look applied but not be, and the fields are locked. */
    function lockFields(panel, locked) {
        panel.section.querySelectorAll('input, select, .plain').forEach(function (control) {
            control.disabled = locked;
        });
    }

    /* A run is one press: audio that is starting, playing or waiting to repeat. */

    let run = null;

    function nextChimeTime(now) {
        let delay = 1000 - now.getMilliseconds();
        if (delay < MINIMUM_DELAY_MS) delay += 1000;
        return new Date(now.getTime() + delay);
    }

    function finish(thisRun, showResult) {
        run = null;
        clearTimeout(thisRun.startTimer);
        clearTimeout(thisRun.gapTimer);
        clearTimeout(thisRun.endTimer);
        clearInterval(thisRun.toneTimer);
        if (thisRun.wakeLock) thisRun.wakeLock.release().catch(function () {});
        /* Closing the context silences anything that is playing. */
        if (thisRun.context) thisRun.context.close().catch(function () {});
        thisRun.panel.button.disabled = false;
        thisRun.panel.button.textContent = thisRun.panel.sendLabel();
        lockFields(thisRun.panel, false);
        showResult();
    }

    function failRun(thisRun) {
        if (run !== thisRun) return;
        finish(thisRun, function () {
            const text = thisRun.count === 0 ? TEXT.failed : 'Audio stopped after ' + chimes(thisRun.count) + '. Press the button to try again.';
            showStatus(thisRun.panel, text, true);
        });
    }

    /* A context at the chirp's own rate plays it without the browser's resampler, which
       lowers its 12 kHz carrier. Older browsers can refuse this option. */
    function newAudioContext(sampleRate) {
        if (sampleRate) {
            try {
                return new RealAudioContext({ sampleRate: sampleRate });
            } catch (error) {
                /* The default rate below still plays the chirp. */
            }
        }
        return new RealAudioContext();
    }

    /* Call during a press: iOS only starts audio that is created during one.
       Calls `ready` once the audio is running. `sampleRate` is optional. */
    function startRun(panel, ready, sampleRate) {
        const thisRun = { panel: panel, context: null, connector: null, wakeLock: null, count: 0, deadline: 0 };
        run = thisRun;
        lockFields(panel, true);
        try {
            thisRun.context = newAudioContext(sampleRate);
            /* Safari 16.4 and later: play even with the side switch on silent. */
            if (navigator.audioSession) navigator.audioSession.type = 'playback';
            const timeout = new Promise(function (resolve) { setTimeout(resolve, AUDIO_START_TIMEOUT_MS); });
            Promise.race([thisRun.context.resume(), timeout]).then(function () {
                if (run !== thisRun) return;
                if (thisRun.context.state !== 'running') {
                    failRun(thisRun);
                    return;
                }
                ready(thisRun);
            }).catch(function () { failRun(thisRun); });
        } catch (error) {
            failRun(thisRun);
        }
        return thisRun;
    }

    /* Chimes, for the X8 and AudioMoth tabs */

    function playNextChime(thisRun, playChime) {
        const now = new Date();
        const chimeTime = nextChimeTime(now);
        const wait = chimeTime.getTime() - now.getTime();

        thisRun.startTimer = setTimeout(function () {
            thisRun.count += 1;
            showPlayed(thisRun.panel, thisRun.count, chimeTime);
        }, wait);

        thisRun.endTimer = setTimeout(function () { failRun(thisRun); }, wait + CHIME_END_TIMEOUT_MS);

        playChime(thisRun.connector, now, function () {
            if (run !== thisRun) return;
            clearTimeout(thisRun.endTimer);
            if (thisRun.count >= CHIME_LIMIT || Date.now() >= thisRun.deadline) {
                finish(thisRun, function () {
                    showStatus(thisRun.panel, 'Stopped automatically after 1 minute. ' + chimes(thisRun.count) + ' played.');
                });
            } else {
                thisRun.gapTimer = setTimeout(function () { playNextChime(thisRun, playChime); }, GAP_BETWEEN_CHIMES_MS);
            }
        });
    }

    function startChimeLoop(thisRun, playChime) {
        thisRun.panel.button.textContent = TEXT.stop;
        showStatus(thisRun.panel, TEXT.waiting);
        thisRun.deadline = Date.now() + RUN_LIMIT_MS;
        playNextChime(thisRun, playChime);
    }

    function toneText(secondsLeft) {
        return 'Move the switch to CUSTOM now. Tone ends in ' + secondsLeft + ' s.';
    }

    function playToneThenChimes(thisRun, playChime) {
        let secondsLeft = TONE_MS / 1000;
        thisRun.toneTimer = setInterval(function () {
            secondsLeft -= 1;
            if (secondsLeft > 0) showStatus(thisRun.panel, toneText(secondsLeft));
        }, 1000);
        thisRun.endTimer = setTimeout(function () { failRun(thisRun); }, TONE_MS + CHIME_END_TIMEOUT_MS);
        thisRun.connector.playTone(TONE_MS, function () {
            if (run !== thisRun) return;
            clearInterval(thisRun.toneTimer);
            clearTimeout(thisRun.endTimer);
            startChimeLoop(thisRun, playChime);
        });
    }

    function startChimes(panel, withTone, playChime) {
        if (!RealAudioContext) {
            showStatus(panel, TEXT.noAudio, true);
            return;
        }
        panel.button.textContent = withTone ? TEXT.stopToneAndChimes : TEXT.stop;
        showStatus(panel, withTone ? toneText(TONE_MS / 1000) : TEXT.waiting);
        const thisRun = startRun(panel, function (started) {
            /* The library calls new window.AudioContext() once per connector.
               A constructor that returns an object makes "new" give that object. */
            window.AudioContext = function () { return started.context; };
            started.connector = new window.AudioMothChimeConnector();
            if (withTone) playToneThenChimes(started, playChime);
            else startChimeLoop(started, playChime);
        });
        /* Some phones turn the screen off after 30 s, which would stop the timers.
           Browsers offer this only on https and localhost. */
        if (navigator.wakeLock) {
            navigator.wakeLock.request('screen').then(function (lock) {
                if (run === thisRun) thisRun.wakeLock = lock;
                else lock.release().catch(function () {});
            }).catch(function () {});
        }
    }

    function sendX8() {
        startChimes(panels.x8, false, function (connector, now, done) {
            connector.playTime(now, null, null, null, done);
        });
    }

    /* Field checks. Each returns the value, or shows an error and returns undefined. */

    /* On a touch screen, focus would open the keyboard, which can cover the error. The
       error names the field, and the press handler scrolls the error into view. */
    function fieldError(panel, field, text) {
        showStatus(panel, text, true);
        element(field).setAttribute('aria-invalid', 'true');
        if (window.matchMedia('(pointer: fine)').matches) element(field).focus({ preventScroll: true });
        return undefined;
    }

    /* Number() alone also accepts forms such as 0x10 and 1e1. */
    function decimalNumber(text) {
        return /^[-+]?(\d+\.?\d*|\.\d+)$/.test(text) ? Number(text) : NaN;
    }

    /* Returns { latitude, longitude }, both null when both fields are blank. */
    function readLocation(panel, latitudeField, longitudeField) {
        const latitudeText = element(latitudeField).value.trim();
        const longitudeText = element(longitudeField).value.trim();
        if (latitudeText === '' && longitudeText === '') return { latitude: null, longitude: null };
        const latitude = decimalNumber(latitudeText);
        const longitude = decimalNumber(longitudeText);
        /* A wrong value is reported before a missing one, so 200 with a blank longitude
           does not ask for a longitude and then fail on the latitude. */
        if (latitudeText !== '' && !(Math.abs(latitude) <= 90)) return fieldError(panel, latitudeField, TEXT.latitudeInvalid);
        if (longitudeText !== '' && !(Math.abs(longitude) <= 180)) return fieldError(panel, longitudeField, TEXT.longitudeInvalid);
        if (longitudeText === '') return fieldError(panel, longitudeField, TEXT.longitudeMissing);
        if (latitudeText === '') return fieldError(panel, latitudeField, TEXT.latitudeMissing);
        return { latitude: latitude, longitude: longitude };
    }

    /* Returns 8 byte values in the order typed, which is the order AudioMoth
       prints them: the connector sends them last byte first. Null when blank. */
    function readDeploymentId(panel) {
        const text = element('am-id').value.trim();
        if (text === '') return null;
        if (text.length !== 16) {
            const count = text.length === 1 ? '1 character' : text.length + ' characters';
            return fieldError(panel, 'am-id', 'Deployment ID wrong length: ' + count + '. Enter exactly 16 hexadecimal digits.');
        }
        if (!/^[0-9a-fA-F]+$/.test(text)) return fieldError(panel, 'am-id', TEXT.idNotHex);
        const bytes = [];
        for (let i = 0; i < 16; i += 2) bytes.push(parseInt(text.slice(i, i + 2), 16));
        return bytes;
    }

    function sendAudiomoth() {
        const panel = panels.audiomoth;
        const location = readLocation(panel, 'am-lat', 'am-lon');
        if (location === undefined) return;
        const id = readDeploymentId(panel);
        if (id === undefined) return;
        startChimes(panel, element('am-clock-set').checked, function (connector, now, done) {
            if (id) connector.playTimeAndDeploymentID(now, null, location.latitude, location.longitude, id, done);
            else connector.playTime(now, null, location.latitude, location.longitude, done);
        });
    }

    /* Chirp, for the PippyG tab */

    function readPippygSettings(panel) {
        const idText = element('pg-id').value.trim();
        if (!/^\d+$/.test(idText) || Number(idText) > 255) return fieldError(panel, 'pg-id', TEXT.deviceIdInvalid);
        const location = readLocation(panel, 'pg-lat', 'pg-lon');
        if (location === undefined) return undefined;
        const wake = Number(element('pg-wake').value);
        const sleep = Number(element('pg-sleep').value);
        const mode = Number(element('pg-mode').value);
        return {
            deviceId: Number(idText),
            latitude: location.latitude,
            longitude: location.longitude,
            wakeHour: Math.floor(wake / 60),
            wakeQuarter: (wake % 60) / 15,
            sleepHour: Math.floor(sleep / 60),
            sleepQuarter: (sleep % 60) / 15,
            recordingLengthCode: Number(element('pg-length').value),
            triggerCode: Number(element('pg-trigger').value),
            birdMode: mode === 0 ? 0 : 1,
            birdModeSelect: mode === 0 ? 0 : mode - 1,
            suppressInterference: element('pg-noise').checked
        };
    }

    function sendPippyg() {
        const panel = panels.pippyg;
        const settings = readPippygSettings(panel);
        if (settings === undefined) return;
        if (!RealAudioContext) {
            showStatus(panel, TEXT.noAudio, true);
            return;
        }
        panel.button.disabled = true;
        showStatus(panel, TEXT.chirpWaiting);
        startRun(panel, function (thisRun) {
            const now = new Date();
            const chirpTime = nextChimeTime(now);
            const wait = (chirpTime.getTime() - now.getTime()) / 1000;
            settings.epochSeconds = window.PippygChirp.localEpoch(chirpTime);
            const samples = window.PippygChirp.chirpSamples(settings);
            const buffer = thisRun.context.createBuffer(1, samples.length, window.PippygChirp.SAMPLE_RATE);
            buffer.getChannelData(0).set(samples);
            const source = thisRun.context.createBufferSource();
            source.buffer = buffer;
            source.connect(thisRun.context.destination);
            source.onended = function () {
                if (run !== thisRun) return;
                chirpPlayed = true;
                finish(thisRun, function () {
                    showStatus(panel, 'Chirp played, with local time ' + dateText(chirpTime) + ' ' + timeText(chirpTime) + '. ' + TEXT.chirpMisread);
                });
            };
            thisRun.endTimer = setTimeout(function () { failRun(thisRun); }, (wait * 1000) + CHIME_END_TIMEOUT_MS);
            /* The audio clock starts the chirp on the second more exactly than a timer. */
            source.start(thisRun.context.currentTime + wait);
        }, window.PippygChirp.SAMPLE_RATE);
    }

    /* The result of the PippyG's wake, sleep and mode settings */

    function awakeText() {
        const wake = Number(element('pg-wake').value);
        const sleep = Number(element('pg-sleep').value);
        const extraHours = BIRD_EXTRA_HOURS[Number(element('pg-mode').value)];
        const span = (sleep - wake + MINUTES_PER_DAY) % MINUTES_PER_DAY;
        if (extraHours === 0) {
            if (span === 0) return 'PippyG awake all the time.';
            const overnight = wake > sleep ? ', overnight' : '';
            return 'PippyG awake ' + durationText(span) + ' a day' + overnight + ': ' + clockText(wake) + ' to ' + clockText(sleep) + '.';
        }
        if (span === 0) return 'Not checked: Bird + ' + extraHours + ' hours when Wake Time equals Sleep Time.';
        const blocks = birdBlocks(wake, sleep, extraHours * 60);
        const minutes = blocks.reduce(function (total, block) {
            return total + (block[1] - block[0] + MINUTES_PER_DAY) % MINUTES_PER_DAY;
        }, 0);
        const times = blocks.map(function (block) { return clockText(block[0]) + ' to ' + clockText(block[1]); });
        return 'PippyG awake ' + durationText(minutes) + ' a day at 48 kHz: ' + times.join(' and ') + '.';
    }

    /* The blocks of a Bird + N setting, as [start, end] in minutes since midnight. Firmware 8
       and 10 compare the time of day with plain wake + extra and sleep - extra, which do not
       wrap at midnight. So a block that reaches midnight stops there. Blocks that touch
       become one block, which can run past midnight. */
    function birdBlocks(wake, sleep, extra) {
        const first = [wake, Math.min(wake + extra, MINUTES_PER_DAY)];
        const second = [Math.max(sleep - extra, 0), sleep];
        const overnight = wake > sleep;
        const joined = overnight ? first[1] === MINUTES_PER_DAY && second[0] === 0 : first[1] >= second[0];
        if (joined) return [[wake, sleep]];
        return [first, second].filter(function (block) { return block[1] > block[0]; });
    }

    function showPippygChoices() {
        const bird = Number(element('pg-mode').value) !== 0;
        element('pg-trigger-top').textContent = bird ? 'Continuous' : 'Very high';
        element('pg-trigger-note').textContent = bird ? '' : TEXT.triggerVeryHighFirmware8;
        const seconds = bird ? LENGTH_S_BIRD : LENGTH_S_BAT;
        Array.from(element('pg-length').options).forEach(function (option, code) {
            option.textContent = seconds[code] + ' s (' + LENGTH_MB[code] + ' MB)';
        });
        element('pg-awake').textContent = awakeText();
    }

    function savePippygChoices() {
        saved.pippyg = {
            mode: element('pg-mode').value,
            trigger: element('pg-trigger').value,
            wake: element('pg-wake').value,
            sleep: element('pg-sleep').value,
            id: element('pg-id').value,
            length: element('pg-length').value,
            noise: element('pg-noise').checked
        };
        save();
    }

    /* A saved value that is not one of the options would leave the select blank. */
    function setSelect(id, value, fallback) {
        element(id).value = value;
        if (element(id).selectedIndex < 0) element(id).value = fallback;
    }

    function setUpPippygFields() {
        ['pg-wake', 'pg-sleep'].forEach(function (id) {
            const select = element(id);
            for (let minutes = 0; minutes < MINUTES_PER_DAY; minutes += 15) {
                select.add(new Option(clockText(minutes), String(minutes)));
            }
        });
        const choices = saved.pippyg || {};
        setSelect('pg-mode', choices.mode, '0');
        setSelect('pg-trigger', choices.trigger, String(DEFAULT_TRIGGER_CODE));
        setSelect('pg-wake', choices.wake, String(DEFAULT_WAKE_MINUTES));
        setSelect('pg-sleep', choices.sleep, String(DEFAULT_SLEEP_MINUTES));
        element('pg-id').value = choices.id || '0';
        setSelect('pg-length', choices.length, '0');
        element('pg-noise').checked = Boolean(choices.noise);
        showPippygChoices();
        ['pg-mode', 'pg-trigger', 'pg-wake', 'pg-sleep', 'pg-id', 'pg-length', 'pg-noise'].forEach(function (id) {
            element(id).addEventListener('change', function () {
                showPippygChoices();
                savePippygChoices();
            });
        });
        ['pg-id', 'pg-lat', 'pg-lon'].forEach(function (id) {
            element(id).addEventListener('input', function () {
                if (panels.pippyg.status.classList.contains('error')) showIdle(panels.pippyg);
            });
        });
    }

    /* Filling in fields from the phone */

    /* The send button stays off until the lookup ends, because a chime or chirp sent now
       would leave out the location that the fields are about to show. */
    function useLocation(panel, locateButton, latitudeField, longitudeField) {
        /* The status line can be far below this button, off the screen. */
        function showLocationError(text) {
            showStatus(panel, text, true);
            panel.status.scrollIntoView({ block: 'nearest' });
        }
        if (!window.isSecureContext) {
            showLocationError(TEXT.locationInsecure);
            return;
        }
        if (!navigator.geolocation) {
            showLocationError(TEXT.locationNotFound);
            return;
        }
        showIdle(panel);
        lockFields(panel, true);
        panel.button.disabled = true;
        const buttonLabel = locateButton.textContent;
        locateButton.textContent = TEXT.locating;
        let lookupOver = false;
        function endLookup(showResult) {
            if (lookupOver) return;
            lookupOver = true;
            clearTimeout(lookupTimer);
            lockFields(panel, false);
            panel.button.disabled = false;
            locateButton.textContent = buttonLabel;
            showResult();
        }
        const lookupTimer = setTimeout(function () {
            endLookup(function () { showLocationError(TEXT.locationNotFound); });
        }, LOCATION_WAIT_MS);
        navigator.geolocation.getCurrentPosition(function (position) {
            endLookup(function () {
                element(latitudeField).value = position.coords.latitude.toFixed(6);
                element(longitudeField).value = position.coords.longitude.toFixed(6);
                showIdle(panel);
            });
        }, function (error) {
            endLookup(function () {
                showLocationError(error.code === error.PERMISSION_DENIED ? TEXT.locationRefused : TEXT.locationNotFound);
            });
        }, { enableHighAccuracy: true, timeout: 15000, maximumAge: 60000 });
    }

    function useRandomId() {
        const bytes = crypto.getRandomValues(new Uint8Array(8));
        element('am-id').value = Array.from(bytes, function (byte) { return pad(byte.toString(16)); }).join('').toUpperCase();
        showIdle(panels.audiomoth);
    }

    /* Tabs */

    const tabs = Array.from(document.querySelectorAll('[role="tab"]'));

    function selectTab(name, moveFocus) {
        if (run) stopRun();
        tabs.forEach(function (tab) {
            const selected = tab.dataset.tab === name;
            tab.setAttribute('aria-selected', String(selected));
            tab.tabIndex = selected ? 0 : -1;
            element('panel-' + tab.dataset.tab).hidden = !selected;
            if (selected && moveFocus) tab.focus();
        });
        saved.tab = name;
        save();
        try {
            history.replaceState(null, '', '#' + name);
        } catch (error) {
            /* Some embedded views refuse this. The tab still changes. */
        }
    }

    function stopRun() {
        const thisRun = run;
        finish(thisRun, function () {
            /* Only a tab change can stop a chirp, and the PippyG tab then starts over. */
            if (thisRun.panel === panels.pippyg) showIdle(panels.pippyg);
            else showStatus(thisRun.panel, thisRun.count === 0 ? TEXT.stoppedEarly : 'Stopped after ' + chimes(thisRun.count) + '.');
        });
    }

    tabs.forEach(function (tab, index) {
        tab.addEventListener('click', function () {
            /* Tapping the selected tab must not stop a run. */
            if (tab.getAttribute('aria-selected') !== 'true') selectTab(tab.dataset.tab, false);
        });
        tab.addEventListener('keydown', function (event) {
            const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
            if (step === 0) return;
            event.preventDefault();
            selectTab(tabs[(index + step + tabs.length) % tabs.length].dataset.tab, true);
        });
    });

    /* Start-up */

    setUpPippygFields();
    element('am-clock-set').checked = Boolean(saved.audiomothClockSet);

    Object.values(panels).forEach(function (panel) {
        panel.button.textContent = panel.sendLabel();
        showIdle(panel);
        panel.button.addEventListener('click', function () {
            if (run) stopRun();
            else panel.send();
            /* The button may be stuck above the status line, far down the page. */
            panel.status.scrollIntoView({ block: 'nearest' });
        });
    });

    element('am-clock-set').addEventListener('change', function () {
        saved.audiomothClockSet = element('am-clock-set').checked;
        save();
        if (!run) panels.audiomoth.button.textContent = panels.audiomoth.sendLabel();
    });
    ['am-lat', 'am-lon', 'am-id'].forEach(function (id) {
        element(id).addEventListener('input', function () {
            if (!run) showIdle(panels.audiomoth);
        });
    });
    element('am-locate').addEventListener('click', function () { useLocation(panels.audiomoth, element('am-locate'), 'am-lat', 'am-lon'); });
    element('pg-locate').addEventListener('click', function () { useLocation(panels.pippyg, element('pg-locate'), 'pg-lat', 'pg-lon'); });
    element('am-random').addEventListener('click', useRandomId);

    showClock();
    const isPanel = function (name) { return Object.prototype.hasOwnProperty.call(panels, name); };
    const linked = location.hash.slice(1);
    selectTab(isPanel(linked) ? linked : isPanel(saved.tab) ? saved.tab : 'x8', false);
}());
