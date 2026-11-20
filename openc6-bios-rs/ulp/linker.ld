/* LP 程序链接脚本。
 *
 * LP RAM 固定映射在 0x5000_0000，LP 核复位后从这个地址取指，
 * 所以入口必须落在镜像的第一个字节，且整个镜像的位置就是运行时地址。
 *
 * 内存划分（LP RAM 共 16 KB）:
 *   0x5000_0000  程序镜像
 *   0x5000_2000  栈顶（向下增长）
 *   0x5000_3000  HP/LP 共享变量区（见 src/main.rs 的 SHARED_BASE）
 */
ENTRY(_start)

SECTIONS
{
  . = 0x50000000;

  .text : ALIGN(4)
  {
    KEEP(*(.text.entry))
    *(.text .text.*)
  }

  /* 复位函数放最后，保证 lp_main 紧跟 _start，入口附近只有一条跳转 */
  .trigger : ALIGN(4)
  {
    *(.trigger)
  }

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
    *(.srodata .srodata.*)
  }

  .data : ALIGN(4)
  {
    *(.data .data.*)
    *(.sdata .sdata.*)
  }

  .bss (NOLOAD) : ALIGN(4)
  {
    *(.bss .bss.*)
    *(.sbss .sbss.*)
    *(COMMON)
  }

  /DISCARD/ :
  {
    *(.eh_frame)
    *(.comment)
    *(.riscv.attributes)
  }
}
