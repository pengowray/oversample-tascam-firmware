// X8 bat app emulator: audio in and out, the screen canvas, and the controls.
//
// The bat app itself (crates/bat-app) runs in WebAssembly. Audio runs through
// a ScriptProcessorNode on the main thread: deprecated, but it lets the page
// call the WebAssembly synchronously for each 21 ms block without a worklet.

import init, { Emulator } from './pkg/bat_app_web.js';

const RATE = 192000; // The X8's highest recording rate; files are resampled to it.
const BLOCK = 4096;

const wasm = await init();
const emu = new Emulator(RATE);
const W = Emulator.width();
const H = Emulator.height();

const $ = (id) => document.getElementById(id);
const canvas = $('screen');
const g = canvas.getContext('2d');

// ---- Screen ---------------------------------------------------------------

// Largest scale, in half steps, at which the whole device fits the window.
// The page header and the device frame's padding take about 130 px of height;
// the controls panel and the buttons beside the screen take about 640 px of width.
function fitScreen() {
  const narrow = window.innerWidth <= 760;
  const availH = narrow ? Infinity : window.innerHeight - 130;
  const availW = narrow ? window.innerWidth - 64 : window.innerWidth - 640;
  let scale = Math.floor(Math.min(availH / H, availW / W) * 2) / 2;
  if (scale < 1) scale = Math.min(availH / H, availW / W); // small phones: fit exactly
  canvas.style.width = `${W * scale}px`;
  canvas.style.height = `${H * scale}px`;
}
window.addEventListener('resize', fitScreen);
fitScreen();

function frame() {
  const now = new Date();
  emu.set_clock(now.getHours() * 3600 + now.getMinutes() * 60 + now.getSeconds());
  emu.render();
  const view = new Uint8ClampedArray(wasm.memory.buffer, emu.frame_ptr(), W * H * 4);
  g.putImageData(new ImageData(view, W, H), 0, 0);
  updateRecButton();
  updateTransport();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

function screenPoint(ev) {
  const r = canvas.getBoundingClientRect();
  return [Math.floor((ev.clientX - r.left) * W / r.width), Math.floor((ev.clientY - r.top) * H / r.height)];
}
// Show a pointer where a tap does something (mode buttons; the waterfall in HET and COMB).
canvas.addEventListener('pointermove', (ev) => {
  const [x, y] = screenPoint(ev);
  canvas.style.cursor = emu.is_tappable(x, y) ? 'pointer' : 'default';
});
canvas.addEventListener('pointerdown', (ev) => {
  resumeAudio();
  const [x, y] = screenPoint(ev);
  emu.touch(x, y);
});
// Scrolling over the screen turns the wheel: up is clockwise.
let wheelAccum = 0;
canvas.addEventListener('wheel', (ev) => {
  ev.preventDefault();
  wheelAccum += ev.deltaY;
  const steps = Math.trunc(wheelAccum / 50);
  if (steps !== 0) {
    emu.wheel(-steps);
    wheelAccum -= steps * 50;
  }
}, { passive: false });

// ---- Audio ----------------------------------------------------------------

const ctx = new AudioContext({ sampleRate: RATE });
const node = ctx.createScriptProcessor(BLOCK, 2, 2);
const gain = ctx.createGain();
// A limiter after the volume, so a loud file at a high volume cannot blast.
const limiter = new DynamicsCompressorNode(ctx, { threshold: -6, knee: 0, ratio: 20, attack: 0.001, release: 0.1 });
const silence = ctx.createConstantSource();
silence.offset.value = 0;
silence.connect(node);
silence.start();
node.connect(gain).connect(limiter).connect(ctx.destination);

let source = 'none';      // 'none', 'file' or 'mic'
let file = null;          // { name, ch3, ch4, length }
let pos = 0;
let playing = false;
let micStream = null;
let micNode = null;
let load = 0;
const in3 = new Float32Array(BLOCK);
const in4 = new Float32Array(BLOCK);

node.onaudioprocess = (e) => {
  const n = e.outputBuffer.length;
  const outL = e.outputBuffer.getChannelData(0);
  const outR = e.outputBuffer.getChannelData(1);
  let a = in3.subarray(0, n);
  let b = in4.subarray(0, n);
  if (source === 'mic') {
    a = e.inputBuffer.getChannelData(0);
    b = e.inputBuffer.numberOfChannels > 1 ? e.inputBuffer.getChannelData(1) : a;
  } else if (source === 'file' && playing) {
    for (let i = 0; i < n; i++) {
      if (pos >= file.length) {
        if ($('loop').checked) pos = 0; else { playing = false; a.fill(0, i); b.fill(0, i); break; }
      }
      a[i] = file.ch3[pos];
      b[i] = file.ch4[pos];
      pos++;
    }
  } else {
    a.fill(0);
    b.fill(0);
  }
  const t0 = performance.now();
  emu.process(a, b, outL, outR);
  const used = (performance.now() - t0) / (n / RATE * 1000);
  load = load * 0.9 + used * 0.1;
};

function resumeAudio() {
  if (ctx.state !== 'running') ctx.resume();
}
// Browsers start audio paused until the page gets a click or a key press.
ctx.onstatechange = () => { $('audio-paused').hidden = ctx.state === 'running'; };
ctx.onstatechange();
document.addEventListener('pointerdown', resumeAudio);
document.addEventListener('keydown', resumeAudio);

function setVolume() {
  const db = Number($('volume').value);
  gain.gain.value = 10 ** (db / 20);
  $('volume-value').textContent = `+${db} dB`;
}
$('volume').addEventListener('input', setVolume);
setVolume();

setInterval(() => {
  if (load > 0) $('load').textContent = `This computer runs the bat app ${Math.round(1 / load)} times faster than real time.`;
}, 500);

// ---- Inputs -----------------------------------------------------------------

function fmt(seconds) {
  const s = Math.floor(seconds);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}
// Length of a file: seconds with a tenth below a minute, else m:ss.
function length(seconds) {
  return seconds < 60 ? `${seconds.toFixed(1)} s` : fmt(seconds);
}

async function openFile(name, arrayBuffer) {
  resumeAudio();
  $('input-status').textContent = `Loading ${name}…`;
  let buffer;
  try {
    buffer = await ctx.decodeAudioData(arrayBuffer);
  } catch (err) {
    $('input-status').textContent = `Couldn't read ${name}: ${err.message || err}`;
    return;
  }
  stopMic();
  const ch3 = buffer.getChannelData(0);
  const ch4 = buffer.numberOfChannels > 1 ? buffer.getChannelData(1) : ch3;
  file = { name, ch3, ch4, length: buffer.length };
  // Set the volume so the file's loudest sample comes out near -12 dBFS:
  // 32-bit float bat recordings are often very quiet.
  let peak = 0;
  for (const ch of [ch3, ch4]) for (let i = 0; i < ch.length; i++) peak = Math.max(peak, Math.abs(ch[i]));
  if (peak > 0) {
    $('volume').value = String(Math.max(0, Math.min(60, Math.round(-12 - 20 * Math.log10(peak)))));
    setVolume();
  }
  pos = 0;
  source = 'file';
  playing = true;
  const channels = buffer.numberOfChannels > 1 ? 'stereo' : 'mono, played as both inputs';
  $('input-status').textContent = `${name}, ${length(buffer.duration)}, ${channels}.`;
  $('transport').hidden = false;
  // Once a file is open, opening another is no longer the main action.
  $('file').parentElement.classList.remove('primary');
}

$('file').addEventListener('change', async (ev) => {
  const f = ev.target.files[0];
  if (f) openFile(f.name, await f.arrayBuffer());
  ev.target.value = '';
});

document.addEventListener('dragover', (ev) => { ev.preventDefault(); document.body.classList.add('dropping'); });
document.addEventListener('dragleave', (ev) => { if (!ev.relatedTarget) document.body.classList.remove('dropping'); });
document.addEventListener('drop', async (ev) => {
  ev.preventDefault();
  document.body.classList.remove('dropping');
  const f = ev.dataTransfer.files[0];
  if (f) openFile(f.name, await f.arrayBuffer());
});

// ?mode=het|comb|fd|te|pitch starts in that listening mode.
const params = new URLSearchParams(location.search);
const modeIndex = ['het', 'comb', 'fd', 'te', 'pitch'].indexOf((params.get('mode') || '').toLowerCase());
if (modeIndex >= 0) emu.set_mode(modeIndex);

// ?wav=path loads a file from this server, for testing.
const wavParam = params.get('wav');
if (wavParam) {
  fetch(wavParam)
    .then((r) => { if (!r.ok) throw new Error(`${r.status} ${r.statusText}`); return r.arrayBuffer(); })
    .then((buf) => openFile(wavParam.split('/').pop(), buf))
    .catch((err) => { $('input-status').textContent = `Couldn't load ${wavParam}: ${err.message}`; });
}

async function startMic() {
  resumeAudio();
  try {
    micStream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 2, sampleRate: RATE },
    });
  } catch (err) {
    $('input-status').textContent = `Couldn't open the microphone: ${err.message || err}`;
    return;
  }
  micNode = ctx.createMediaStreamSource(micStream);
  micNode.connect(node);
  source = 'mic';
  playing = false;
  $('transport').hidden = true;
  const rate = micStream.getAudioTracks()[0].getSettings().sampleRate;
  const label = micStream.getAudioTracks()[0].label || 'Microphone';
  $('input-status').textContent = rate && rate < 192000
    ? `${label} at ${rate / 1000} kHz: it hears nothing above ${rate / 2000} kHz, so bat calls will not show.`
    : `${label}${rate ? ` at ${rate / 1000} kHz` : ''}.`;
  $('mic').textContent = 'Stop microphone';
}

function stopMic() {
  if (micNode) micNode.disconnect();
  if (micStream) micStream.getTracks().forEach((t) => t.stop());
  micNode = null;
  micStream = null;
  $('mic').textContent = 'Use microphone';
  if (source === 'mic') {
    source = file ? 'file' : 'none';
    $('transport').hidden = !file;
    $('input-status').textContent = file ? `${file.name}, ${length(file.length / RATE)}.` : 'No input yet. Open a file, drop one on the page, or use the microphone.';
  }
}

$('mic').addEventListener('click', () => (micStream ? stopMic() : startMic()));

// ---- Transport ---------------------------------------------------------------

function togglePlay() {
  resumeAudio();
  if (!file) return;
  if (source !== 'file') { stopMic(); source = 'file'; }
  if (!playing && pos >= file.length) pos = 0;
  playing = !playing;
}
$('play').addEventListener('click', togglePlay);

let seeking = false;
$('seek').addEventListener('input', () => {
  seeking = true;
  if (file) pos = Math.floor(Number($('seek').value) / 1000 * file.length);
});
$('seek').addEventListener('change', () => { seeking = false; });

function updateTransport() {
  if (!file) return;
  $('play').textContent = playing ? 'Pause' : 'Play';
  if (!seeking) $('seek').value = String(Math.round(pos / file.length * 1000));
  $('time').textContent = `${fmt(pos / RATE)} / ${fmt(file.length / RATE)}`;
}

// ---- Buttons and keys --------------------------------------------------------

function pressButton(name) {
  resumeAudio();
  if (name === 'play') togglePlay();
  else emu.button(name);
}

function updateRecButton() {
  document.querySelector('[data-button="rec"]').setAttribute('aria-pressed', String(emu.is_recording()));
}

document.querySelectorAll('[data-button]').forEach((b) => b.addEventListener('click', () => pressButton(b.dataset.button)));
document.querySelectorAll('[data-wheel]').forEach((b) => b.addEventListener('click', () => { resumeAudio(); emu.wheel(Number(b.dataset.wheel)); }));

document.addEventListener('keydown', (ev) => {
  if (ev.target.closest('input, textarea, select') && ev.target.type !== 'radio' && ev.target.type !== 'checkbox') return;
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
  const key = ev.key.toLowerCase();
  const actions = {
    ' ': () => pressButton('play'),
    r: () => pressButton('rec'),
    m: () => pressButton('mark'),
    h: () => pressButton('home'),
    arrowleft: () => emu.wheel(-1),
    arrowdown: () => emu.wheel(-1),
    arrowright: () => emu.wheel(1),
    arrowup: () => emu.wheel(1),
  };
  if (key >= '1' && key <= '5') actions[key] = () => emu.set_mode(Number(key) - 1);
  if (actions[key]) {
    ev.preventDefault();
    resumeAudio();
    actions[key]();
  }
});
