#!/usr/bin/env python3
"""Generate the ISA conformance corpus for brain-emu.

The expected values come from Unicorn (a QEMU-derived ARM core), which we use
purely as an *oracle*: this script assembles each case with Keystone, runs it on
Unicorn with a known register/memory state, and records the resulting registers,
CPSR and data memory.  tests/test_instr.cpp then replays exactly those cases
through brain-emu and diffs the result.

Why an oracle instead of hand-written expectations: the carry/overflow/Q-flag
corner cases are precisely where an emulator goes subtly wrong, and there are
thousands of interesting operand combinations.

Usage:  python3 tests/gen_instr_tests.py [--cases N] [--out tests/data/corpus.txt]

Divergences deliberately excluded (see docs/ISA-COVERAGE.md):
  * ARMv6/7-only encodings (the corpus only uses words Keystone encodes as
    16-bit Thumb-1 / classic ARM forms);
  * UNPREDICTABLE encodings (Rd==15 in MUL, PC in a LDM/STM list, ...);
  * unaligned addresses (ARMv5 wraps within the word, QEMU does not);
  * CPSR bits that differ by architecture version: J (24), GE (19:16), A (8),
    E (9) -- the harness masks CPSR with 0xF80000FF.
"""
import argparse
import re
import os
import random
import struct
import sys

from keystone import (Ks, KS_ARCH_ARM, KS_MODE_ARM, KS_MODE_THUMB,
                     KS_MODE_LITTLE_ENDIAN)
from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_MODE_THUMB, UC_PROT_ALL
from unicorn.arm_const import (UC_ARM_REG_R0, UC_ARM_REG_R15, UC_ARM_REG_CPSR)

CODE_BASE = 0x00008000
DATA_BASE = 0x00009000
DATA_SIZE = 0x100
RAM_BASE = 0x00000000
RAM_SIZE = 0x00020000
REG_MASK = 0xF80000FF          # N Z C V Q | I F T mode (see docstring)

R_REGS = list(range(UC_ARM_REG_R0, UC_ARM_REG_R0 + 16))


# Keystone assembles for a *recent* architecture, so a few ARMv5T Thumb words
# either get a wider encoding or a different mnemonic.  These two-operand forms
# are hand-encoded from the A6.4.1 table (0x4000 | op<<6 | Rm<<3 | Rd), which is
# exactly what the ARM926EJ-S decodes.
THUMB_ALU = {
    "ANDS": 0x00, "EORS": 0x01, "LSLS": 0x02, "LSRS": 0x03, "ASRS": 0x04,
    "ADCS": 0x05, "SBCS": 0x06, "RORS": 0x07, "TST": 0x08, "RSBS": 0x09,
    "CMP": 0x0A, "CMN": 0x0B, "ORRS": 0x0C, "MULS": 0x0D, "BICS": 0x0E,
    "MVNS": 0x0F,
}


def manual_thumb(line):
    import re
    m = re.match(r"^(LSLS|LSRS|ASRS) R(\d+), R(\d+), #(\d+)$", line)
    if m:
        # format 1: 000 type imm5 Rm Rdn, and #32 is encoded as #0 (A6.4.1)
        typ = {"LSLS": 0, "LSRS": 1, "ASRS": 2}[m.group(1)]
        rd, rm, amt = int(m.group(2)), int(m.group(3)), int(m.group(4))
        if rd > 7 or rm > 7 or amt > 32:
            return None
        return (typ << 11) | ((amt & 31) << 6) | (rm << 3) | rd
    m = re.match(r"^([A-Z0-9]+) R(\d+), R(\d+)$", line)
    if not m:
        return None
    op = THUMB_ALU.get(m.group(1))
    rd, rm = int(m.group(2)), int(m.group(3))
    if op is None or rd > 7 or rm > 7:
        return None
    return 0x4000 | (op << 6) | (rm << 3) | rd


class Asm:
    """Keystone wrapper with an explicit fallback table.

    Keystone defaults to a recent architecture, so a handful of ARMv5 words
    (Thumb LSL#0 = CPY, ARM BLX immediate) either assemble to a wider form or
    are refused; those we encode ourselves.
    """

    def __init__(self, mode, width):
        self.ks = Ks(KS_ARCH_ARM, mode | KS_MODE_LITTLE_ENDIAN)
        self.width = width

    def asm(self, line, addr=CODE_BASE):
        if self.width == 2:
            w = manual_thumb(line)
            if w is not None:
                return struct.pack("<H", w)
        enc, count = self.ks.asm(line, addr=addr)
        if count != 1:
            raise AssertionError("not one instruction: %r (%d)" % (line, count))
        if len(enc) != self.width:
            raise AssertionError("wrong width %d for ARMv5: %r" %
                                 (len(enc), line))
        return bytes(enc)


# --------------------------------------------------------------------------
# cases: (name, mode, [asm lines])
# --------------------------------------------------------------------------

def arith_cases(rnd, n_each=3):
    """Data processing, multiplies, saturating ops: many operand combinations."""
    out = []
    ops_arm = [
        "ANDS R0, R1, R2", "EORS R0, R1, R2", "BICS R0, R1, R2", "ORRS R0, R1, R2",
        "MVNS R0, R1",
        "ADDS R0, R1, R2", "SUBS R0, R1, R2", "RSBS R0, R1, R2",
        "ADC R0, R1, R2", "SBC R0, R1, R2", "RSC R0, R1, R2",
        "CMN R1, R2", "CMP R1, R2", "TEQ R1, R2", "TST R1, R2",
        "ADDS R0, R1, R2, LSL #3", "ADDS R0, R1, R2, LSR #5",
        "ADDS R0, R1, R2, ASR #7", "ADDS R0, R1, R2, ROR #11",
        "ADDS R0, R1, R2, LSL R3", "ADDS R0, R1, R2, ASR R3",
        "MOVS R0, #0xFF", "ADDS R0, R1, #0x4010000",
        "ADCS R0, R1, R2", "SBCS R0, R1, R2",
        "ADD R15, R1, #4",                     # PC write, no flags
    ]
    ops_thumb = [
        "MVNS R0, R1", "ADDS R0, R1", "SUBS R0, R1",
        "EORS R0, R1", "ANDS R0, R1", "ORRS R0, R1", "BICS R0, R1",
        "TSTS R0, R1", "TEQS R0, R1", "CMPS R0, R1", "CMNS R0, R1",
        "ADCS R0, R1", "SBCS R0, R1", "RSBS R0, R1",
        "MULS R0, R1", "LSLS R0, R1, #1", "LSRS R0, R1, #1", "ASRS R0, R1, #1",

        "MOV R0, R1", "ADD R8, R1", "CMP R8, R1", "MOV R8, R1", "ADD R0, R8",
        "ADDS R0, R1, R2", "SUBS R0, R1, R2", "ADDS R0, R1, #5", "SUBS R0, R7, #7",
        "MOVS R0, #0", "ADDS R0, #0xFF", "SUBS R0, #0xFF", "CMPS R0, #0",
    ]
    for line in ops_arm:
        out.append(("arm_" + slug(line), "arm", [line]))
    for line in ops_thumb:
        out.append(("thumb_" + slug(line), "thumb", [line]))

    # multiply / DSP family, ARM state only
    dsp = [
        "MULS R0, R1, R2", "MUL R0, R1, R2", "MLA R0, R1, R2, R3",
        "MLAS R0, R1, R2, R3",
        "UMULL R0, R1, R2, R3", "SMULL R0, R1, R2, R3",
        "UMLAL R0, R1, R2, R3", "SMLAL R0, R1, R2, R3",
        "UMULLS R0, R1, R2, R3", "SMLALS R0, R1, R2, R3",
        "QADD R0, R1, R2", "QSUB R0, R1, R2", "QDADD R0, R1, R2",
        "QDSUB R0, R1, R2",
        "SMULBB R0, R1, R2", "SMULTB R0, R1, R2", "SMULBT R0, R1, R2",
        "SMULTT R0, R1, R2",
        "SMLABB R0, R1, R2, R3", "SMLATB R0, R1, R2, R3",
        "SMLABT R0, R1, R2, R3", "SMLATT R0, R1, R2, R3",
        "SMULWB R0, R1, R2", "SMULWT R0, R1, R2",
        "SMLAWB R0, R1, R2, R3", "SMLAWT R0, R1, R2, R3",
        "SMLALBB R0, R1, R2, R3", "SMLALTT R0, R1, R2, R3",
        "SMLALTB R0, R1, R2, R3", "SMLALBT R0, R1, R2, R3",
        "CLZ R0, R1", "CLZ R0, R2", "CLZ R0, R3",
    ]
    for line in dsp:
        out.append(("arm_" + slug(line), "arm", [line]))

    # randomised operands: this is where flag bugs show up
    for i in range(n_each * 12):
        vals = [rnd.choice([0, 1, 0xFFFFFFFF, 0x80000000, 0x7FFFFFFF,
                            0x12345678, 0xFFFF0001, 0x8000, 0x7FFF,
                            rnd.getrandbits(32), rnd.getrandbits(32),
                            rnd.getrandbits(16) << 16, 1 << rnd.randrange(32)])
                for _ in range(4)]
        lines = list(rnd.sample(ops_arm, 4)) + list(rnd.sample(dsp, 2))
        out.append(("rnd_arith_%d" % i, "arm", lines, vals))
    for i in range(n_each * 8):
        vals = [rnd.choice([0, 1, 0xFFFFFFFF, 0x8000, rnd.getrandbits(32),
                            rnd.getrandbits(16), 0x7FFFFFFF, 0x80000000])
                for _ in range(4)]
        lines = list(rnd.sample(ops_thumb, 5))
        out.append(("rnd_thumb_arith_%d" % i, "thumb", lines, vals))
    return out


def slug(line):
    s = line.lower()
    for ch in " ,#[]!{}.;:":
        s = s.replace(ch, "_")
    while "__" in s:
        s = s.replace("__", "_")
    return s.strip("_")


def mem_cases():
    """Loads and stores of every shape.

    R1 is parked in the middle of the compared 256-byte data window so both
    machines see the access succeed: with an unmapped base every case
    degenerates into a data abort and proves nothing about the address or data
    path.  Stores deliberately use a source register other than R0 -- the store
    data lives in the Rd field for ARM and Thumb alike, and a decoder that
    reads the wrong nibble only shows up with a non-zero register number."""
    base = [0x12345678, DATA_BASE + 0x20, 4, 0x11112222,
            DATA_BASE + 0x40, DATA_BASE + 0x60, DATA_BASE + 0x70, 0x77778888,
            0x9999AAAA, 0xBBBBCCCC, 0x00000001, 0x00000002, 0x00000003,
            DATA_BASE + 0x100, DATA_BASE + 0x100, CODE_BASE]
    a = [
        "LDR R0, [R1]", "STR R0, [R1]", "LDRB R0, [R1]", "STRB R0, [R1]",
        "LDR R0, [R1, #4]", "LDR R0, [R1, #-8]", "STR R2, [R1, #16]!",
        "LDR R2, [R1, #4]!", "LDR R3, [R1], #8", "STR R3, [R1], #-4",
        "STR R5, [R1, #0x20]", "STRB R4, [R1, #0x18]", "LDR R6, [R1, #0x1C]",
        "LDR R0, [R1, R2, LSL #2]", "LDR R0, [R1, R2, ASR #1]",
        "LDR R0, [R1, R2, LSL R3]",
        "LDRH R0, [R1]", "LDRH R0, [R1, #4]", "LDRH R0, [R1, R2]",
        "STRH R0, [R1]", "STRH R2, [R1, #8]!", "LDRSH R0, [R1, R2]",
        "LDRSB R0, [R1, #2]", "LDRSH R3, [R1, #2]", "STRH R3, [R1, R2]",
        "LDRD R2, R3, [R1]", "LDRD R2, R3, [R1, #8]!", "STRD R2, R3, [R1, #4]",
        "LDR R0, [PC, #8]", "LDRB R0, [PC, #4]",
        "LDMIA R1!, {R0, R2-R4}", "LDMDB R1, {R0, R2-R4}",
        "LDMIA R1, {R0-R7}", "STMIA R1!, {R0, R2-R4}", "STMDB R1!, {R0-R3}",
        "STMIB R1!, {R2, R5}", "STMDA R1!, {R2, R3}", "STMIA R1!, {R4-R6}",
        "LDMIA R1, {R2, R3}!", "LDMFD R1!, {R4-R6}", "STMIA R1, {R5-R7}",
    ]
    t = [
        "LDR R0, [R1, #4]", "STR R0, [R1, #4]", "LDRB R0, [R1, #1]",
        "STRB R0, [R1, #3]", "LDR R0, [R1]", "LDR R0, [R1, R2]",
        "STR R0, [R1, R2]", "LDRB R0, [R1, R2]", "LDRH R0, [R1, #2]",
        "STRH R0, [R1, #6]", "LDRSH R0, [R1, #2]", "LDRSB R0, [R1, #1]",
        "LDRH R0, [R1, R2]", "STRH R0, [R1, R2]",
        "STR R2, [R1, #8]", "STRB R3, [R1, #0x14]", "STRH R4, [R1, #0x16]",
        "STR R5, [R1]", "LDR R7, [R1, #0x10]", "LDRB R6, [R1, #0x12]",
        "LDR R0, [PC, #8]", "LDR R0, [SP, #8]", "STR R0, [SP, #4]",
        "LDMIA R1!, {R0, R2, R3}", "STMIA R1!, {R0, R2, R3}",
        "STMIA R1!, {R2, R4, R5}", "LDR R3, [R1, R2]",
        "PUSH {R4, R5, LR}", "POP {R4, R5}", "PUSH {R0-R3}", "POP {R0-R3}",
        "PUSH {R4, R5}", "ADD R0, SP, #4", "POP {R4, R5}",
        "ADD SP, #0x1C", "SUB SP, #8",
    ]
    out = []
    for ln in a:
        out.append(("arm_" + slug(ln), "arm", [ln], base))
    for ln in t:
        lines_ = [ln]
        if "SP" in ln:
            # Establish the stack pointer *in the program*: unicorn only commits
            # a seeded R13 to the bank its TCG code actually reads at the end
            # of a translation block, so a one-instruction case that starts from
            # a seeded SP asks the two machines to disagree about a value that
            # never came from an instruction.  MOV SP, R4 puts it at 0x9040 --
            # inside the compared window -- on both sides.
            lines_.insert(0, "MOV SP, R4")
        out.append(("thumb_" + slug(ln), "thumb", lines_, base))
    for mode, lines_ in (("arm", ["MOV SP, R4", "PUSH {R0, R2}", "POP {R1, R3}"]),
                         ("thumb", ["MOV SP, R4", "PUSH {R0, R2}", "POP {R1, R3}",
                                    "ADD R1, SP, #0"])):
        out.append((mode + "_seq_push_pop", mode, lines_, base, None, len(lines_)))
    # a store to unmapped memory: the abort must leave the GPRs untouched
    bad = list(base)
    bad[3] = 0x40000000
    out.append(("arm_str_abort", "arm", ["STR R2, [R3, #4]"], bad))
    out.append(("thumb_str_abort", "thumb", ["STR R2, [R3, #4]"], bad))
    return out


def misc_cases():
    """PSR access, exchange branches, CLZ, swaps, and the v5T specials."""
    a = ["MRS R0, CPSR", "MRS R1, SPSR", "MRS R5, CPSR",
         "MSR CPSR_F, R1", "MSR CPSR_FC, R2", "MSR SPSR_C, R1",
         "CLZ R0, R1", "CLZ R0, R2", "CLZ R0, R3", "CLZ R5, R0",
         "SWP R0, R1, [R4]", "SWPB R2, R5, [R4, #1]", "SWP R6, R7, [R4, #4]",
         "NOP", "SVC 0x1234", "BX R3", "BLX R3",
         # MCR is here to prove the *decode* (a coprocessor transfer must not
         # be turned into an undefined instruction).  The matching MRC is left
         # out on purpose: what it returns is a CP15 *reset value*, which the
         # oracle does not take from the ARM926EJ-S TRM but from QEMU's own
         # default, so comparing it would encode somebody's guess.
         "MCR p15, 0, R0, c7, c5, 0",
         "MVN R0, R1", "MVNS R0, R1, LSL #3", "MOV R0, R1, RRX",
         "RSB R0, R1, R2, LSR #31", "TEQ R0, R1, LSL #1"]
    t = ["MOV R8, R1", "MOV R1, R8", "ADD R0, R8", "ADD R8, R0", "ADD R8, R9",
         "CMP R0, R8", "CMP R8, R0", "MOV R0, #0", "NEG R0, R1", "ASRS R0, R1, #1",
         "BKPT #1", "SVC #1", "CPS R0", "MVNS R0, R1", "MULS R0, R1",
         "LSLS R0, R1, #3", "LSRS R0, R1, #3", "ASRS R0, R1, #3",
         "ADDS R0, R1, R2", "SUBS R0, R1, R2", "ADD R3, R1, R2", "SUB R3, R1, R2",
         "ADD R0, R0, R0", "ADD R0, R0, R0, LSL #1"]
    out = []
    for ln in a:
        out.append(("arm_" + slug(ln), "arm", [ln]))
    for ln in t:
        out.append(("thumb_" + slug(ln), "thumb", [ln]))
    # flag-state driven conditional execution: every one of the 16 conditions
    for i in range(32):
        cpsr = (rnd_flags(i) << 28) | 0x10
        out.append(("arm_cond_%d" % i, "arm",
                    ["ADDS R0, R1, R2", "MOVEQ R3, #1", "ADDNE R3, R3, #2"],
                    None, cpsr, 3))
    return out


def rnd_flags(i):
    """All 16 N-Z-C-V combinations, then randomised repeats of them."""
    if i < 16:
        return ((i >> 3) & 1) << 31 | ((i >> 2) & 1) << 30 | \
               ((i >> 1) & 1) << 29 | (i & 1) << 28
    return 0xE + ((i * 7) & 7)


def branch_cases(rnd):
    """Branch coverage.

    Every target lands inside the loaded code and `count` is chosen so that
    both outcomes retire exactly the same number of instructions: the effect
    (which instruction ran) is visible in R0/R1, and the link value in R14, so
    no comparison against a PC is needed."""
    out = []
    # A branch case only proves anything if a *wrongly scaled* offset lands on
    # an instruction with a different effect, so the twelve slots below are
    # twelve distinct ADDs, one per word, and the targets are small signed
    # displacements inside that window: an offset scaled by 16 instead of 4 (a
    # real bug this corpus then caught) leaves a different R0 behind.
    slots = ["ADD R0, R0, #0x%x" % (i + 1) for i in range(12)]
    for disp in (1, 2, 3, 5, 8, 11):
        tgt = CODE_BASE + 8 + disp * 4
        out.append(("arm_b_fwd%d" % disp, "arm",
                    ["MOV R1, #1", "B 0x%x" % tgt] + slots, None, None, 3))
        bl = list(slots)
        bl[disp - 1] = "MOV R2, R14"        # slot the branch lands on
        out.append(("arm_bl_fwd%d" % disp, "arm", ["BL 0x%x" % tgt] + bl,
                    None, None, 3))
    # Backward branches, from the end of the block: a missing sign extension of
    # the offset lands far above the code instead of on a slot, and a target
    # scaled by 16 rather than 4 lands on a different ADD, so both are visible.
    back = ["MOV R1, #1"] + slots + ["B 0x%x" % (CODE_BASE + 4 + 4 * j)
                                     for j in (0, 3, 11)]
    for i, j in enumerate((0, 3, 11)):
        out.append(("arm_b_back%d" % j, "arm",
                    ["MOV R1, #1"] + slots + ["B 0x%x" % (CODE_BASE + 4 + 4 * j)],
                    None, None, 15))
        bl = ["MOV R1, #1"] + list(slots) + ["BL 0x%x" % (CODE_BASE + 4 + 4 * j)]
        bl[1 + j] = "MOV R2, R14"
        out.append(("arm_bl_back%d" % j, "arm", bl, None, None, 16))
    assert back
    out.append(("arm_bl_far", "arm",
                ["BL 0x%x" % (CODE_BASE + 8 + 0x100), "ADD R0, R0, #0x100",
                 "ADD R1, R1, #1", "ADD R1, R1, #2"], None, None, 3))
    # BLX immediate: ARM encoding, target bit[0] = Thumb.  Landing on a zero
    # word keeps the case deterministic (both models then execute `movs r0, r0`
    # there), so what is actually checked is the interworking itself: the T bit
    # set and R14 = the instruction after the branch, with bit[0] set.
    for i, disp in enumerate((0, 1, 2)):
        out.append(("arm_blxi_%d" % i, "arm",
                    ["ADD R0, R0, #0x100", "BLX 0x%x" % (CODE_BASE + 0x100 + 1),
                     "ADD R0, R0, #0x200"], None, None, 2))
    for cond in ("EQ", "NE", "CS", "CC", "MI", "PL", "VS", "VC", "HI", "LS",
                 "GE", "LT", "GT", "LE", "AL"):
        out.append(("arm_b%s" % cond, "arm",
                    ["MOV R1, #1", "B%s 0x%x" % (cond, CODE_BASE + 0xC),
                     "ADD R0, R0, #0x100", "ADD R0, R0, #1"], None, None, 3))
        # Two targets: CODE+8 is word aligned, CODE+0xA is only halfword
        # aligned.  A core that masks a Thumb branch target to 4 bytes (as the
        # ARM state rule requires) gets the second one wrong, and the slots carry
        # distinct ADDS immediates so the difference shows up in R0.
        for tgt in (0x8, 0xA):
            out.append(("thumb_b%s_%x" % (cond, tgt), "thumb",
                        ["MOV R1, #1", "B%s 0x%x" % (cond, CODE_BASE + tgt)] +
                        ["ADDS R0, R0, #0x%x" % (i + 1) for i in range(6)],
                        None, None, 3))
    # BX / BLX with the target in a register (R3 is preset to a code address)
    r3 = [0x12345678, 0x9ABCDEF0, 0xFFFFFFFF, CODE_BASE + 8, DATA_BASE,
          DATA_BASE + 0x40, DATA_BASE + 0x80, 0x11112222, 0x33334444,
          0x55556666, 0x77778888, 0x9999AAAA, 0xBBBBCCCC, DATA_BASE + 0x100,
          DATA_BASE + 0x100, CODE_BASE]
    for name, mode, lrn, cnt in [
        ("arm_bx_r3", "arm", ["BX R3", "ADD R0, R0, #0x100", "ADD R0, R0, #2",
                              "ADD R0, R0, #4"], 3),
        ("arm_blx_r3", "arm", ["BLX R3", "ADD R0, R0, #0x100", "MOV R2, R14",
                               "ADD R0, R0, #4"], 3),
        ("thumb_bx_r3", "thumb", ["BX R3", "ADDS R0, R0, #0x40", "ADDS R0, #2",
                                  "ADDS R0, #4"], 3),
    ]:
        out.append((name, mode, lrn, list(r3), None, cnt))
    for i in range(24):
        flags = rnd.randrange(16) << 28
        out.append(("rnd_bcond_%d" % i, "arm",
                    ["MOV R1, R0", "BEQ 0x%x" % (CODE_BASE + 0xC),
                     "ADD R0, R0, #1", "ADD R0, R0, #0x40"],
                    None, flags | 0x10, 3))
    return out


def multi_seq_cases(rnd):
    """Short realistic sequences (function prologue/epilogue style)."""
    seqs = [
        ["STMDB R13!, {R4-R8, LR}", "ADD R4, R0, R1", "MOV R5, R2",
         "LDR R6, [R3, #4]", "ADD R4, R4, R6", "SUB R5, R5, R4",
         "LDMIA R13!, {R4-R8, PC}"],
        ["MOV R0, #0", "MOV R1, #1", "ADD R0, R0, R1, LSL #2",
         "ADD R0, R0, R1, LSL #4", "MOV R1, R0, LSR #3",
         "ORR R0, R0, R1, LSL #8", "BIC R0, R0, #0xFF00"],
        ["MOV R2, #0x1000", "STR R1, [R2, #0]", "LDR R3, [R2, #0]",
         "SUB R3, R3, R1", "MOV R0, R3", "SWP R0, R4, [R2]"],
    ]
    out = []
    for i, sq in enumerate(seqs):
        out.append(("seq_arm_%d" % i, "arm", sq))
    tseqs = [
        ["PUSH {R4, R5, LR}", "MOVS R4, R0", "ADDS R4, R1",
         "LDR R5, [PC, #0x10]", "ADDS R4, R5", "POP {R4, R5, PC}"],
        ["MOVS R0, #0", "MOVS R1, #1", "ADDS R0, R1", "LSLS R0, R0, #4",
         "MOV R8, R0", "ADDS R0, R8", "MVNS R0, R0"],
    ]
    for i, sq in enumerate(tseqs):
        out.append(("seq_thumb_%d" % i, "thumb", sq))
    return out


# --------------------------------------------------------------------------
# oracle
# --------------------------------------------------------------------------

def run_unicorn(words, mode, regs, cpsr, data, nsteps):
    mu = Uc(UC_ARCH_ARM, UC_MODE_ARM if mode == "arm" else UC_MODE_THUMB)
    mu.mem_map(RAM_BASE, RAM_SIZE, UC_PROT_ALL)
    for i, w in enumerate(words):
        mu.mem_write(CODE_BASE + i * (4 if mode == "arm" else 2),
                     struct.pack("<H" if mode == "thumb" else "<I", w))
    mu.mem_write(DATA_BASE, bytes(data))
    # Writing CPSR makes QEMU reload its cached bank, so the GPR seeds have to
    # go in *after* it or R13/R14 are silently lost.
    # Unicorn picks the state from bit0 of the start address, so a Thumb run
    # must be entered with T set in CPSR *and* an odd address, otherwise QEMU
    # happily decodes the halfwords as ARM.
    if mode == "thumb":
        cpsr |= 0x20
    mu.reg_write(UC_ARM_REG_CPSR, cpsr)
    for i, v in enumerate(regs[:15]):
        if i not in (13, 14):
            mu.reg_write(R_REGS[i], v)
    # R13/R14 are re-asserted after the PC write: unicorn reloads the bank it
    # caches for the current mode when the PC (and therefore the T bit) moves,
    # and a banked SP written before that reload is lost.
    mu.reg_write(UC_ARM_REG_R15, CODE_BASE)
    for i in (13, 14):
        if i < len(regs):
            mu.reg_write(R_REGS[i], regs[i])
    start = CODE_BASE | (1 if mode == "thumb" else 0)
    # `end` is deliberately far away: it must not truncate the run, `count` does.
    mu.emu_start(start, CODE_BASE + 0x800, count=nsteps)
    out_regs = [mu.reg_read(R_REGS[i]) & 0xFFFFFFFF for i in range(16)]
    out_cpsr = mu.reg_read(UC_ARM_REG_CPSR) & 0xFFFFFFFF
    out_data = bytes(mu.mem_read(DATA_BASE, DATA_SIZE))
    return out_regs, out_cpsr, out_data


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", type=int, default=0, help="limit cases")
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__),
                                                   "data", "corpus.txt"))
    ap.add_argument("--seed", type=int, default=20261006)
    args = ap.parse_args()

    rnd = random.Random(args.seed)
    cases = []
    cases += arith_cases(rnd)
    cases += mem_cases()
    cases += misc_cases()
    cases += branch_cases(rnd)
    cases += multi_seq_cases(rnd)
    if args.cases:
        cases = cases[:args.cases]

    ka = Asm(KS_MODE_ARM, 4)
    kt = Asm(KS_MODE_THUMB, 2)
    blocks = []
    skipped = []
    for case in cases:
        name, mode, lines = case[0], case[1], case[2]
        regs = list(case[3]) if len(case) > 3 and case[3] else [
            0x12345678, 0x9ABCDEF0, 0xFFFFFFFF, 0x00008000,
            DATA_BASE, DATA_BASE + 0x40, DATA_BASE + 0x80, 0x11112222,
            0x33334444, 0x55556666, 0x77778888, 0x9999AAAA,
            0xBBBBCCCC, DATA_BASE + 0x100, DATA_BASE + 0x100, CODE_BASE]
        cpsr = case[4] if len(case) > 4 and case[4] is not None else (
            0x60000000 if mode == "thumb" else 0x60000000)
        nsteps = case[5] if len(case) > 5 else len(case[2])
        if any(re.match(r"^(\w+\s+)?(PC|R15)\b", ln) and
               not re.match(r"^(BX|BLX|POP|LDM|LDR)\b", ln) for ln in lines):
            skipped.append((name, "writes PC from a data value"))
            continue
        data = bytes([i & 0xFF for i in range(DATA_SIZE)])
        words = []
        try:
            k = ka if mode == "arm" else kt
            for ln in lines:
                b = k.asm(ln, addr=CODE_BASE + len(words) *
                          (4 if mode == "arm" else 2))
                if mode == "arm":
                    words.append(struct.unpack("<I", b[:4])[0])
                else:
                    words.append(struct.unpack("<H", b[:2])[0])
        except Exception as e:                                    # noqa: BLE001
            skipped.append((name, str(e)[:60]))
            continue
        # a couple of instructions to touch memory
        try:
            uregs, ucpsr, udata = run_unicorn(words, mode, regs, cpsr, data,
                                              nsteps)
        except Exception as e:                                    # noqa: BLE001
            skipped.append((name, "unicorn: " + str(e)[:60]))
            continue
        blk = ["BEGIN %s mode=%s count=%d" % (name, mode, nsteps),
               "CPSR 0x%08x" % cpsr,
               "REGS " + " ".join("%08x" % v for v in regs),
               "DATA " + " ".join("%02x" % b for b in data),
               "CODE " + " ".join("%08x" % w for w in words),
               "EXPECT_CPSR 0x%08x" % (ucpsr & REG_MASK),
               "EXPECT_REGS " + " ".join("%08x" % v for v in uregs),
               "EXPECT_DATA " + " ".join("%02x" % b for b in udata),
               "END"]
        blocks.append("\n".join(blk))

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w") as f:
        f.write("# brain-emu ISA corpus, generated by %s\n" %
                os.path.basename(__file__))
        f.write("# expectations from Unicorn; CPSR compared with mask 0x%08x\n"
                % REG_MASK)
        f.write("CASES %d\n" % len(blocks))
        for b in blocks:
            f.write(b + "\n")
    print("wrote %d cases to %s" % (len(blocks), args.out))
    if skipped:
        print("skipped %d (encoder could not produce an ARMv5 word):" %
              len(skipped))
        for n, why in skipped[:20]:
            print("   %-34s %s" % (n, why))
    return 0


if __name__ == "__main__":
    sys.exit(main())
