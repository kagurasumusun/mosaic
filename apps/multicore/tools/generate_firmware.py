#!/usr/bin/env python3
"""Provenance tool: regenerate apps/multicore firmware words.

Run from the repository root with a venv that has keystone-engine and
capstone installed:

    python apps/multicore/tools/generate_firmware.py > tmp/multicore_fw_out.txt

Prints the verified capstone listings plus the Rust word blocks that must
match apps/multicore/src/multicore.rs, and writes tmp/multicore_fw_asm.txt
(primary + secondary A32 listing), tmp/multicore_rv.txt (RV32I listing) and
the three raw images tmp/multicore_{primary,secondary,cop}.bin.

A32 goes through keystone; RV32I is hand-encoded here (the pip keystone
wheel ships without the RISC-V backend) and cross-checked with capstone.
"""
import struct

from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM
from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM, CS_ARCH_RISCV
import capstone as _cs

UART_BASE = 0x1000_0000
MHU_BASE = 0x1000_1000
SHARED_BASE = 0x0001_0000


class Emit:
    def __init__(self):
        self.lines = []
        self.pool_ldrs = []  # (word_index, pool_slot)
        self.n = 0

    def raw(self, *ls):
        for line in ls:
            self.lines.append(line)
            if not line.strip().endswith(":"):
                self.n += 1

    def pool(self, slot):
        self.pool_ldrs.append((self.n, slot))
        self.lines.append("ldr r2, [pc, #0]")
        self.n += 1


def assemble_a32(e, pool):
    src = "\n".join(e.lines)
    ks = Ks(KS_ARCH_ARM, KS_MODE_ARM)
    body, _ = ks.asm(src, addr=0)
    n_words = len(body) // 4
    assert n_words == e.n, (n_words, e.n)
    words = list(struct.unpack("<%dI" % n_words, bytes(body)))
    for ldr_index, slot in e.pool_ldrs:
        target = (n_words + slot) * 4
        pc = (ldr_index + 2) * 4
        off = target - pc
        assert 0 <= off < 4096, (ldr_index, slot, off)
        assert words[ldr_index] == 0xE59F2000, hex(words[ldr_index])
        words[ldr_index] |= off
    image_words = words + list(pool)
    image = struct.pack("<%dI" % len(image_words), *image_words)
    cs = Cs(CS_ARCH_ARM, CS_MODE_ARM)
    dis = list(cs.disasm(image, 0))
    assert len(dis) == len(image_words), (len(dis), len(image_words))
    # Verify pool LDR targets.
    for ldr_index, slot in e.pool_ldrs:
        ins = dis[ldr_index]
        assert ins.mnemonic == "ldr", ins
        off = int(ins.op_str.split("#")[1].rstrip("]"), 0)
        target = (ldr_index + 2) * 4 + off
        value = struct.unpack("<I", image[target:target + 4])[0]
        assert value == pool[slot], (hex(value), hex(pool[slot]))
    # keystone round-trip
    body2, _ = ks.asm(src, addr=0)
    assert bytes(body2) == bytes(body)
    return image_words, dis, src


# ---------------------------------------------------------------- primary ---
# core0 (ARM926): drive both handshakes, print "smp=XX\namp=XX\n".
# r2 must point at the UART for putc and at the MHU for doorbells, so each
# phase reloads r2 from the literal pool (slots: 0=UART, 1=MHU).
e = Emit()
e.pool(0)  # UART -> r2
e.raw("mov r0, #1", "orr r0, r0, #0x100", "str r0, [r2, #0x30]")  # CR
e.pool(1)  # MHU -> r2
e.raw("mov r3, #0x10000")  # shared base
# SMP request to core1: word0 = 0x21, SET bank0 bit0
e.raw("mov r0, #0x21", "str r0, [r3, #0]")
e.raw("mov r0, #1", "str r0, [r2, #0x008]")
e.raw("poll1:", "ldr r0, [r2, #0x100]", "tst r0, #1", "beq poll1")
e.raw("mov r0, #1", "str r0, [r2, #0x10C]")  # CLEAR1 bit0
e.raw("ldr r4, [r3, #4]")
e.pool(0)  # UART -> r2 for the print phase
for ch in "smp=":
    e.raw(f"mov r0, #{ord(ch)}", "bl putc")
e.raw("mov r5, r4", "bl putbyte", f"mov r0, #{ord(chr(10))}", "bl putc")
e.pool(1)  # MHU -> r2 for the second handshake
# AMP request to core2: word2 = 0x40, SET bank2 bit0
e.raw("mov r0, #0x40", "str r0, [r3, #8]")
e.raw("mov r0, #1", "str r0, [r2, #0x208]")
e.raw("poll2:", "ldr r0, [r2, #0x100]", "tst r0, #2", "beq poll2")
e.raw("mov r0, #2", "str r0, [r2, #0x10C]")  # CLEAR1 bit1
e.raw("ldr r4, [r3, #0xC]")
e.pool(0)  # UART -> r2 for the final print
for ch in "amp=":
    e.raw(f"mov r0, #{ord(ch)}", "bl putc")
e.raw("mov r5, r4", "bl putbyte", f"mov r0, #{ord(chr(10))}", "bl putc")
e.raw("b spin", "spin:", "b spin")
# putc: leaf. putnib tail-calls putc so LR survives. putbyte nests two BLs
# and therefore saves/restores LR with push {lr} / pop {pc}.
e.raw("putc:", "str r0, [r2, #0]", "bx lr")
e.raw("putbyte:", "push {lr}", "mov r0, r5, lsr #4", "bl putnib",
      "and r0, r5, #0xF", "bl putnib", "pop {pc}")
e.raw("putnib:", "cmp r0, #10", "addlt r0, r0, #0x30",
      "addge r0, r0, #0x57", "b putc")
PRIMARY, primary_dis, primary_src = assemble_a32(e, [UART_BASE, MHU_BASE])

# -------------------------------------------------------------- secondary ---
# core1 (ARM926): respond to the bank0 doorbell: word1 = word0 + 0x40.
e = Emit()
e.pool(0)  # MHU -> r2
e.raw("mov r3, #0x10000")
e.raw("polls:", "ldr r0, [r2, #0x000]", "tst r0, #1", "beq polls")
e.raw("mov r0, #1", "str r0, [r2, #0x00C]")  # CLEAR0 bit0
e.raw("ldr r4, [r3, #0]", "add r4, r4, #0x40", "str r4, [r3, #4]")
e.raw("mov r0, #1", "str r0, [r2, #0x108]")  # SET1 bit0
e.raw("b spin", "spin:", "b spin")
SECONDARY, secondary_dis, secondary_src = assemble_a32(e, [MHU_BASE])

# -------------------------------------------------------------------- cop ---
# RV32I encoders (base integer subset only).


def rv_lui(rd, imm20):
    return ((imm20 & 0xFFFFF) << 12) | (rd << 7) | 0x37


def rv_addi(rd, rs1, imm):
    return ((imm & 0xFFF) << 20) | (rs1 << 15) | (rd << 7) | 0x13


def rv_xori(rd, rs1, imm):
    return ((imm & 0xFFF) << 20) | (rs1 << 15) | (4 << 12) | (rd << 7) | 0x13


def rv_andi(rd, rs1, imm):
    return ((imm & 0xFFF) << 20) | (rs1 << 15) | (7 << 12) | (rd << 7) | 0x13


def rv_lw(rd, rs1, off):
    return ((off & 0xFFF) << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x03


def rv_sw(rs2, rs1, off):
    imm = off & 0xFFF
    return ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((imm & 0x1F) << 7) | 0x23


def rv_beq(rs1, rs2, off):
    o = off & 0x1FFF
    imm = (((o >> 12) & 1) << 31) | (((o >> 5) & 0x3F) << 25) \
        | (((o >> 1) & 0xF) << 8) | (((o >> 11) & 1) << 7)
    return imm | (rs2 << 20) | (rs1 << 15) | 0x63


def rv_jal(rd, off):
    o = off & 0x1FFFFF
    imm = (((o >> 20) & 1) << 31) | (((o >> 1) & 0x3FF) << 21) \
        | (((o >> 11) & 1) << 20) | (((o >> 12) & 0xFF) << 12)
    return imm | (rd << 7) | 0x6F


# core2 (RV32I): respond to the bank2 doorbell: word3 = word2 XOR 0xFF.
cop_lines = [
    ("lui x5, 0x10001", rv_lui(5, 0x10001)),       # MHU base
    ("lui x7, 0x10", rv_lui(7, 0x10)),             # shared base
    ("pollc:", None),
    ("lw x8, 0x200(x5)", rv_lw(8, 5, 0x200)),      # STAT2
    ("andi x8, x8, 1", rv_andi(8, 8, 1)),
    ("beq x8, x0, pollc", None),                   # patched below
    ("addi x9, x0, 1", rv_addi(9, 0, 1)),
    ("sw x9, 0x20C(x5)", rv_sw(9, 5, 0x20C)),      # CLEAR2 bit0
    ("lw x10, 8(x7)", rv_lw(10, 7, 8)),            # request word2
    ("xori x11, x10, 0xFF", rv_xori(11, 10, 0xFF)),
    ("sw x11, 12(x7)", rv_sw(11, 7, 12)),          # response word3
    ("addi x12, x0, 2", rv_addi(12, 0, 2)),
    ("sw x12, 0x108(x5)", rv_sw(12, 5, 0x108)),    # SET1 bit1
    ("spinc:", None),
    ("j spinc", None),                             # patched below
]
words = []
label_word = {}
word_of_line = {}
for i, (text, word) in enumerate(cop_lines):
    if text.endswith(":"):
        label_word[text[:-1]] = len(words)
        continue
    word_of_line[i] = len(words)
    words.append(word if word is not None else 0)
beq_line = next(i for i, (t, _) in enumerate(cop_lines) if t.startswith("beq"))
jal_line = next(i for i, (t, _) in enumerate(cop_lines) if t == "j spinc")
beq_off = (label_word["pollc"] - (word_of_line[beq_line] + 1)) * 4
assert -4096 <= beq_off < 0, beq_off
words[word_of_line[beq_line]] = rv_beq(8, 0, beq_off & 0x1FFF)
words[word_of_line[jal_line]] = rv_jal(0, 0)
COP = words
_rv_mode = getattr(_cs, "CS_MODE_RISCV32", 0)
cs_rv = Cs(CS_ARCH_RISCV, _rv_mode)
cop_image = struct.pack("<%dI" % len(COP), *COP)
cop_dis = list(cs_rv.disasm(cop_image, 0))
assert len(cop_dis) == len(COP), (len(cop_dis), len(COP))
# Cross-check the branch target: capstone resolves beq to the poll label.
beq_ins = cop_dis[word_of_line[beq_line]]
assert beq_ins.mnemonic in ("beq", "beqz"), beq_ins
cop_spin = label_word["spinc"] * 4

# ---------------------------------------------------------------- listings --
print(f"; primary: {len(PRIMARY)} words")
for ins in primary_dis:
    print(f"; {ins.address:04x}: {ins.mnemonic} {ins.op_str}")
print(f"; secondary: {len(SECONDARY)} words")
for ins in secondary_dis:
    print(f"; {ins.address:04x}: {ins.mnemonic} {ins.op_str}")
print(f"; cop: {len(COP)} words, spin @{cop_spin:#x}")
for ins in cop_dis:
    print(f"; {ins.address:04x}: {ins.mnemonic} {ins.op_str}")


def rust_block(name, image_words):
    print(f"\npub static {name}: &[u32] = &[")
    line = "   "
    for i, w in enumerate(image_words):
        line += f" 0x{w:08X},"
        if (i + 1) % 8 == 0:
            print(line)
            line = "   "
    if line.strip():
        print(line)
    print("];")


rust_block("PRIMARY_WORDS", PRIMARY)
rust_block("SECONDARY_WORDS", SECONDARY)
rust_block("COP_WORDS", COP)

with open("tmp/multicore_fw_asm.txt", "w") as f:
    f.write("; primary\n" + primary_src + "\n; secondary\n" + secondary_src + "\n")
with open("tmp/multicore_rv.txt", "w") as f:
    for text, _ in cop_lines:
        f.write(text + "\n")
for name, image_words in (("primary", PRIMARY), ("secondary", SECONDARY), ("cop", COP)):
    with open(f"tmp/multicore_{name}.bin", "wb") as f:
        f.write(struct.pack("<%dI" % len(image_words), *image_words))
print("\nwrote tmp/multicore_{fw_asm.txt,rv.txt,primary.bin,secondary.bin,cop.bin}")
