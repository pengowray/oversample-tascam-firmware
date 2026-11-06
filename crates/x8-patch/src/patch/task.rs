use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::BatApp;

use super::audio::{ANALYSIS_OFF_ABOVE, process_input};
use super::itcm::itcm_still_ok;
use super::keys::{apply_actions, apply_touches};
use super::screen::{draw_frame, hide_for_popup, render, show_frame};
use super::state::{
    ACTIONS_READ, ACTIONS_WRITE, APP, BACKLOG_DROPPED, BAT_ACTIVE, BLOCKS, DRAW_BLOCKS,
    DRAW_PERMILLE, DSP_BLOCKS, DSP_PERMILLE, DSP_STACK, DSP_STACK_BYTES, IN_READ, IN_WRITE,
    LAST_RENDER, LOAD_SECOND, LOAD_WINDOW_START, OUT_READ, OUT_WRITE, SESSION, SESSION_SEEN, STACK, Stack, TASK,
    TOUCH_DOWN, TOUCH_READ, TOUCH_WRITE,
};
use super::{blocks_per_second, read_byte, stock, system_state};

/// Frames processed per call at most when processing in task 1 (only if the
/// DSP task could not be created), so one call cannot hold task 1 for long.
const MAX_FRAMES_PER_CALL: u32 = 24_576;

const RENDERS_PER_SECOND: u32 = 10;
/// While the recorder is recording: the stock interrupt and the SD card writes
/// then leave the app the least time. Drawing took 21% of wall time at 10 a
/// second (B231).
const RENDERS_PER_SECOND_RECORDING: u32 = 5;
const TRANSPORT_RECORDING: u8 = 3;
/// The DSP task stops a pass after this much wall time (30 ms) even with input
/// waiting, then draws and sleeps, so a mode that cannot keep up cannot starve
/// Qt (which also handles powering off).
const PASS_BLOCKS: u32 = 1_440;
/// The pass between drawing a frame and copying it to the display stops after
/// this much wall time (10 ms), to keep the time before the task next sleeps
/// close to what it was with no such pass. Input left over waits for the next
/// pass. About 2,500 frames are waiting after the drawing, which a task with
/// time to spare clears in under 10 ms.
const CATCH_UP_BLOCKS: u32 = 480;
/// Input left beyond this many frames (32 ms) after a pass can be dropped, in one
/// jump, so the sound stays current. This is the last resort, for when even the
/// listening mode alone cannot keep up. It is above ANALYSIS_OFF_ABOVE (5,120),
/// so skipping the analysis gets the chance to catch up first, and 2,048 frames
/// (10.7 ms) below the size of the input ring (8,192), so that the task's sleep
/// does not fill the ring: the interrupt would then leave a hole for each block
/// that does not fit, instead of this one jump.
const BACKLOG_FRAMES: u32 = 6_144;
/// Input is dropped only while the two rings together hold more than this many
/// frames (75 ms), and only down to this many.
///
/// While the rings run, the frames they hold stay constant, normally at the 8,192
/// the output ring is primed with. The output ring holds about that many while the
/// task keeps up, so that total is the longest hold-up of the DSP task that the
/// headphones survive. A recording task that holds the DSP task for 32 ms or more
/// ends a pass with over BACKLOG_FRAMES waiting, even though the task can catch up.
/// Dropping input then gains nothing, because the output ring is no fuller. It also
/// lowers the total, and only an output underrun raises it again, so the headphones
/// would run dry sooner at the next hold-up.
///
/// The total rises above 8,192 only after the output has run dry, because input keeps
/// arriving while the output waits to be primed: to 12,288 for a task running at twice
/// real time, and towards 16,384, the size of the output ring, for a task barely
/// keeping up. A task running at more than 1.33 times real time stays under this limit
/// and drops nothing.
///
/// In a block-by-block model of the rings (random hold-ups with means of 4 to 32 ms,
/// task speeds of 1.1 to 2 times real time), this rule gave the same underruns as
/// never dropping, within 1 per minute. Dropping whenever input passed BACKLOG_FRAMES
/// gave 10% more to 3 times as many.
const MOST_HELD_FRAMES: u32 = 14_336;
const SLEEP_MS: u32 = 2;

/// The DSP task: unused ids at start-up (the table at 0x8131dac4 uses 1, 2,
/// 4 to 7, 9 to 12 and 15 to 22), tried in turn. Priority 16 is below task 1
/// (15) and every audio and recording task, equal to the settings save task,
/// and above task 21 (17), which has a 1 MB stack and is probably Qt.
const TASK_IDS: [i32; 5] = [23, 14, 13, 8, 3];
const TASK_PRIORITY: u32 = 16;
const TA_ACT: u32 = 0x02;
const E_OBJ: i32 = -41;

#[repr(C)]
struct CreateTask {
    attributes: u32,
    extended_info: isize,
    entry: extern "C" fn(isize),
    priority: u32,
    stack_size: usize,
    stack: *mut u8,
}

/// Run `work` on the patch's own stack. The RTOS task switch (PendSV at
/// 0x3023c) only saves and restores the stack pointer, so a task may run on
/// any stack.
pub(super) unsafe fn on_patch_stack(work: extern "C" fn()) {
    let top = (STACK.0.get() as usize + core::mem::size_of::<Stack>()) & !7;
    // SAFETY: r4 keeps the task's stack pointer across the call; the work
    // function follows the C ABI, so it may change r0 to r3, r12, lr and
    // s0 to s15 (this FPU has no d16 to d31) and keeps the rest.
    unsafe {
        core::arch::asm!(
            "mov r4, sp",
            "mov sp, {top}",
            "blx {work}",
            "mov sp, r4",
            top = in(reg) top,
            work = in(reg) work,
            out("r4") _,
            out("r0") _, out("r1") _, out("r2") _, out("r3") _, out("r12") _, out("lr") _,
            out("s0") _, out("s1") _, out("s2") _, out("s3") _, out("s4") _, out("s5") _, out("s6") _, out("s7") _,
            out("s8") _, out("s9") _, out("s10") _, out("s11") _, out("s12") _, out("s13") _, out("s14") _, out("s15") _,
        );
    }
}

/// Drop input beyond BACKLOG_FRAMES, while the rings hold more than
/// MOST_HELD_FRAMES, so a mode that cannot keep up plays current sound with gaps
/// rather than ever later sound.
fn drop_backlog(app: &mut BatApp<'static>) {
    let read = IN_READ.load(Relaxed);
    let waiting = IN_WRITE.load(Acquire).wrapping_sub(read);
    let held = waiting.wrapping_add(OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Acquire)));
    if waiting > BACKLOG_FRAMES && held > MOST_HELD_FRAMES {
        let skip = (waiting - BACKLOG_FRAMES).min(held - MOST_HELD_FRAMES);
        IN_READ.store(read.wrapping_add(skip), Release);
        BACKLOG_DROPPED.store(BACKLOG_DROPPED.load(Relaxed).wrapping_add(skip), Relaxed);
        app.input_lost(skip);
    }
}

/// Whether to redraw now, `now` being the block count. Sound comes first: while
/// input is backing up, a redraw waits, because the drawing takes about 13 ms and
/// the input ring would fill. It waits at most half a second, so the recording
/// time and the meters keep moving.
fn render_due(now: u32) -> bool {
    let since = now.wrapping_sub(LAST_RENDER.load(Relaxed));
    let rate = if read_byte(stock::TRANSPORT) == TRANSPORT_RECORDING { RENDERS_PER_SECOND_RECORDING } else { RENDERS_PER_SECOND };
    if since < blocks_per_second() / rate {
        return false;
    }
    let waiting = IN_WRITE.load(Acquire).wrapping_sub(IN_READ.load(Relaxed));
    waiting <= ANALYSIS_OFF_ABOVE || since >= blocks_per_second() / 2
}

/// At the start of each opening of the bat screen: empty the rings, from the
/// consumer's side of the input one and the producer's side of the output one,
/// and drop actions queued before. What the app hears next does not follow on
/// from what it heard before the screen closed.
fn new_session(app: &mut BatApp<'static>) {
    let session = SESSION.load(Acquire);
    if session != SESSION_SEEN.load(Relaxed) {
        SESSION_SEEN.store(session, Relaxed);
        IN_READ.store(IN_WRITE.load(Acquire), Release);
        OUT_WRITE.store(OUT_READ.load(Acquire), Release);
        ACTIONS_READ.store(ACTIONS_WRITE.load(Acquire), Release);
        TOUCH_READ.store(TOUCH_WRITE.load(Acquire), Release);
        TOUCH_DOWN.store(false, Relaxed);
        app.input_lost(0);
    }
}

pub(super) fn task_running() -> bool {
    (TASK.load(Relaxed) as i32) > 0
}

/// The bat app, once `enter_work` has built it. Only one context uses it at a
/// time: the DSP task, or task 1 when there is no DSP task.
pub(super) unsafe fn app() -> &'static mut BatApp<'static> {
    // SAFETY: built by enter_work.
    unsafe { (*APP.0.get()).assume_init_mut() }
}

/// Create the DSP task, trying each free id in turn.
pub(super) fn start_task() {
    // SAFETY: the RTOS's cre_tsk, from task 1.
    let create: unsafe extern "C" fn(i32, *const CreateTask) -> i32 = unsafe { core::mem::transmute(stock::CRE_TSK) };
    let pk = CreateTask {
        attributes: TA_ACT,
        extended_info: 0,
        entry: dsp_task,
        priority: TASK_PRIORITY,
        stack_size: DSP_STACK_BYTES,
        stack: DSP_STACK.0.get().cast(),
    };
    let mut result = E_OBJ;
    for id in TASK_IDS {
        // SAFETY: as above; pk lives until cre_tsk returns, and the stack for
        // as long as the patch.
        result = unsafe { create(id, &pk) };
        if result != E_OBJ {
            if result == 0 {
                result = id;
            }
            break;
        }
    }
    TASK.store(result as u32, Relaxed);
}

fn sleep_ms(ms: u32) {
    // SAFETY: the RTOS's dly_tsk, from the DSP task.
    let delay: unsafe extern "C" fn(u32) -> i32 = unsafe { core::mem::transmute(stock::DLY_TSK) };
    unsafe { delay(ms) };
}

/// The DSP task: runs the app while the bat screen is up, sleeps otherwise.
extern "C" fn dsp_task(_: isize) {
    loop {
        if !BAT_ACTIVE.load(Acquire) {
            sleep_ms(20);
            continue;
        }
        if system_state() == stock::STATE_POPUP {
            hide_for_popup();
            continue;
        }
        if !itcm_still_ok() {
            sleep_ms(20);
            continue;
        }
        // SAFETY: built before BAT_ACTIVE is set; only this task uses it.
        let app = unsafe { app() };
        new_session(app);
        apply_actions(app);
        apply_touches(app);
        let start = BLOCKS.load(Relaxed);
        process_input(app, u32::MAX, PASS_BLOCKS);
        drop_backlog(app);
        let processed = BLOCKS.load(Relaxed);
        DSP_BLOCKS.store(DSP_BLOCKS.load(Relaxed).wrapping_add(processed.wrapping_sub(start)), Relaxed);
        if render_due(processed) {
            LAST_RENDER.store(processed, Relaxed);
            draw_frame();
            let drawn = BLOCKS.load(Relaxed);
            DRAW_BLOCKS.store(DRAW_BLOCKS.load(Relaxed).wrapping_add(drawn.wrapping_sub(processed)), Relaxed);
            // Catch up between the drawing and the copy to the display, so input
            // does not wait for the whole frame. On B231 the two took about 60%
            // and 40% of a frame's time: the input ring held 1,900 frames when
            // the debug lines were drawn, just before the copy, and would hold
            // 3,300 by the end.
            process_input(app, u32::MAX, CATCH_UP_BLOCKS);
            drop_backlog(app);
            let caught_up = BLOCKS.load(Relaxed);
            DSP_BLOCKS.store(DSP_BLOCKS.load(Relaxed).wrapping_add(caught_up.wrapping_sub(drawn)), Relaxed);
            show_frame();
            DRAW_BLOCKS.store(DRAW_BLOCKS.load(Relaxed).wrapping_add(BLOCKS.load(Relaxed).wrapping_sub(caught_up)), Relaxed);
        }
        let now = BLOCKS.load(Relaxed);
        let second = now.wrapping_sub(LOAD_SECOND.load(Relaxed));
        if second >= blocks_per_second() {
            DSP_PERMILLE.store(DSP_BLOCKS.swap(0, Relaxed) * 1000 / second, Relaxed);
            DRAW_PERMILLE.store(DRAW_BLOCKS.swap(0, Relaxed) * 1000 / second, Relaxed);
            LOAD_WINDOW_START.store(LOAD_SECOND.load(Relaxed), Relaxed);
            LOAD_SECOND.store(now, Relaxed);
        }
        // Sleep after every pass, even with input waiting: the audio interrupt
        // adds 4 frames every 21 us, so the ring is never empty, and Qt (task
        // 21, priority 17) still runs the hidden app, REC included.
        sleep_ms(SLEEP_MS);
    }
}

/// Without a DSP task: process in task 1 and redraw now and then.
pub(super) extern "C" fn tick_work() {
    if !BAT_ACTIVE.load(Relaxed) {
        return;
    }
    // SAFETY: built by enter_work; with no DSP task only task 1 uses it.
    let app = unsafe { app() };
    new_session(app);
    apply_actions(app);
    apply_touches(app);
    process_input(app, MAX_FRAMES_PER_CALL, PASS_BLOCKS);
    let now = BLOCKS.load(Relaxed);
    if render_due(now) {
        LAST_RENDER.store(now, Relaxed);
        render();
    }
}
