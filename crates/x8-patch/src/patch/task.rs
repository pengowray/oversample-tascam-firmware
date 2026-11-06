use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::BatApp;

use super::audio::process_input;
use super::itcm::itcm_still_ok;
use super::keys::{apply_actions, apply_touches};
use super::screen::{hide_for_popup, render};
use super::state::{
    ACTIONS_READ, ACTIONS_WRITE, APP, BACKLOG_DROPPED, BAT_ACTIVE, BLOCKS, DRAW_BLOCKS,
    DRAW_PERMILLE, DSP_BLOCKS, DSP_PERMILLE, DSP_STACK, DSP_STACK_BYTES, IN_READ, IN_WRITE,
    LAST_RENDER, LOAD_SECOND, LOAD_WINDOW_START, OUT_READ, OUT_WRITE, SESSION, SESSION_SEEN, STACK, Stack, TASK,
    TOUCH_DOWN, TOUCH_READ, TOUCH_WRITE,
};
use super::{blocks_per_second, stock, system_state};

/// Frames processed per call at most when processing in task 1 (only if the
/// DSP task could not be created), so one call cannot hold task 1 for long.
const MAX_FRAMES_PER_CALL: u32 = 24_576;

const RENDERS_PER_SECOND: u32 = 10;
/// The DSP task stops a pass after this much wall time (30 ms) even with input
/// waiting, then draws and sleeps, so a mode that cannot keep up cannot starve
/// Qt (which also handles powering off). Input left beyond BACKLOG_FRAMES
/// after a pass is dropped, so the sound stays current.
const PASS_BLOCKS: u32 = 1_440;
const BACKLOG_FRAMES: u32 = 4_096;
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

/// Drop input beyond BACKLOG_FRAMES, so a mode that cannot keep up plays
/// current sound with gaps rather than ever later sound.
fn drop_backlog() {
    let read = IN_READ.load(Relaxed);
    let waiting = IN_WRITE.load(Acquire).wrapping_sub(read);
    if waiting > BACKLOG_FRAMES {
        let skip = waiting - BACKLOG_FRAMES;
        IN_READ.store(read.wrapping_add(skip), Release);
        BACKLOG_DROPPED.store(BACKLOG_DROPPED.load(Relaxed).wrapping_add(skip), Relaxed);
    }
}

/// At the start of each opening of the bat screen: empty the rings, from the
/// consumer's side of the input one and the producer's side of the output one,
/// and drop actions queued before.
fn new_session() {
    let session = SESSION.load(Acquire);
    if session != SESSION_SEEN.load(Relaxed) {
        SESSION_SEEN.store(session, Relaxed);
        IN_READ.store(IN_WRITE.load(Acquire), Release);
        OUT_WRITE.store(OUT_READ.load(Acquire), Release);
        ACTIONS_READ.store(ACTIONS_WRITE.load(Acquire), Release);
        TOUCH_READ.store(TOUCH_WRITE.load(Acquire), Release);
        TOUCH_DOWN.store(false, Relaxed);
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
        new_session();
        apply_actions(app);
        apply_touches(app);
        let start = BLOCKS.load(Relaxed);
        process_input(app, u32::MAX, PASS_BLOCKS);
        drop_backlog();
        let processed = BLOCKS.load(Relaxed);
        DSP_BLOCKS.store(DSP_BLOCKS.load(Relaxed).wrapping_add(processed.wrapping_sub(start)), Relaxed);
        if processed.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
            LAST_RENDER.store(processed, Relaxed);
            render();
            DRAW_BLOCKS.store(DRAW_BLOCKS.load(Relaxed).wrapping_add(BLOCKS.load(Relaxed).wrapping_sub(processed)), Relaxed);
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
    new_session();
    apply_actions(app);
    apply_touches(app);
    process_input(app, MAX_FRAMES_PER_CALL, PASS_BLOCKS);
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
        LAST_RENDER.store(now, Relaxed);
        render();
    }
}
