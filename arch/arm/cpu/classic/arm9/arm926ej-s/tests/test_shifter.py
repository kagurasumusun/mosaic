#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-brain-emu-1.0
"""Barrel-shifter differential test: the shifter in cpu/ops.h and its encodings.

`MOV{S} Rd, Rm, <shift>` is the purest probe of the shifter there is: the result is
the shifted operand and the carry is whatever the shifter says.  Comparing that
against Unicorn's ARM model pins shift_arm() line for line, and pins the *decoder's*
re-encoding of the five-bit amount field, which is where the interesting rules live:

  * `LSL #0` is no shift at all and leaves C alone - the only shift whose zero field
    means zero, so decode_op2_reg() drops the shift entirely (OP2_REG).
  * `LSR #0` and `ASR #0` are *not* shifts by zero: the field that would be 0 encodes
    a 32-bit shift, so LSR gives zero with C from bit 31 and ASR gives the sign with
    C from bit 31 (ARM DDI 0084D s4.5.2 says this outright: "the form of the shift
    field which might be expected to correspond to LSR #0 is used to encode LSR #32,
    which has a zero result with bit 31 of Rm as the carry output"; DDI 0406 A1.8.1
    agrees).  The core therefore hands the shifter an amount of 32, not 0.
  * `ROR #0` *is* a different operation: RRX, a 33-bit rotate through the carry, and
    it is spelled by the ROR type with a zero amount.  The decoder names that SH_RRX
    so no caller has to know the trick.
  * A shift amount taken from a register is *not* re-encoded at all: `LSR Rm, R0`
    with R0 = 0 is a shift by zero (C unchanged), and amounts above 31 saturate for
    the shifts and fold as (n - 1) MOD 32 + 1 for the rotate, which is why the
    register matrix below goes up to 255.

This is a *core* test: the same shift_arm() feeds the interpreter and the JIT front
end, so a divergence here is a bug in the emulator rather than in a translation.
Needs unicorn; skips (exit 0) without it, or without a built build/jit_emit.
"""

import os
import struct
import subprocess
import sys

def _find_tool():
    """Find the repo's `jit_emit`, which lives in the flat `build/` directory.

    The suite is driven by `make check` (which has just built the tools, and so
    sets BRAIN_REQUIRE_TOOLS: a missing tool then has to fail, or a green run
    would mean nothing) and by hand from anywhere in the tree.  Walk up from this
    file to the first ancestor that has one.
    """
    d = os.path.dirname(os.path.abspath(__file__))
    while True:
        cand = os.path.join(d, "build", "jit_emit")
        if os.path.exists(cand):
            return cand
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    if os.environ.get("BRAIN_REQUIRE_TOOLS"):
        print("FAIL: jit_emit is not built (run `make tools`)")
        sys.exit(1)
    print("SKIP: jit_emit is not built (run `make`)")
    sys.exit(0)

TOOL = _find_tool()
REPO = os.path.dirname(os.path.dirname(TOOL))   # where `build/` lives

RAM_BASE, RAM_SIZE = 0x00000000, 0x00020000
CODE_BASE = 0x00008000

try:
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_PROT_ALL
    from unicorn.arm_const import UC_ARM_REG_R0, UC_ARM_REG_R15, UC_ARM_REG_CPSR
except ImportError:                                     # pragma: no cover
    print("  SKIP: unicorn not installed, so the shifter was not compared with a "
          "model (`python3 -m pip install unicorn`)")
    sys.exit(0)

try:
    from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM
    _cs = Cs(CS_ARCH_ARM, CS_MODE_ARM)
except ImportError:                                     # pragma: no cover
    _cs = None

R_REGS = list(range(UC_ARM_REG_R0, UC_ARM_REG_R0 + 16))

NAMES = ("LSL", "LSR", "ASR", "ROR")

# `MOV{S} Rd, Rm, shift #imm5` and `MOV{S} Rd, Rm, shift Rs`:
#   cond 00 I=0 opcode S Rn Rd  Rm[3:0] type[6:5] L[4] amount[11:7] / Rs[11:8]
# The L bit picks "immediate amount" (0) from "amount in a register" (1), and the
# register form leaves bit 7 clear because Rs starts at bit 8.
DP_MOV = 13 << 21
S_BIT = 1 << 20
USER = 0x10


def word_imm(shift_type, amount, rm=1, rd=0, s=True):
    return ((0xE << 28) | DP_MOV | (S_BIT if s else 0) | (rd << 12) | (rm & 0xF) |
            (shift_type & 3) << 5 | (amount & 0x1F) << 7)


def word_reg(shift_type, rs, rm=1, rd=0, s=True):
    return ((0xE << 28) | DP_MOV | (S_BIT if s else 0) | (rd << 12) | (rm & 0xF) |
            (shift_type & 3) << 5 | (1 << 4) | (rs & 0xF) << 8)


def run_model(word, regs, cpsr):
    mu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    mu.mem_map(RAM_BASE, RAM_SIZE, UC_PROT_ALL)
    mu.mem_write(CODE_BASE, struct.pack("<I", word))
    # CPSR first: writing it makes QEMU reload the bank it caches for the current
    # mode, which is what would drop an R13/R14 seeded before it.
    mu.reg_write(UC_ARM_REG_CPSR, cpsr)
    for i, v in enumerate(regs[:15]):
        if i not in (13, 14):
            mu.reg_write(R_REGS[i], v)
    mu.reg_write(UC_ARM_REG_R15, CODE_BASE)
    mu.emu_start(CODE_BASE, CODE_BASE + 4, count=1)
    return ([mu.reg_read(R_REGS[i]) & 0xFFFFFFFF for i in range(13)],
            mu.reg_read(UC_ARM_REG_CPSR) & 0xFFFFFFFF)


def run_core(word, regs, cpsr):
    """One instruction of the core, read back through the JIT tool's PRE/POST dump.

    --block matters: it runs the word through the *interpreter* and prints the state
    afterwards, without asking the JIT front end to translate it first.  The front end
    legitimately refuses several shapes probed here (notably `MOV/MVN Rd, Rm, shift
    #n` with S, and any shift amount of 32), and in the translate-first mode a refusal
    stopped the run before the instruction - the test then compared untouched state
    against the model and only looked correct because both sides read zero."""
    args = [TOOL, "--exec", "--steps", "1", "--hex", "%08x" % word, "--cpsr", "%08x" % cpsr,
            "--regs", ",".join("%08x" % v for v in (list(regs) + [0] * 16)[:13])]
    p = subprocess.run(args, capture_output=True, text=True, cwd=REPO)
    # Status 1 is the JIT front end refusing to translate the word, which says
    # nothing about the interpreter run whose result is being read here.  (Same
    # convention as test_jit_aarch64.py in this directory.)
    if p.returncode not in (0, 1):
        raise RuntimeError("jit_emit failed: %s%s" % (p.stdout, p.stderr))
    post = None
    for line in p.stdout.splitlines():
        if line.startswith("POST "):
            post = line.split()
    if post is None or len(post) != 15:
        raise RuntimeError("unexpected jit_emit output for %08x:\n%s" % (word, p.stdout))
    return [int(x, 16) for x in post[2:]], int(post[1], 16)


def disasm(word):
    if _cs is None:
        return "%08x" % word
    for insn in _cs.disasm(struct.pack("<I", word), CODE_BASE):
        return "%s %s" % (insn.mnemonic, insn.op_str)
    return "<undecoded>"


def main():
    # Seeds chosen so that every rule about *which bit* the carry comes from is
    # distinguishable: 0x00000001 has bit 0 only, 0x80000000 bit 31 only.
    rm_seeds = (0x00000001, 0x80000000, 0x12345678)
    shapes = []
    for typ in range(4):
        for amt in (0, 1, 2, 3, 15, 16, 30, 31):
            for rm in rm_seeds:
                for cin in (0, 1):
                    shapes.append(("%s #%d" % (NAMES[typ], amt),
                                   word_imm(typ, amt, rm), rm, cin, None))
        for amt in (0, 1, 31, 32, 33, 64, 255):        # Rs-supplied: no re-encoding
            for rm in rm_seeds[:2]:
                for cin in (0, 1):
                    shapes.append(("%s R2=%d" % (NAMES[typ], amt),
                                   word_reg(typ, 2, rm), rm, cin, amt))
    for typ in range(4):                                # S=0: flags must not move
        for amt in (0, 31):
            shapes.append(("%s #%d, S=0" % (NAMES[typ], amt),
                           word_imm(typ, amt, 0x80000001, s=False), 0x80000001, 1,
                           None))

    bad = 0
    for tag, word, rm, cin, rs_amt in shapes:
        regs = [0] * 16
        regs[1] = rm
        if rs_amt is not None:
            regs[2] = rs_amt
        cpsr = USER | (0x20000000 if cin else 0)
        want_regs, want_cpsr = run_model(word, regs, cpsr)
        got_regs, got_cpsr = run_core(word, regs, cpsr)
        why = []
        for i in range(13):
            if got_regs[i] != want_regs[i]:
                why.append("r%d got %08x want %08x" % (i, got_regs[i], want_regs[i]))
        # N, Z, C, V only: Q/A/E/I/F/T are the model's own affair here, and none of
        # these instructions can change them.
        if (got_cpsr ^ want_cpsr) & 0xF0000000:
            why.append("cpsr got %08x want %08x" % (got_cpsr & 0xF0000000,
                                                    want_cpsr & 0xF0000000))
        if why:
            bad += 1
            print("  FAIL %-16s %08x [%s]: %s" % (tag, word, disasm(word),
                                                  "; ".join(why)))
            if bad > 20:
                print("  ... stopping after 20")
                break
    print("  shifter: %d encodings compared with the model, %d disagreements"
          % (len(shapes), bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
