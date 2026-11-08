use core::ptr;
use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::BatApp;

use super::state::{
    AUDIO_INSTALLED, AUDIO_SLOT_UNEXPECTED, AUDIO_STATE, BAT_ACTIVE, BLOCKS, BLOCKS_AT_SECOND, CYCLE_COUNTER_ON,
    CYCLES_AT_SECOND, CYCLES_PER_S, DEBUG_SHOWN, HEAR_MIX, HEARD_WORDS, HOOK_CYCLES, HOOK_CYCLES_PER_S, HOOK_IN_ITCM,
    IN_DROPPED, IN_DROPPED_SEEN, IN_HOLE_AT, IN_READ, IN_RING, IN_WAITING_MOST, IN_WRITE, NO_INPUT_WORDS, OUT_FADE_IN,
    OUT_LAST, OUT_READ, OUT_WRITE, PLAYBACK, PRIMED, SOURCE_PEAK, STOCK_CYCLES, STOCK_CYCLES_PER_S, UNDERRUNS,
    WORD_PEAKS,
};
use super::{Handler, blocks_per_second, rate_class, spare, stock};

/// Output frames buffered before the headphones start playing the bat app's
/// output (43 ms at 192 kHz), so the DSP task's passes do not run dry.
const PRIME_FRAMES: u32 = 8_192;
/// When the output ring runs dry there are no frames left to fade, so each ear
/// falls away from its last sample by this factor per frame: 60 dB in 95 frames
/// (0.5 ms at 192 kHz). A step to zero would be a click on top of the gap.
const DRY_DECAY: f32 = 0.93;
/// When the output starts again it fades in over this many frames (0.33 ms).
const RESUME_FADE_FRAMES: u32 = 64;

const CHUNK: usize = 1_024;

/// Input waiting above this many frames (26.7 ms) makes the app skip its
/// analysis, which is most of its work, until the waiting input falls below
/// ANALYSIS_ON_BELOW. The headphones then stay continuous, at the cost of grey
/// columns on the waterfall.
///
/// While recording, the drawing part of a frame takes about 13 ms of wall time (an
/// estimate from B231's figures; see `dsp_task`), in which about 2,500 frames
/// arrive, so a task that is keeping up sees about that many waiting after each
/// frame's drawing. This level leaves room for about another 14 ms on top, for the
/// recording tasks, which run first, before any gap appears on the waterfall. It
/// also leaves 3,072 frames (16 ms) before the input ring is full, and at the usual
/// 8,192 frames in the two rings together, 16 ms before the headphones run out.
pub(super) const ANALYSIS_OFF_ABOVE: u32 = 5_120;
/// Far enough below the level above that one episode is not followed at once by
/// another. Each restart of the analysis costs a window of 512 samples with no
/// column. Going from 5,120 to 2,048 misses about 16 ms of sound, or two display
/// columns.
const ANALYSIS_ON_BELOW: u32 = 2_048;

/// Gain on the app's output before the stock headphone gain (+18 dB), with a
/// ceiling of -6 dBFS: bat calls are often 40 to 60 dB below full scale.
pub(super) const OUTPUT_GAIN: f32 = 8.0;
const OUTPUT_CEILING: f32 = 0.5;

const SCALE: f32 = 1.0 / 2_147_483_648.0;

/// Arm's debug registers for the cycle counter (privileged access only, so
/// used from the audio interrupt).
const DEMCR: *mut u32 = 0xe000_edfc as *mut u32;
const DWT_CTRL: *mut u32 = 0xe000_1000 as *mut u32;
const DWT_CYCCNT: *const u32 = 0xe000_1004 as *const u32;
const DWT_LAR: *mut u32 = 0xe000_1fb0 as *mut u32;

#[inline(always)]
fn cycles() -> u32 {
    // SAFETY: reading the cycle counter, from the interrupt.
    unsafe { ptr::read_volatile(DWT_CYCCNT) }
}

pub(super) fn install_audio_hook() {
    // SAFETY: the eDMA handle's callback word, set once by the stock setup.
    let current = unsafe { ptr::read_volatile(stock::AUDIO_CALLBACK_SLOT) };
    if current == 0 {
        // The audio is not set up yet: try again on the next call.
        return;
    }
    if current != stock::AUDIO_CALLBACK {
        AUDIO_STATE.store(AUDIO_SLOT_UNEXPECTED, Relaxed);
        return;
    }
    let hook: Handler = audio_hook;
    // SAFETY: a single aligned word store, which the interrupt reads whole.
    unsafe { ptr::write_volatile(stock::AUDIO_CALLBACK_SLOT, hook as usize) };
    AUDIO_STATE.store(AUDIO_INSTALLED, Relaxed);
}

/// Run the hook from its copy in ITCM, or from SDRAM. The ITCM copy is only valid once
/// `load_itcm` has run and while the ITCM check passes, so the hook installed at start-up
/// is the SDRAM one, and the bat screen switches between them.
pub(super) fn use_itcm_hook(itcm: bool) {
    if AUDIO_STATE.load(Relaxed) != AUDIO_INSTALLED {
        return;
    }
    let hook: Handler = if itcm { audio_hook_itcm } else { audio_hook };
    // SAFETY: as in install_audio_hook. Both hooks do the same, so the interrupt may
    // run either one.
    unsafe { ptr::write_volatile(stock::AUDIO_CALLBACK_SLOT, hook as usize) };
}

/// Process up to `budget` frames from the input ring, stopping after
/// `time_blocks` audio blocks of wall time, and queue the output. Returns the
/// frames processed. Not inlined, so x8patch.ld can put it in ITCM.
#[inline(never)]
pub(super) fn process_input(app: &mut BatApp<'static>, budget: u32, time_blocks: u32) -> u32 {
    let start = BLOCKS.load(Relaxed);
    let mut first = [0.0f32; CHUNK];
    let mut second = [0.0f32; CHUNK];
    let mut left = [0.0f32; CHUNK];
    let mut right = [0.0f32; CHUNK];
    let ring = IN_RING.0.get().cast::<[i32; 2]>();
    let out = spare::OUT_RING as *mut f32;
    let mut done = 0;
    while done < budget && BLOCKS.load(Relaxed).wrapping_sub(start) < time_blocks {
        let read = IN_READ.load(Relaxed);
        let available = IN_WRITE.load(Acquire).wrapping_sub(read);
        if available > IN_WAITING_MOST.load(Relaxed) {
            IN_WAITING_MOST.store(available, Relaxed);
        }
        if available > ANALYSIS_OFF_ABOVE {
            app.set_skip_analysis(true);
        } else if available < ANALYSIS_ON_BELOW {
            app.set_skip_analysis(false);
        }
        let mut n = available.min(CHUNK as u32).min(budget - done) as usize;
        // Frames that did not fit left a jump in the ring. Read up to it, then
        // tell the app. Until then, no analysis: only the latest jump's place is
        // known, and an earlier one may lie before it.
        let seen = IN_DROPPED_SEEN.load(Relaxed);
        let lost = IN_DROPPED.load(Acquire).wrapping_sub(seen);
        if lost > 0 {
            let to_jump = IN_HOLE_AT.load(Relaxed).wrapping_sub(read) as i32;
            if to_jump > 0 {
                app.set_skip_analysis(true);
                n = n.min(to_jump as usize);
            } else {
                app.input_lost(lost);
                IN_DROPPED_SEEN.store(seen.wrapping_add(lost), Relaxed);
            }
        }
        // Leave input in the input ring while the output ring has no room for it. If the
        // input ring then overflows, or `drop_backlog` drops some input, the app is told
        // about the jump and fades it. The app could not fade frames thrown away after it
        // had processed them. Only this task moves OUT_WRITE, and the interrupt only makes
        // more room, so these frames still fit when the app has processed them.
        let write = OUT_WRITE.load(Relaxed);
        let used = write.wrapping_sub(OUT_READ.load(Acquire));
        n = n.min(spare::OUT_FRAMES.saturating_sub(used) as usize);
        if n == 0 {
            break;
        }
        for i in 0..n {
            let at = read.wrapping_add(i as u32) % spare::IN_FRAMES;
            // SAFETY: inside the input ring.
            let [a, b] = unsafe { ptr::read_volatile(ring.add(at as usize)) };
            first[i] = a as f32 * SCALE;
            second[i] = b as f32 * SCALE;
        }
        IN_READ.store(read.wrapping_add(n as u32), Release);
        if app.test_calls_on() {
            app.add_test_calls(&mut first[..n], &mut second[..n]);
        }
        app.process(&first[..n], &second[..n], &mut left[..n], &mut right[..n]);
        let mut at = (write % spare::OUT_FRAMES) as usize;
        for (&l, &r) in left[..n].iter().zip(&right[..n]) {
            // SAFETY: `at` is below OUT_FRAMES, so both samples are inside the output ring.
            unsafe {
                ptr::write_volatile(out.add(2 * at), l);
                ptr::write_volatile(out.add(2 * at + 1), r);
            }
            at += 1;
            if at == spare::OUT_FRAMES as usize {
                at = 0;
            }
        }
        // Both samples of every frame are written before the interrupt can read them.
        OUT_WRITE.store(write.wrapping_add(n as u32), Release);
        done += n as u32;
    }
    done
}

/// The audio DMA callback, run from SDRAM: see `hook_body`.
unsafe extern "C" fn audio_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as for hook_body.
    unsafe { hook_body::<false>(a, b, c, d) }
}

/// The same, which x8patch.ld puts in ITCM: from SDRAM the hook cost about 900 cycles a
/// call on B239, 48,000 times a second.
unsafe extern "C" fn audio_hook_itcm(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as for hook_body.
    unsafe { hook_body::<true>(a, b, c, d) }
}

/// Runs the stock callback, then, with the bat screen up at 192 kHz, passes the inputs
/// chosen by `selected_inputs` to the app and plays the app's output.
/// `ITCM` says which copy is running. Storing it also keeps the two copies different, so
/// the compiler does not merge them into one function.
#[inline(always)]
unsafe fn hook_body<const ITCM: bool>(a: usize, b: usize, c: usize, d: usize) -> usize {
    HOOK_IN_ITCM.store(ITCM, Relaxed);
    if !CYCLE_COUNTER_ON.load(Relaxed) {
        // SAFETY: turning on the Cortex-M7 cycle counter (trace enable, then
        // the DWT's lock and counter enable); this interrupt is privileged.
        unsafe {
            ptr::write_volatile(DEMCR, ptr::read_volatile(DEMCR) | 1 << 24);
            ptr::write_volatile(DWT_LAR, 0xc5ac_ce55);
            ptr::write_volatile(DWT_CTRL, ptr::read_volatile(DWT_CTRL) | 1);
        }
        CYCLE_COUNTER_ON.store(true, Relaxed);
    }
    let t0 = cycles();
    // SAFETY: read before the stock callback moves the ring on.
    let index = unsafe { ptr::read_volatile(stock::AUDIO_INDEX) } as usize % 3;
    // The stock routine scales frame 0's output by the gain before it
    // updates the gain (once per block), and frames 1 to 3 by the new gain.
    // SAFETY: the stock output gain, an f32 in ITCM.
    let frame0_gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) };
    // SAFETY: the stock callback, with its own arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::AUDIO_CALLBACK) };
    let result = unsafe { stock_callback(a, b, c, d) };
    let t1 = cycles();
    STOCK_CYCLES.store(STOCK_CYCLES.load(Relaxed).wrapping_add(t1.wrapping_sub(t0)), Relaxed);
    let blocks = BLOCKS.load(Relaxed).wrapping_add(1);
    BLOCKS.store(blocks, Relaxed);
    if blocks.wrapping_sub(BLOCKS_AT_SECOND.load(Relaxed)) >= blocks_per_second() {
        BLOCKS_AT_SECOND.store(blocks, Relaxed);
        CYCLES_PER_S.store(t1.wrapping_sub(CYCLES_AT_SECOND.load(Relaxed)), Relaxed);
        CYCLES_AT_SECOND.store(t1, Relaxed);
        STOCK_CYCLES_PER_S.store(STOCK_CYCLES.swap(0, Relaxed), Relaxed);
        HOOK_CYCLES_PER_S.store(HOOK_CYCLES.swap(0, Relaxed), Relaxed);
    }
    if !BAT_ACTIVE.load(Acquire) || rate_class() != 2 {
        return result;
    }
    let input = (stock::INPUT_BLOCKS + index * 0x180) as *const i32;
    let output = (stock::OUTPUT_BLOCKS + (index + 2) % 3 * 0x40) as *mut i32;

    if DEBUG_SHOWN.load(Relaxed) {
        for (w, peak) in WORD_PEAKS.iter().enumerate() {
            let mut p = peak.load(Relaxed);
            for k in 0..4 {
                // SAFETY: a word of this frame in the input block.
                p = p.max(unsafe { ptr::read_volatile(input.add(12 * k + w)) }.unsigned_abs());
            }
            peak.store(p, Relaxed);
        }
    }

    // SAFETY: the stock output gain, an f32 in ITCM.
    let stock_gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) };
    let playback = PLAYBACK.load(Relaxed);
    // The block's 4 frames into the input ring: the two inputs chosen by
    // `selected_inputs`, or MANUAL's mix, which is the stock output (left,
    // right) that the stock callback has just written as clamp(mix) x
    // (2^31 - 1) x the output gain. Until a file plays, that mix is the live
    // inputs, so BAT PLAYBACK hears silence then.
    let from_output = playback || HEAR_MIX.load(Relaxed);
    // SAFETY: a byte in the stock DSP state.
    let output_usable = !playback || unsafe { ptr::read_volatile(stock::DSP_PLAYING) } == 1;
    let words = HEARD_WORDS.load(Relaxed);
    let (first_word, second_word) = ((words & 0xff) as usize, (words >> 8 & 0xff) as usize);
    let no_input = words == NO_INPUT_WORDS;
    let write = IN_WRITE.load(Relaxed);
    if spare::IN_FRAMES - write.wrapping_sub(IN_READ.load(Acquire)) >= 4 {
        let ring = IN_RING.0.get().cast::<[i32; 2]>();
        for k in 0..4 {
            let gain = if k == 0 { frame0_gain } else { stock_gain };
            // SAFETY: words of this frame in the input and output blocks, and
            // the ring.
            unsafe {
                let pair = if !from_output && !no_input {
                    [
                        ptr::read_volatile(input.add(12 * k + first_word)),
                        ptr::read_volatile(input.add(12 * k + second_word)),
                    ]
                } else if from_output && output_usable && gain != 0.0 {
                    let unscale = |word: i32| (word as f32 / gain) as i32;
                    [unscale(ptr::read_volatile(output.add(2 * k))), unscale(ptr::read_volatile(output.add(2 * k + 1)))]
                } else {
                    [0, 0]
                };
                if from_output {
                    let peak = pair[0].unsigned_abs().max(pair[1].unsigned_abs());
                    SOURCE_PEAK.store(SOURCE_PEAK.load(Relaxed).max(peak), Relaxed);
                }
                ptr::write_volatile(ring.add((write.wrapping_add(k as u32) % spare::IN_FRAMES) as usize), pair);
            }
        }
        IN_WRITE.store(write.wrapping_add(4), Release);
    } else {
        IN_HOLE_AT.store(write, Relaxed);
        IN_DROPPED.store(IN_DROPPED.load(Relaxed).wrapping_add(4), Release);
    }

    // The app's output, once enough is buffered, at the stock headphone gain.
    let read = OUT_READ.load(Relaxed);
    let available = OUT_WRITE.load(Acquire).wrapping_sub(read);
    let mut primed = PRIMED.load(Relaxed);
    let mut fade_in = OUT_FADE_IN.load(Relaxed);
    if !primed && available >= PRIME_FRAMES {
        primed = true;
        fade_in = RESUME_FADE_FRAMES;
    } else if primed && available < 4 {
        primed = false;
        UNDERRUNS.store(UNDERRUNS.load(Relaxed).wrapping_add(1), Relaxed);
    }
    PRIMED.store(primed, Relaxed);
    let gain = stock_gain * 2_147_483_647.0;
    let ring = spare::OUT_RING as *const f32;
    let mut left = f32::from_bits(OUT_LAST[0].load(Relaxed));
    let mut right = f32::from_bits(OUT_LAST[1].load(Relaxed));
    // One modulo per block. OUT_READ is always a multiple of 4: this interrupt moves it in
    // steps of 4, and `enter_work` rounds it down. OUT_FRAMES is a multiple of 4 too, so the
    // block's 4 frames never cross the end of the ring.
    let at = (read % spare::OUT_FRAMES) as usize;
    for k in 0..4 {
        if primed {
            // SAFETY: `at + k` is below OUT_FRAMES, so both samples are inside the output ring.
            (left, right) =
                unsafe { (ptr::read_volatile(ring.add(2 * (at + k))), ptr::read_volatile(ring.add(2 * (at + k) + 1))) };
            if fade_in > 0 {
                fade_in -= 1;
                let share = (RESUME_FADE_FRAMES - fade_in) as f32 / RESUME_FADE_FRAMES as f32;
                left *= share;
                right *= share;
            }
        } else {
            left *= DRY_DECAY;
            right *= DRY_DECAY;
        }
        let word = |y: f32| ((y * OUTPUT_GAIN).clamp(-OUTPUT_CEILING, OUTPUT_CEILING) * gain) as i32;
        // SAFETY: the words of this frame in the output block.
        unsafe {
            ptr::write_volatile(output.add(2 * k), word(left));
            ptr::write_volatile(output.add(2 * k + 1), word(right));
        }
    }
    OUT_LAST[0].store(left.to_bits(), Relaxed);
    OUT_LAST[1].store(right.to_bits(), Relaxed);
    OUT_FADE_IN.store(fade_in, Relaxed);
    if primed {
        OUT_READ.store(read.wrapping_add(4), Release);
    }
    HOOK_CYCLES.store(HOOK_CYCLES.load(Relaxed).wrapping_add(cycles().wrapping_sub(t1)), Relaxed);
    result
}
