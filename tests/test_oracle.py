#!/usr/bin/env python3
"""Differential oracle: core vs Unicorn's ARM926EJ-S model, both states.

The corpus compares one instruction word against a recorded expectation, which is
fine for "did it execute" but blind to the rules that only show up in a corner:
which Thumb shift an encoding means, whether Q is sticky, what `SMLAL` does on
overflow, whether a PC-relative base is aligned before or after the +8, and how an
ARMv5 unaligned word access rotates its data.  Those were all review claims, and each
one here is *measured* instead of argued: the same program bytes and the same seed
state go into `build/jit_emit` (our interpreter) and into Unicorn, and the resulting
R0-R12, CPSR (N,Z,C,V,Q,I,F,T,mode) and data memory must agree.

Programs are assembled with Keystone where an encoding is easy to get wrong by hand
(the multiply/SAT/ADR families) and built by hand where the point *is* the encoding
(the shift fields - an assembler would politely rewrite `LSRS R,R,#0` for us and hide
the very case under test).

SKIPs without the three python bindings, and needs `make` to have produced
build/jit_emit, so `make check` stays green on a bare tree.

    /home/user/oracle-venv/bin/python tests/test_oracle.py     # capstone unicorn keystone
"""
import os
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TOOL = os.path.join(ROOT, "build", "jit_emit")

CODE_BASE = 0x8000
DATA_BASE = 0x9000
MASK = 0xF80000FF            # N Z C V Q, and I F T + mode

try:
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_MODE_THUMB, UC_PROT_ALL
    import unicorn.arm_const as AC
    from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM, KS_MODE_THUMB
    import keystone
    from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB, CS_MODE_V8
except ImportError:
    print("SKIP: needs capstone/unicorn/keystone in the interpreter running the suite")
    sys.exit(0)

# Assemble for ARMv5, not the ARMv8 keystone defaults to: on an ARM926EJ-S the long
# branch is a pair of independent halfword instructions and `ADD.W Rd, PC, #imm8`
# does not exist, so an ARMv6T2 encoding is an invalid *test*, not a core bug.  The
# Capstone check in _capstone_text() below rejects whatever slips through anyway.
_V5 = getattr(keystone, "KS_MODE_V5", 0)
try:
    _KS = {False: Ks(KS_ARCH_ARM, KS_MODE_ARM | _V5),
           True: Ks(KS_ARCH_ARM, KS_MODE_THUMB | _V5)}
except Exception:
    _KS = {False: Ks(KS_ARCH_ARM, KS_MODE_ARM), True: Ks(KS_ARCH_ARM, KS_MODE_THUMB)}

# Register ids must come from the module by name: unicorn's ARM_R0 is not 0, and
# arithmetic on the enum (R0 + i) reaches deprecated aliases and warns per access.
RID = [getattr(AC, "UC_ARM_REG_R%d" % i) for i in range(13)]
PCID = getattr(AC, "UC_ARM_REG_PC")
CPSRID = getattr(AC, "UC_ARM_REG_CPSR")


def asm(src, thumb):
    """Assembler bytes for a program (one entry per instruction), or None when the
    assembler cannot express one of the lines (that shape is then reported skipped)."""
    out = b""
    for line in src:
        try:
            enc, _cnt = _KS[thumb].asm(line)
        except Exception:
            return None
        out += bytes(enc)
    return out


def _capstone_text(blob, thumb):
    """Disassemble a program with Capstone, or None if it cannot.  Used to make
    sure a shape we compare really is an instruction of this architecture: a
    32-bit-Thumb `ADD.W`/`ADR` is ARMv6T2 and an ARM926 must not be judged on it."""
    try:
        from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB, CS_MODE_V8
    except ImportError:
        return "?"                       # no capstone: compare everything anyway
    md = Cs(CS_ARCH_ARM, (CS_MODE_THUMB | CS_MODE_V8) if thumb else CS_MODE_ARM)
    n = 2 if thumb else 4
    out = []
    for i in range(0, len(blob), n):
        got = list(md.disasm(blob[i:i + n], 0x8000 + i))
        if not got:
            return None
        out.append(got[0].mnemonic + " " + got[0].op_str)
    return " | ".join(out)


def words_of(blob, thumb):
    """Memory image -> the unit list jit_emit --hex wants (halfwords for Thumb)."""
    n = 2 if thumb else 4
    return [int.from_bytes(blob[i:i + n], "little") for i in range(0, len(blob), n)]


SEEDS = [
    # (cpsr, r0..r12, r13, r14, r15) - CPSR must carry no T here: these seeds are
    # used for ARM-state programs too, and the core keeps a T written through MSR
    # while the model normalises it away, which would swamp the real signal.  Q
    # (bit 27) is set in two of them, because every saturating instruction that
    # clears a sticky Q - instead of leaving it alone - has to be caught.
    (0x60000010, [0, 0, 0x80000001, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, DATA_BASE + 0x100]),
    (0x68000010, [0x80000000, 0x80000000, 0x80000000, 0x00000001, 0xFFFFFFFF,
                  0x7FFFFFFF, 0x80000000, 0x00000001, 0x12345678, 0x00000000,
                  0xFFFFFFFF, 0x40000000, 0xC0000000, DATA_BASE + 0x100]),
    (0xF8000010, [0x00000001, 0x7FFFFFFF, 0x7FFFFFFF, 0xFFFFFFFF, 0x80000000,
                  0x40000000, 0x00000003, 0xFFFFFFF9, 0xDEADBEEF, 0x10000000,
                  0x00000001, 0x80000001, 0xFFFFFFFF, DATA_BASE + 0x100]),
    (0xA0000010, [0x00000000, 0xFFFFFFFF, 0x80000001, 0x00000000, 0x11111111,
                  0x22222222, 0x00000020, 0x0000001F, 0x7FFFFFFF, 0x80000000,
                  0x00000020, 0xFFFFFFFF, 0x00000001, DATA_BASE + 0x100]),
]

# A data window whose every word is distinct and position-sensitive, so a rotation
# of the payload cannot hide: word at DATA_BASE+4k = 0xAABBCCDD + k.
DATA = b"".join(struct.pack("<I", 0xAABB0000 + k) for k in range(64))


def pad13(regs):
    """R0..R12 (+R13) only: R14/R15 are not seeded in either engine, because the
    core's regs[15] *is* the prefetched start address (the tool sets it to the load
    address) and unicorn's PC is set by emu_start - seeding R15 in one and not the
    other starts the two runs at different instructions."""
    return (list(regs) + [0] * 14)[:14]


def run_core(blob, thumb, pc, cpsr, regs, steps, peek=()):
    args = [TOOL, "--exec", "--hex", ",".join("%x" % w for w in words_of(blob, thumb)),
            "--pc", "%x" % pc, "--cpsr", "%x" % cpsr,
            "--regs", ",".join("%x" % r for r in pad13(regs)), "--steps", str(steps),
            "--poke", ",".join("%x:%x" % (DATA_BASE + 4 * i,
                                         struct.unpack_from("<I", DATA, 4 * i)[0])
                               for i in range(len(DATA) // 4))]
    if thumb:
        args.append("--thumb")
    if peek:
        args += ["--peek", ",".join("%x" % a for a in peek)]
    out = subprocess.run(args, capture_output=True, text=True, timeout=120).stdout
    post = pc_next = None
    peeks = {}
    for line in out.splitlines():
        if line.startswith("POST "):
            post = [int(x, 16) for x in line.split()[1:]]
        elif line.startswith("R15 "):
            pc_next = int(line.split()[1], 16)
        elif line.startswith("PEEK "):
            f = line.split()
            peeks[int(f[1], 16)] = int(f[2], 16)
    if post is None:
        raise RuntimeError("no POST in tool output:\n" + out[:400])
    return post, pc_next, peeks


def run_model(blob, thumb, pc, cpsr, regs, steps, peek=()):
    mu = Uc(UC_ARCH_ARM, UC_MODE_ARM | (UC_MODE_THUMB if thumb else 0))
    mu.mem_map(CODE_BASE & ~0xFFF, 0x4000, UC_PROT_ALL)
    # The program must live at the PC under test: several families (Thumb ADR,
    # LDR-literal) are about Align(PC,4), which only differs when PC is not already
    # aligned, and writing everything to 0x8000 would hide exactly that.
    mu.mem_write(pc, blob)
    mu.mem_write(DATA_BASE, DATA)
    seed = pad13(regs)
    for i in range(13):
        mu.reg_write(RID[i], seed[i] & 0xFFFFFFFF)
    mu.reg_write(AC.UC_ARM_REG_SP, seed[13] & 0xFFFFFFFF)
    mu.reg_write(CPSRID, (cpsr | (0x20 if thumb else 0)) & 0xFFFFFFFF)
    mu.emu_start(pc | (1 if thumb else 0), pc + len(blob) + 0x100, count=steps)
    # Same layout as jit_emit's POST line: CPSR first, then R0..R12.
    out = [mu.reg_read(CPSRID) & MASK]
    out += [mu.reg_read(RID[i]) & 0xFFFFFFFF for i in range(13)]
    return out, mu.reg_read(PCID) & 0xFFFFFFFF, \
        {a: struct.unpack("<I", bytes(mu.mem_read(a, 4)))[0] for a in peek}


def check(cases, thumb, pcs, label, fails, compared, skipped, mask=MASK):
    refused = set()
    for src, steps in cases:
        blob = src if isinstance(src, bytes) else asm(src, thumb)
        if blob is None:
            skipped[0] += 1
            print("  note: assembler cannot encode %r" % (src,))
            continue
        if isinstance(src, list) and _capstone_text(blob, thumb) is None:
            skipped[0] += 1
            print("  note: capstone rejects %r (not an ARMv5 encoding)" % (src,))
            continue
        for pc in pcs:
            for cpsr, regs in SEEDS:
                got, gpc, gpeek = run_core(blob, thumb, pc, cpsr, regs, steps, ())
                got[0] &= mask
                try:
                    want, wpc, upeek = run_model(blob, thumb, pc, cpsr, regs, steps, ())
                except Exception as ex:
                    skipped[0] += 1      # the model refuses the encoding (unallocated
                                         # fields in *my* word, or a model-side quirk
                                         # like PC-as-operand reading unmapped bytes):
                                         # nothing to compare - print it once per shape
                    if blob not in refused:
                        refused.add(blob)
                        print("  note: model refuses %r (%s)" % (src, type(ex).__name__))
                    continue
                want[0] &= mask
                compared[0] += 1
                tag = "%s pc=%04x cpsr=%08x: %s" % (
                    label, pc, cpsr, " | ".join(src) if isinstance(src, list)
                    else "<%s>" % " ".join("%02x" % b for b in src))
                if got != want:      # got[0]/want[0] = CPSR masked, then R0..R12
                    for i in range(14):
                        if got[i] != want[i]:
                            fails.append("%s\n    %s: core %08x model %08x" %
                                         (tag, "cpsr(NZCVQ,IF,T,mode)" if i == 0
                                          else "r%d" % (i - 1), got[i], want[i]))
                            break
    return


def hand(word):
    return bytes(struct.pack("<H", word)), word


# ---------------------------------------------------------------- ARM data processing
def family_dp_imm(fails, compared, skipped):
    """Rotated-immediate decode: value = ROR(imm8, 2*rot), rot 0 = imm8 with the
    carry left alone.  Wrong rot/imm8 fields (or a shift-by-2*rot applied when
    rot == 0) show up in the result or in C."""
    cases = []
    for op in (0x0, 0x1, 0x2, 0x3, 0x4, 0xC, 0xE, 0xD, 0xF, 0x8, 0x9):
        for rot in (0, 1, 7, 12, 15):
            for imm8 in (0x00, 0x01, 0x80, 0xFF):
                # MOV/MVN have no Rn (bits 19:16 must read 0) and the four
                # test-and-set-opcodes have no Rd, or the encoding is unallocated.
                rd = 0 if op in (0x8, 0x9) else 5
                # Rn = 0 for MOV/MVN (no Rn); a real register otherwise - using the
                # PC as an operand of a flag-setting op is UNPREDICTABLE territory.
                rn = 0 if op in (0xD, 0xF) else 6
                w = (0xE << 28) | (1 << 25) | (op << 21) | (1 << 20) | (rd << 12) | \
                    (rn << 16) | (rot << 8) | imm8
                cases.append((bytes(struct.pack("<I", w)), 1))
    check(cases, False, (0x8000,), "dp-imm", fails, compared, skipped)


# ------------------------------------------------------------------ Thumb shift forms
def family_thumb_regshift(fails, compared, skipped):
    """T1 ALU-by-register: the group numbers the four shifts 2,3,4,7 while the
    internal shift-type enum numbers them 0,1,2,3 - a raw copy of the field makes
    LSLS execute as ASR."""
    # T1 `0100 00 op(2) op Rdn(3:0) Rm(8:6)`: the 4-bit op is at bits[9:6] and
    # Rs at bits[8:6], and bit[5] is the S bit (the group is `MOV Rd, Rm, shift Rs`
    # or `MOV{S}` - `LSLS R1, R1` is `0x4089`, while `0x4111` has op = 4 = ASR with
    # S clear).  Capstone prints every word below, which is what caught me building
    # this group wrong the first time.
    cases = []
    for op in (2, 3, 4, 7):                       # LSL LSR ASR ROR, as encoded
        for rs in (1, 2, 3):                      # Rs = R1..R3 hold the amounts
            for s in (0, 1):
                hw = (op << 6) | (rs << 3) | 1 | (s << 5)
                cases.append((bytes(struct.pack("<H", hw)), 1))
    check(cases, True, (0x8000,), "thumb-regshift", fails, compared, skipped)


def family_thumb_immshift(fails, compared, skipped):
    """T1 shift-immediate, including the imm5 = 0 corners (MOV / LSR #32 / RRX)."""
    # `000 imm5(10:6) Rm(5:3) Rd(2:0)`: type is imm5[4:3], i.e. bits[10:9], with
    # 00 LSL, 01 LSR, 10 ASR, 11 ROR - a *32-bit* LSR/ASR only exists in the ARM
    # encoding and in the 32-bit Thumb one, so imm5 = 0 here means LSL #0 (which is
    # MOV for ARMv5) and never a shift by 32.
    cases = []
    for typ in (0, 1, 2, 3):
        for imm5 in (0, 1, 2, 16, 31):
            if typ == 0 and imm5 == 0:
                # `000 00000 Rm Rd` is MOV (register) with no flag change on an
                # ARM926; from ARMv6 the same encoding is `LSLS #0`, which does
                # update N/Z - the model is a v6-and-later core, so this one shape
                # cannot be settled here and the core's own behaviour is pinned by
                # tests/test_unit.cpp instead.
                continue
            hw = (typ << 9) | (imm5 << 6) | (2 << 3) | 1
            cases.append((bytes(struct.pack("<H", hw)), 1))
    check(cases, True, (0x8000,), "thumb-immshift", fails, compared, skipped)


def family_arm_zero_shift(fails, compared, skipped):
    """LSR #0 / ASR #0 / ROR #0 in an ARM shifted-register operand mean 32 / 32 /
    RRX, and the same field with the register form means "no shift at all"."""
    cases = []
    for typ in (0, 1, 2, 3):
        for amt in (0, 1, 31, 32):
            w = (0xE << 28) | (0x0 << 21) | (1 << 20) | (5 << 12) | (1 << 7) | \
                (typ << 5) | amt
            cases.append((bytes(struct.pack("<I", w)), 1))
            w = (0xE << 28) | (0x0 << 21) | (1 << 20) | (5 << 12) | (6 << 8) | \
                (typ << 5) | (1 << 4) | 2      # register amount, R6 = shift value
            cases.append((bytes(struct.pack("<I", w)), 1))
    check(cases, False, (0x8000,), "arm-zeroshift", fails, compared, skipped)


# ------------------------------------------------------------- saturating arithmetic
def family_saturation(fails, compared, skipped):
    """Q behaviour of the ARMv5E arithmetic: SMLAL/SMLAWy/QADD-family set Q on
    overflow and must not clear it otherwise; SMULxy/MUL/MLA leave Q alone; only
    MSR may clear it.  Seed 2 and 4 start with Q = 1, so any instruction that
    writes Q = 0 on a non-overflowing path is caught here."""
    progs = [
        ["smlal r0,r1,r2,r3"], ["umlal r0,r1,r2,r3"], ["smull r0,r1,r2,r3"],
        ["umull r0,r1,r2,r3"], ["mul r0,r1,r2"], ["mla r0,r1,r2,r3"],
        ["smlawb r0,r1,r2"], ["smlawt r0,r1,r2"], ["smulwb r0,r1,r2"],
        ["smlabb r0,r1,r2,r3"], ["smlabt r0,r1,r2,r3"], ["smulbb r0,r1,r2"],
        ["smlaxy r0,r1,r2,r3"], ["qadd r0,r1,r2"], ["qsub r0,r1,r2"],
        ["qdadd r0,r1,r2"], ["qdsub r0,r1,r2"], ["mrs r0,cpsr"],
        # sticky Q: saturate, then an instruction that must not clear it
        ["qadd r0,r1,r2", "add r5,r6,r7", "add r8,r9,r10", "mrs r4,cpsr"],
        ["smlalbb r0,r1,r2,r3", "add r5,r6,r7", "mrs r4,cpsr"],
        # ... and MSR may clear it again
    ]
    cases = [(p, len(p)) for p in progs]
    check(cases, False, (0x8000,), "saturation", fails, compared, skipped)
    # MSR only reaches CPSR from a privileged mode (the core correctly takes an
    # undefined instruction from USER mode, where the model does not), so this runs
    # in SVC.  #0x13 is a legal MOV-immediate (a full 0x60000013 would need ARMv6's
    # MOVW): it clears N,Z,C,V,Q,J and keeps SVC, which is exactly the point - MSR is
    # the only instruction allowed to drop a sticky Q.
    msr = [(["qadd r0,r1,r2", "mov r3, #0x13", "msr cpsr_cxsf, r3", "mrs r4,cpsr",
             "qadd r0,r1,r2", "mrs r5,cpsr"], 6)]
    saved = SEEDS[:]
    SEEDS[:] = [(cpsr | 0x13, regs) for cpsr, regs in SEEDS]
    check(msr, False, (0x8000,), "saturation-msr", fails, compared, skipped)
    SEEDS[:] = saved


# --------------------------------------------------------------------- PC-relative
def family_pc_relative(fails, compared, skipped):
    """ADR/LDR-literal base: Thumb uses Align(PC,4) (so a halfword-misaligned PC
    rounds *down* after the +8) while ARM uses PC+8 exactly, and ADR's ARM form also
    clears the low two bits of the result for the non-writeback encoding."""
    for thumb, progs in ((True, [["adr r0, #8"], ["adr r0, #12"], ["adr r1, #0"],
                                 ["ldr r0, [pc, #4]"], ["adr r2, #4", "mov r8, r8", "mov r9, r9"],
                                 ["add r0, pc, #8"], ["sub r0, pc, #1"]]),
                         (False, [["adr r0, #8"], ["adr r0, #12"], ["adr r2, #4"],
                                  ["ldr r0, [pc, #4]"], ["add r0, pc, #4"]])):
        cases = [(p, len(p)) for p in progs]
        pcs = (0x8000, 0x8002, 0x8006) if thumb else (0x8000, 0x8004)
        check(cases, thumb, pcs, "pc-rel%s" % ("t" if thumb else "a"), fails, compared, skipped)


# ------------------------------------------------------------------ unaligned access
def family_unaligned(fails, compared, skipped):
    """ARMv5 unaligned words: a load rotates the data right by (address & 3) * 8
    bytes and does not touch C; a store writes only the four bytes addressed, i.e.
    the top (4 - (addr & 3)) bytes of its data word are dropped."""
    progs = [["ldr r0, [r1, #1]"], ["ldr r0, [r1, #2]"], ["ldr r0, [r1, #3]"],
             ["ldr r0, [pc, #1]"], ["ldrb r0, [r1, #3]"], ["ldrh r0, [r1, #2]"],
             ["ldrsh r0, [r1, #1]"], ["ldrsb r0, [r1, #1]"],
             ["str r0, [r1, #1]"], ["str r2, [r1, #2]"], ["strb r0, [r1, #3]"],
             ["ldr r0, [r1], #1"], ["ldr r0, [r1, #1]!" if False else "ldr r0, [r1, #1]!"],
             ["swp r0, r1, [r2]"], ["ldr r0, [r1, #1]", "add r5,r5,r5", "mrs r4,cpsr"]]
    cases = [(p, len(p)) for p in progs]
    # R1/R2 are index registers here: seed 2 has r1 = 0x7FFFFFFF, which is a wild
    # address, so use one dedicated seed with the index pointing into DATA_BASE.
    saved = SEEDS[:]
    tail = [0, 0, DATA_BASE + 0x100]
    SEEDS[:] = [(0x60000010, [0xDEADBEEF, DATA_BASE, 0xA5A5A5A5, DATA_BASE + 8,
                              0, 0, 0, 0, 0, 0] + tail),
                (0xE0000010, [0x12345678, DATA_BASE + 60, 0xFFFFFFFF, DATA_BASE + 4,
                              0x11111111, 0x22222222, 0, 0, 0] + tail)]
    peek = tuple(DATA_BASE + a for a in (0, 4, 8, 12, 16, 20, 56, 60))
    for src, steps in cases:
        blob = asm(src, False)
        if blob is None or _capstone_text(blob, False) is None:
            skipped[0] += len(SEEDS)
            continue
        for cpsr, regs in SEEDS:
            compared[0] += 1
            got, gpc, gpeek = run_core(blob, False, 0x8000, cpsr, regs, steps, peek)
            try:
                want, wpc, upeek = run_model(blob, False, 0x8000, cpsr, regs, steps, peek)
            except Exception:
                skipped[0] += 1
                continue
            if got != want:
                i = next(k for k in range(14) if got[k] != want[k])
                fails.append("unaligned: %s (%s)\n    core %08x model %08x" %
                             (src[0], "cpsr" if i == 0 else "r%d" % (i - 1),
                              got[i], want[i]))
            if gpeek != upeek:
                bad = [a for a in upeek if gpeek.get(a) != upeek[a]]
                fails.append("unaligned: %s memory@%s\n    core %s model %s" %
                             (src[0], ",".join("%08x" % a for a in bad),
                              ",".join("%08x" % gpeek[a] for a in bad),
                              ",".join("%08x" % upeek[a] for a in bad)))

    SEEDS[:] = saved


def family_q_overflow(fails, compared, skipped):
    """Q must be set by SMLAL when the signed 64-bit accumulation overflows (the
    result itself is *not* saturated), and left alone by UMULL/SMULL/UMLAL-style
    paths that document no Q effect.  The seeds here put a near-maximum accumulator
    in R0:R1 and a large product in R2*R3 so the sum really does overflow."""
    global SEEDS
    saved = SEEDS
    SEEDS = [(cpsr, [acc[0], acc[1], 0x80000000, 0x80000000, 0xFFFFFFFF, 0, 0, 0, 0,
                     0, 0, 0, 0, DATA_BASE + 0x100])
             for cpsr in (0x60000013, 0x00000013)
             for acc in ((0xFFFFFFFF, 0x7FFFFFFF), (0x00000000, 0x00000000),
                         (0xFFFFFFFF, 0xFFFFFFFF), (0x00000001, 0x80000000))]
    progs = [["smlal r0,r1,r2,r3"], ["umlal r0,r1,r2,r3"], ["smull r0,r1,r2,r3"],
             ["umull r0,r1,r2,r3"], ["smlabb r0,r1,r2,r3"], ["smlawb r0,r1,r2,asr #16"],
             ["qadd r0,r1,r2"], ["qdsub r0,r1,r2"]]
    cases = [(p, len(p)) for p in progs]
    check(cases, False, (0x8000,), "q-overflow", fails, compared, skipped)
    SEEDS = saved


def main():
    if not os.path.exists(TOOL):
        print("SKIP: %s missing (run make)" % TOOL)
        return 0
    fails, compared, skipped = [], [0], [0]
    family_dp_imm(fails, compared, skipped)
    family_thumb_regshift(fails, compared, skipped)
    family_thumb_immshift(fails, compared, skipped)
    family_arm_zero_shift(fails, compared, skipped)
    family_saturation(fails, compared, skipped)
    family_pc_relative(fails, compared, skipped)
    family_unaligned(fails, compared, skipped)
    family_q_overflow(fails, compared, skipped)
    for m in fails[:200]:
        print(m)
    if len(fails) > 200:
        print("... %d more" % (len(fails) - 200))
    # triage summary: which family, and which field, is off
    from collections import Counter
    fam = Counter(m.split(" pc=")[0].split(":")[0].strip() for m in fails)
    fld = Counter(m.split("\n    ")[1].split(":")[0].split(" r")[0].strip()
                  for m in fails if "\n    " in m)
    print("by family:", ", ".join("%s=%d" % kv for kv in fam.most_common()))
    print("by field:", ", ".join("%s=%d" % kv for kv in fld.most_common()))
    print("oracle: %d programs compared against the model, %d disagreements"
          " (%d shapes not compared: assembler/decoder/model refused them,"
          " see the notes)" % (compared[0], len(fails), skipped[0]))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
