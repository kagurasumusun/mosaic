/* sensorbus A32 firmware — verified listing.
 *
 * This file is the human-readable source of FIRMWARE_WORDS in
 * src/sensorbus.rs. It was assembled offline with keystone (KS_ARCH_ARM /
 * KS_MODE_ARM) at base 0, verified by capstone disassembly and by a full
 * peripheral simulation (unicorn + Python models of DW-APB-I2C, PL022,
 * PL061, PL011, the bit-bang decoder and the shared I2C bus); the run
 * prints "ctl=60 bb=60 spi=60\n".
 *
 * Register use: r2 peripheral base (loaded from the literal pool), r4
 * controller chip id, r5 bit-bang id / print scratch, r6 SPI chip id,
 * r7 preserves r5 across printing, sp = RAM top (putbyte pushes LR).
 * The final four words are the literal pool: I2C, GPIO, SSP, UART bases.
 */
ldr r2, [pc, #0]
mov r0, #1
str r0, [r2, #0x6C]
mov r0, #0x76
str r0, [r2, #0x04]
mov r0, #0xD0
str r0, [r2, #0x10]
mov r0, #0x700
str r0, [r2, #0x10]
poll1:
ldr r0, [r2, #0x70]
tst r0, #8
beq poll1
ldr r4, [r2, #0x10]
mov r0, #0
str r0, [r2, #0x6C]
ldr r2, [pc, #0]
mov r0, #0xFF
str r0, [r2, #0x400]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #0xC]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #3
str r0, [r2, #0xC]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #0xC]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #3
str r0, [r2, #0xC]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #0xC]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #1
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, #0
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r5, r5, lsl #1
ldr r0, [r2, #0x3FC]
and r0, r0, #1
orr r5, r5, r0
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #2
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #8]
mov r0, #0
str r0, [r2, #4]
mov r0, #2
str r0, [r2, #8]
mov r0, #3
str r0, [r2, #0xC]
mov r7, r5
ldr r2, [pc, #0]
mov r0, #0x07
str r0, [r2, #0x00]
mov r0, #0x02
str r0, [r2, #0x04]
mov r0, #0xD0
str r0, [r2, #0x08]
mov r0, #0
str r0, [r2, #0x08]
ldr r0, [r2, #0x08]
ldr r6, [r2, #0x08]
mov r0, #0
str r0, [r2, #0x04]
ldr r2, [pc, #0]
mov r0, #1
orr r0, r0, #0x100
str r0, [r2, #0x30]
mov r0, #99
bl putc
mov r0, #116
bl putc
mov r0, #108
bl putc
mov r0, #61
bl putc
mov r5, r4
bl putbyte
mov r0, #32
bl putc
mov r0, #98
bl putc
mov r0, #98
bl putc
mov r0, #61
bl putc
mov r5, r7
bl putbyte
mov r0, #32
bl putc
mov r0, #115
bl putc
mov r0, #112
bl putc
mov r0, #105
bl putc
mov r0, #61
bl putc
mov r5, r6
bl putbyte
mov r0, #10
bl putc
b spin
spin:
b spin
putc:
str r0, [r2, #0]
bx lr
putbyte:
push {lr}
mov r0, r5, lsr #4
bl putnib
and r0, r5, #0xF
bl putnib
pop {pc}
putnib:
cmp r0, #10
addlt r0, r0, #0x30
addge r0, r0, #0x57
b putc
