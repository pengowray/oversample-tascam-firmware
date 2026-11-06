pub const DISPLAY: usize = 0x87eb_0000; // 320 x 480 RGB565, 0x4b000 bytes
pub const TE_STORAGE: usize = 0x87f0_0000; // f32 x TE_SAMPLES
pub const TE_SAMPLES: usize = 96_000; // 0.5 s at 192 kHz
/// SDRAM between two zeroed ranges, 0x865e1d88 to 0x865fffff, which no
/// stock code refers to; the MPU maps it cached (region 9). The interrupt
/// and the tasks run on one core, so cached rings need no maintenance.
pub const OUT_RING: usize = 0x865e_2000; // f32 x OUT_SAMPLES, 64 KB
pub const OUT_SAMPLES: u32 = 16_384;
/// The input ring is in the patch region (`IN_RING`), also cached.
pub const IN_FRAMES: u32 = 8_192;
