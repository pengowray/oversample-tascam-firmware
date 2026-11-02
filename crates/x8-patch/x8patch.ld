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

  /* Run from ITCM: the app's processing, the DSP library it inlines from,
     the FFT, libm, the snapshot builder and the patch's input loop. */
  .itcm : ALIGN(8)
  {
    __itcm_start = .;
    *(.text.*BatApp7process*)
    *(.text.*bat_dsp*)
    *(.text.*microfft*)
    *(.text.*libm*)
    *(.text.*build_snapshot*)
    *(.text.*process_input*)
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
