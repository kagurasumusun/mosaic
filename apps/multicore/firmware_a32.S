/* multicore A32 firmware — verified listings (core0 primary + core1 secondary).
 *
 * Human-readable source of PRIMARY_WORDS / SECONDARY_WORDS in
 * src/multicore.rs. Assembled offline with keystone (KS_ARCH_ARM /
 * KS_MODE_ARM) at base 0 by tools/generate_firmware.py, verified by
 * capstone disassembly (below) and by the full three-core simulation
 * (unicorn ARM + RISCV, Python MHU/PL011 models): the run prints
 * "smp=61\namp=bf\n".
 *
 * Register use: r2 peripheral base (UART 0x10000000 / MHU 0x10001000,
 * reloaded from the literal pool at every phase switch), r3 shared page
 * 0x10000, r4 handshake payload, r5 print scratch, sp = 0x800 (putbyte
 * pushes LR). The last two words of each image are the literal pool.
 *
 * core0 (primary, 69 words) — capstone listing:
 * 0000: ldr r2, [pc, #0x104]   ; UART base
 * 0004: mov r0, #1
 * 0008: orr r0, r0, #0x100
 * 000c: str r0, [r2, #0x30]    ; UART CR = UARTEN|TXE
 * 0010: ldr r2, [pc, #0xf8]    ; MHU base
 * 0014: mov r3, #0x10000
 * 0018: mov r0, #0x21
 * 001c: str r0, [r3]           ; shared[0] = 0x21
 * 0020: mov r0, #1
 * 0024: str r0, [r2, #8]       ; MHU SET0 = 1 (ring core1)
 * 0028: ldr r0, [r2, #0x100]   ; poll1: STAT1
 * 002c: tst r0, #1
 * 0030: beq #0x28
 * 0034: mov r0, #1
 * 0038: str r0, [r2, #0x10c]   ; CLEAR1 bit0 (ack core1)
 * 003c: ldr r4, [r3, #4]       ; r4 = shared[1] (= 0x61)
 * 0040: ldr r2, [pc, #0xc4]    ; UART base
 * 0044: mov r0, #0x73          ; 's'
 * 0048: bl #0xdc               ; putc
 * 004c: mov r0, #0x6d          ; 'm'
 * 0050: bl #0xdc
 * 0054: mov r0, #0x70          ; 'p'
 * 0058: bl #0xdc
 * 005c: mov r0, #0x3d          ; '='
 * 0060: bl #0xdc
 * 0064: mov r5, r4
 * 0068: bl #0xe4               ; putbyte
 * 006c: mov r0, #0xa
 * 0070: bl #0xdc
 * 0074: ldr r2, [pc, #0x94]    ; MHU base
 * 0078: mov r0, #0x40
 * 007c: str r0, [r3, #8]       ; shared[2] = 0x40
 * 0080: mov r0, #1
 * 0084: str r0, [r2, #0x208]   ; SET2 = 1 (ring core2)
 * 0088: ldr r0, [r2, #0x100]   ; poll2: STAT1
 * 008c: tst r0, #2
 * 0090: beq #0x88
 * 0094: mov r0, #2
 * 0098: str r0, [r2, #0x10c]   ; CLEAR1 bit1 (ack core2)
 * 009c: ldr r4, [r3, #0xc]     ; r4 = shared[3] (= 0xbf)
 * 00a0: ldr r2, [pc, #0x64]    ; UART base
 * 00a4: mov r0, #0x61          ; 'a'
 * 00a8: bl #0xdc
 * 00ac: mov r0, #0x6d          ; 'm'
 * 00b0: bl #0xdc
 * 00b4: mov r0, #0x70          ; 'p'
 * 00b8: bl #0xdc
 * 00bc: mov r0, #0x3d          ; '='
 * 00c0: bl #0xdc
 * 00c4: mov r5, r4
 * 00c8: bl #0xe4
 * 00cc: mov r0, #0xa
 * 00d0: bl #0xdc
 * 00d4: b #0xd8                ; spin
 * 00d8: b #0xd8
 * 00dc: str r0, [r2]           ; putc: UART DR
 * 00e0: bx lr
 * 00e4: str lr, [sp, #-4]!     ; putbyte: push {lr}
 * 00e8: lsr r0, r5, #4
 * 00ec: bl #0xfc               ; putnib
 * 00f0: and r0, r5, #0xf
 * 00f4: bl #0xfc
 * 00f8: pop {pc}
 * 00fc: cmp r0, #0xa           ; putnib (tail-calls putc, LR intact)
 * 0100: addlt r0, r0, #0x30
 * 0104: addge r0, r0, #0x57
 * 0108: b #0xdc
 * 010c: .word 0x10000000       ; pool: UART base
 * 0110: .word 0x10001000       ; pool: MHU base
 *
 * core1 (secondary, 15 words) — capstone listing:
 * 0000: ldr r2, [pc, #0x30]    ; MHU base
 * 0004: mov r3, #0x10000
 * 0008: ldr r0, [r2]           ; polls: STAT0
 * 000c: tst r0, #1
 * 0010: beq #8
 * 0014: mov r0, #1
 * 0018: str r0, [r2, #0xc]     ; CLEAR0 bit0 (ack core0)
 * 001c: ldr r4, [r3]           ; shared[0]
 * 0020: add r4, r4, #0x40
 * 0024: str r4, [r3, #4]       ; shared[1] = shared[0] + 0x40
 * 0028: mov r0, #1
 * 002c: str r0, [r2, #0x108]   ; SET1 = 1 (ring core0)
 * 0030: b #0x34                ; spin
 * 0034: b #0x34
 * 0038: .word 0x10001000       ; pool: MHU base
 */
