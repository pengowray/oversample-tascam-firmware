/* The patch runs from SDRAM 0x81830000 to 0x818fffff: the bootloader copies
   flash 0x20000 to 0x1fe0000 into SDRAM from 0x80000000, the stock image ends
   at 0x8182ffff, IAR's init table starts filling SDRAM at 0x81900000, and no
   stock code refers to anything in between (checked in Ghidra for V2.03). The
   patch tool stores this region at flash 0x1850000, just after the stock
   image, including .bss as zeros. */
MEMORY
{
  PATCH (rwx) : ORIGIN = 0x81830000, LENGTH = 0xd0000
}

ENTRY(X8_PATCH_HEADER)

SECTIONS
{
  .header : { KEEP(*(.header)) } > PATCH
  .text : { *(.text .text.*) } > PATCH
  .rodata : { *(.rodata .rodata.*) } > PATCH
  .data : { *(.data .data.*) } > PATCH
  .bss (NOLOAD) : { *(.bss .bss.* COMMON) } > PATCH
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.* .ARM.extab.*) }
}
