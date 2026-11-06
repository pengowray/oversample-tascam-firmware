pub const DISPLAY: usize = 0x87eb_0000; // 320 x 480 RGB565, 0x4b000 bytes
pub const TE_STORAGE: usize = 0x87f0_0000; // f32 x TE_SAMPLES
pub const TE_SAMPLES: usize = 96_000; // 0.5 s at 192 kHz
/// SDRAM between two zeroed ranges, 0x865e1d88 to 0x865fffff, which no
/// stock code refers to; the MPU maps it cached (region 9, 0x86400000 to
/// 0x865fffff). The reset handler points the stack at the top of this range for
/// one call to a function with no stack use, then moves it to DTCM, so nothing is
/// stored here. The interrupt and the tasks run on one core, so cached rings need
/// no maintenance.
pub const OUT_RING: usize = 0x865e_2000; // [f32; 2] x OUT_FRAMES, left then right, 120 KB
/// As many frames as fit in that range. Not a power of two, so an index into the ring
/// comes from `%`, and the frame counts jump in the ring when they wrap; see
/// `OUT_WRITE` in state.rs for what that does.
pub const OUT_FRAMES: u32 = 15_360;
const _: () = assert!(OUT_RING + OUT_FRAMES as usize * 8 == 0x8660_0000);
// The audio hook reads 4 frames a block and relies on a block not crossing the end.
const _: () = assert!(OUT_FRAMES % 4 == 0);
/// The input ring is in the patch region (`IN_RING`), also cached.
pub const IN_FRAMES: u32 = 8_192;
