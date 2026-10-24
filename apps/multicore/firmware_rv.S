/* multicore RV32I firmware — verified listing (core2 coprocessor).
 *
 * Human-readable source of COP_WORDS in src/multicore.rs. Hand-encoded by
 * tools/generate_firmware.py (the pip keystone wheel ships without the
 * RISC-V backend) and verified by capstone (CS_ARCH_RISCV/CS_MODE_RISCV32)
 * disassembly below plus the full three-core simulation: the coprocessor
 * answers core0's AMP doorbell with shared[3] = shared[2] XOR 0xFF.
 *
 * Register use: x5/t0 MHU base 0x10001000, x7/t2 shared page 0x10000.
 * No stack use (leaf code). 13 words, spin at 0x30.
 *
 * 0000: lui t0, 0x10001        ; MHU base
 * 0004: lui t2, 0x10           ; shared base
 * 0008: lw s0, 0x200(t0)       ; pollc: STAT2
 * 000c: andi s0, s0, 1
 * 0010: beqz s0, -0xc
 * 0014: addi s1, zero, 1
 * 0018: sw s1, 0x20c(t0)       ; CLEAR2 bit0 (ack core0)
 * 001c: lw a0, 8(t2)           ; request word shared[2] (= 0x40)
 * 0020: xori a1, a0, 0xff
 * 0024: sw a1, 0xc(t2)         ; shared[3] = 0xbf
 * 0028: addi a2, zero, 2
 * 002c: sw a2, 0x108(t0)       ; SET1 = 2 (ring core0, bit1)
 * 0030: j 0                    ; spinc: j spinc
 */
