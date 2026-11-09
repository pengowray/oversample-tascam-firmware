/* The patch runs from SDRAM 0x81830000 to 0x818fffff: the bootloader copies
   flash 0x20000 to 0x1fe0000 into SDRAM from 0x80000000, the stock image ends
   at 0x8182ffff, IAR's init table starts filling SDRAM at 0x81900000, and no
   stock code refers to anything in between (checked in Ghidra for V2.03). The
   patch tool stores this region at flash 0x1850000, just after the stock
   image, including .bss as zeros.

   The FlexRAM is 288 KB of ITCM and 224 KB of DTCM (IOMUXC_GPR17 = 0xffffeaaa,
   set by 0x80e3f958). The stock ITCM code ends at 0x31a92 and the stock DTCM
   data, zeroed ranges and main stack end at 0x20011cd8; no stock code refers
   to either remainder. The patch copies its hot code into ITCM (.itcm, stored
   in the patch region) before first use, and keeps its hot state in DTCM. */
MEMORY
{
  PATCH (rwx) : ORIGIN = 0x81830000, LENGTH = 0xd0000
  ITCM (rwx) : ORIGIN = 0x00031b00, LENGTH = 0x16500
  DTCM (rw) : ORIGIN = 0x20011d00, LENGTH = 0x26300
}

ENTRY(X8_PATCH_HEADER)

SECTIONS
{
  .header : { KEEP(*(.header)) } > PATCH

  /* Code from the DSP library that BAT PLAYBACK's card reader calls, kept out of ITCM:
     reading a WAV header and setting the resampler's rates. The reader is its own task,
     which tools/x8patch.py's check does not count as running only after load_itcm. A
     section is placed by the first pattern that matches it, so this comes before .itcm. */
  .text.cold_dsp :
  {
    *(.text.*bat_dsp3wav5parse*)
    *(.text.*bat_dsp3wav8read_fmt*)
    *(.text.*bat_dsp3wav6Format*)
    *(.text.*9Resampler9set_rates*)
  } > PATCH

  /* Run from ITCM: the app's processing, including the closures of its analysis
     that the compiler does not inline, its meters, Auto volume (module level, also
     inlined at present), the DSP library it inlines from, the FFT, libm, the snapshot
     builder, the patch's input loop and the part of the chime decoder that runs for
     every sample. The rest of the decoder does not fit. Also the audio hook, which
     runs 48,000 times a second inside the audio interrupt: from SDRAM it cost about
     900 cycles a call on B239 (`hook` 7 to 9% of the CPU), which fits refilling the
     instruction cache each time. Only its copy `audio_hook_itcm`, which the bat
     screen switches to after loading ITCM; the one installed at start-up stays in
     SDRAM.

     Never memcpy, memset or anything else that code outside the bat screen calls: the
     stock firmware calls the patch's hooks (keys, takes, the launcher, the clock) from
     start-up, before the bat screen first loads ITCM, and a call into ITCM then runs
     whatever was there. B241 to B250 had memcpy and memset here, and load_itcm's own
     copy called memcpy in ITCM, so the bat screen hung on opening. tools/x8patch.py
     checks which functions outside ITCM call into it. */
  .itcm : ALIGN(8)
  {
    __itcm_start = .;
    *(.text.*BatApp7process*)
    *(.text.*BatApp7analyse*)
    *(.text.*Meter6update*)
    *(.text.*5level*)
    *(.text.*bat_dsp*)
    *(.text.*microfft*)
    *(.text.*libm*)
    *(.text.*build_snapshot*)
    *(.text.*process_input*)
    *(.text.*FrontEnd4push*)
    *(.text.*5audio15audio_hook_itcm*)
    . = ALIGN(8);
    __itcm_end = .;
  } > ITCM AT> PATCH
  __itcm_load = LOADADDR(.itcm);

  .text : { *(.text .text.*) } > PATCH
  .rodata : { *(.rodata .rodata.*) } > PATCH
  .data : { *(.data .data.*) } > PATCH
  .bss (NOLOAD) : { *(.bss .bss.* COMMON) } > PATCH
  /* Buffers the patch writes before reading (stacks, the app's buffers, the
     off-screen frame): left out of the update file, so they hold whatever the
     bootloader copied from flash. */
  .noinit (NOLOAD) : { *(.noinit .noinit.*) } > PATCH
  /* Hot state in DTCM, also written before it is read. */
  .dtcm (NOLOAD) : ALIGN(8) { *(.dtcm .dtcm.*) } > DTCM
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.* .ARM.extab.*) }
}
