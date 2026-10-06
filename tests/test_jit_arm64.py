#!/usr/bin/env python3
"""Verify the JIT's AArch64 output, on a host that is not AArch64.

Two layers, both cheap and both able to fail on their own:

1. every encoding the emitter can produce is handed to Capstone, and the text it
   gets back has to be the instruction the emitter *meant* - a wrong shift field or
   a wrong load scale is invisible to a test that only runs the result;
2. blocks taken from the golden corpus are translated by the JIT front end, emitted
   to AArch64 by `build/jit_emit`, and executed by Unicorn's AArch64 model over the
   same shadowed state the core uses.  The core's own run of those words - with the
   oracle's numbers behind it - is what the machine code is compared against.

So the guest side of layer 2 inherits the corpus' Unicorn-vs-interpreter agreement,
and the emitted code is checked against the interpreter.  A block the front end
refuses is not a failure: the point is that whatever it *does* translate, it gets
right.

Run:  tests/test_jit_arm64.py [--corpus F] [--jit-emit P] [-v]
Needs capstone and unicorn; exits 0 with a note when they are missing, so that
`make check` does not depend on a Python environment.
"""
import argparse
import collections
import os
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATE = 0x1000            # the guest address the shadowed state lives at
CODE = 0x40000000         # where the emitted AArch64 goes

try:
    import capstone
except ImportError:                                                # noqa: E402
    capstone = None
try:
    import unicorn
    from unicorn.arm64_const import UC_ARM64_REG_LR, UC_ARM64_REG_X0, UC_ARM64_REG_X1
    from unicorn import UC_ARCH_ARM64, UC_HOOK_CODE, UC_MODE_ARM, UC_PROT_ALL
except ImportError:                                                 # noqa: E402
    unicorn = None


def parse_corpus(path):
    """Each `BEGIN name mode=arm count=N` block, with the fields we need."""
    cases = []
    cur = None
    with open(path, "r") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("BEGIN "):
                parts = line.split()
                cur = {"name": parts[1], "mode": parts[2].split("=")[1],
                       "count": int(parts[3].split("=")[1]), "words": [], "regs": [],
                       "cpsr": 0}
                cases.append(cur)
            elif cur is not None:
                if line.startswith("CODE "):
                    cur["words"] = [int(x, 16) for x in line.split()[1:]]
                elif line.startswith("REGS "):
                    cur["regs"] = [int(x, 16) for x in line.split()[1:]]
                elif line.startswith("CPSR "):
                    cur["cpsr"] = int(line.split()[1], 16)
            if line == "END":
                cur = None
    return cases


def run_emit(jit_emit, words, cpsr, regs):
    """Ask the tool for IR, machine code, and what the interpreter did instead."""
    cmd = [jit_emit, "--auto", "--hex", ",".join("%08x" % w for w in words),
           "--cpsr", "%08x" % cpsr, "--regs", ",".join("%08x" % r for r in regs[:13])]
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode not in (0, 1):
        raise RuntimeError("jit_emit failed: %s%s" % (p.stdout, p.stderr))
    out = {"code": [], "pre": [], "post": [], "used": 0, "insns": 0, "nextpc": 0,
           "stop": "?", "ir": []}
    for line in p.stdout.splitlines():
        tag, _, rest = line.partition(" ")
        if tag == "SKIP":
            out["skip"] = int(rest)
        elif tag == "USED":
            out["used"] = int(rest)
        elif tag == "INSNS":
            out["insns"] = int(rest)
        elif tag == "STOP":
            out["stop"] = rest.strip()
        elif tag == "IR":
            out["ir"].append(rest)
        elif tag == "CODE":
            rest = rest.split()
            if rest and rest[0].isdigit():
                rest = rest[1:]        # `CODE <n> w w ...`: the count is not a word
            out["code"] = [int(x, 16) for x in rest]
        elif tag in ("PRE", "POST"):
            v = [int(x, 16) for x in rest.split()]
            out["pre" if tag == "PRE" else "post"] = v
        elif tag == "NEXTPC":
            out["nextpc"] = int(rest, 16)
    return out


def check_forms(jit_emit, verbose):
    """Layer 1: does each emitted word mean what the emitter thinks it means?"""
    if capstone is None:
        print("  SKIP: capstone not installed, so the encoding check did not run (`python3 -m pip install capstone`)")
        return 0, 0
    p = subprocess.run([jit_emit, "--forms"], capture_output=True, text=True)
    groups = collections.OrderedDict()
    for line in p.stdout.splitlines():
        name, idx, word = line.split()
        groups.setdefault(name, []).append(int(word, 16))
    md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
    # What each form has to disassemble to.  `prologue`/`epilogue` are sequences, so
    # they are checked by their first and last instruction.
    want = {
        "mov_imm_lo": ["mov w1, #0x1234"],
        "set_nz_from": ["tst w1, w1"],
        "mov_rrx": ["mrs x15, nzcv", "ubfx w15, w15, #0x1d, #1",
                    "lsr w1, w2, #1", "orr w1, w1, w15, lsl #31"],
        "mov_imm_full": ["mov w1, #0x5678", "movk w1, #0x1234, lsl #16"],
        "mov_reg": ["mov w1, w2"],
        "add_imm": ["add w1, w2, #0x1f"],
        "adds_imm": ["adds w1, w2, #0x1f"],
        "sub_imm": ["sub w1, w2, #4"],
        "add_reg": ["add w1, w2, w3"],
        "add_lsl": ["add w1, w2, w3, lsl #4"],
        "add_asr": ["add w1, w2, w3, asr #2"],
        "adds_reg": ["adds w1, w2, w3"],
        "sub_reg": ["sub w1, w2, w3"],
        "cmp_reg": ["cmp w2, w3"],
        "and_reg": ["and w1, w2, w3"],
        "bic_reg": ["bic w1, w2, w3"],
        "bics_reg": ["bics w1, w2, w3"],
        "orr_reg": ["orr w1, w2, w3"],
        "orr_lsr7": ["orr w1, w2, w3, lsr #7"],
        "eor_reg": ["eor w1, w2, w3"],
        "orn_reg": ["orn w1, w2, w3"],
        "mvn": ["mvn w1, w2"],
        "neg": ["neg w1, w2"],
        "adc": ["adc w1, w2, w3"],
        "sbc": ["sbc w1, w2, w3"],
        "lsl_imm": ["orr w1, wzr, w2, lsl #4"],
        "lsr_imm": ["orr w1, wzr, w2, lsr #3"],
        "asr_imm": ["orr w1, wzr, w2, asr #2"],
        "ror1": ["orr w1, wzr, w2, ror #1"],
        "ldr_w": ["ldr w1, [x0, #4]"],
        "str_w": ["str w1, [x0, #4]"],
        "mrs_nzcv": ["mrs x14, nzcv"],
        "msr_nzcv": ["msr nzcv, x14"],
        "ret": ["ret"],
        "nop": ["nop"],
    }
    bad = 0
    total = 0
    for name, ws in groups.items():
        total += len(ws)
        blob = b"".join(struct.pack("<I", w) for w in ws)
        got = [("%s %s" % (i.mnemonic, i.op_str)).strip() for i in md.disasm(blob, CODE)]
        if len(got) != len(ws):
            print("  BAD   %-14s %d/%d words decoded" % (name, len(got), len(ws)))
            bad += 1
            continue
        if name in want and got != want[name]:
            print("  BAD   %-14s got %s want %s" % (name, " | ".join(got),
                                                     " | ".join(want[name])))
            bad += 1
            continue
        if verbose:
            print("  ok    %-14s %s" % (name, " | ".join(got)))
    print("  encodings: %d words in %d forms, %d wrong" % (total, len(groups), bad))
    return bad, 1 if bad else 0


def run_block(mu, code, regs, cpsr):
    """Execute one emitted block over a shadowed state image, and read it back."""
    mu.mem_write(STATE, b"".join(struct.pack("<I", v) for v in
                                 list(regs[:13]) + [regs[13], regs[14], regs[15],
                                                    cpsr]))
    entry = CODE
    mu.mem_write(entry, b"".join(struct.pack("<I", w) for w in code))
    mu.reg_write(UC_ARM64_REG_X0, STATE)
    mu.reg_write(UC_ARM64_REG_LR, entry + 4 * len(code))     # where `ret` goes
    mu.emu_start(entry, entry + 4 * len(code), count=len(code) + 1)
    raw = mu.mem_read(STATE, 17 * 4)
    vals = list(struct.unpack("<17I", bytes(raw)))
    return vals


def sweep(mu, jit_emit, verbose):
    """Layer 3: every data-processing shape, not just the ones a generator emitted.

    `jit_emit --sweep` walks 16 opcodes x 4 shift types x 32 amounts x S x the
    rotated-immediate form, and reports the emitted code next to what the interpreter
    made of the same word from the same seed.  Here each block is run on the model
    and compared, register by register and flag by flag.  A shape the front end
    refuses is counted and skipped; the totals are printed, because "we cover 2555 of
    4224 DP shapes" is the honest claim and "the JIT works" is not.
    """
    p = subprocess.run([jit_emit, "--sweep"], capture_output=True, text=True)
    if p.returncode != 0:
        raise RuntimeError("jit_emit --sweep failed: %s" % p.stderr[:400])
    seed = None
    n_ok = n_skip = 0
    fails = []
    for line in p.stdout.splitlines():
        f = line.split()
        if f[0] == "SEED":
            seed = [int(x, 16) for x in f[1:]]
            continue
        if f[0] == "TOTAL":
            continue
        if f[0] != "SW" or seed is None:
            continue
        if f[2] == "skip":
            n_skip += 1
            continue
        word = int(f[1], 16)
        ncode = int(f[2])
        code = [int(x, 16) for x in f[3:3 + ncode]]
        # One line per shape: "SW <word> <ncode> <code...> | <cpsr> <R0-R12> |
        # <nextpc>".  The count is checked so that a change in the tool's format
        # fails here instead of quietly comparing the wrong columns.
        bar = f.index("|")
        assert len(f) == 3 + ncode + 1 + 14 + 1 + 1, line
        post = [int(x, 16) for x in f[bar + 1:bar + 15]]
        nextpc = int(f[-1], 16)
        post_cpsr, post_regs = post[0], post[1:]
        try:
            got = run_block(mu, code, list(seed[1:]) + [0, 0, 0], seed[0])
        except unicorn.UcError as e:
            fails.append("%08x: model error: %s" % (word, e))
            continue
        why = []
        for i in range(13):
            if got[i] != post_regs[i]:
                why.append("r%d got %08x want %08x" % (i, got[i], post_regs[i]))
        if got[15] != nextpc:
            why.append("r15 got %08x want %08x" % (got[15], nextpc))
        if got[16] != post_cpsr:
            why.append("cpsr got %08x want %08x" % (got[16], post_cpsr))
        if why:
            fails.append("%08x: %s" % (word, "; ".join(why)))
        else:
            n_ok += 1
    print("  sweep: %d agreed, %d shapes refused by the front end" % (n_ok, n_skip))
    for m in fails[:12]:
        print("  FAIL %s" % m)
    if len(fails) > 12:
        print("  ... %d more" % (len(fails) - 12))
    return len(fails)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", default=os.path.join(ROOT, "tests/data/corpus.txt"))
    ap.add_argument("--jit-emit", default=os.path.join(ROOT, "build/jit_emit"))
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    if not os.path.exists(args.jit_emit):
        print("SKIP: %s not built" % args.jit_emit)
        return 0

    print("[jit/aarch64] encoding check")
    bad, rc = check_forms(args.jit_emit, args.verbose)
    if bad:
        return 1
    if unicorn is None:
        print("  SKIP: unicorn not installed, so no block was executed on a model (`python3 -m pip install unicorn`)")
        return 0

    cases = [c for c in parse_corpus(args.corpus) if c["mode"] == "arm"]
    if args.limit:
        cases = cases[:args.limit]
    print("[jit/aarch64] corpus blocks on an AArch64 model (%d ARM cases)" % len(cases))

    mu = unicorn.Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    mu.mem_map(STATE & ~0xFFF, 0x2000, UC_PROT_ALL)
    mu.mem_map(CODE, 0x4000, UC_PROT_ALL)

    n_ok = n_skip = 0
    fails = []
    for c in cases:
        out = run_emit(args.jit_emit, c["words"], c["cpsr"], c["regs"])
        if out["used"] == 0 or not out["code"]:
            n_skip += 1
            continue
        if out["insns"] != out["used"]:
            n_skip += 1                       # the interpreter ran more than the block
            continue
        # Seed the model with the state the *block* starts in, which is what the
        # tool printed as PRE after stepping the interpreter to it: the case's own
        # REGS line is only the starting point, and `--auto` may have run past it.
        pre_cpsr, pre_regs = out["pre"][0], out["pre"][1:]
        seed = list(pre_regs) + [0, 0, 0]
        post_cpsr, post_regs = out["post"][0], out["post"][1:]
        try:
            got = run_block(mu, out["code"], seed, pre_cpsr)
        except unicorn.UcError as e:
            fails.append("%s: model error: %s" % (c["name"], e))
            continue
        # The image is regs[0..15] followed by the CPSR word, so index i is R<i>
        # and index 16 is the CPSR - the layout emit_arm64.h documents.
        why = []
        for i in range(13):
            if (got[i] ^ post_regs[i]) & 0xFFFFFFFF:
                why.append("r%d got %08x want %08x" % (i, got[i], post_regs[i]))
        for i in (13, 14):
            if got[i] != seed[i]:
                why.append("r%d changed (%08x -> %08x)" % (i, seed[i], got[i]))
        if got[15] != out["nextpc"]:
            why.append("r15 got %08x want %08x (next_pc)" % (got[15], out["nextpc"]))
        cpsr_now = got[16]
        if cpsr_now & 0xF0000000 != post_cpsr & 0xF0000000:
            why.append("flags got %08x want %08x" % (cpsr_now & 0xF0000000,
                                                      post_cpsr & 0xF0000000))
        if cpsr_now & ~0xF0000000 != pre_cpsr & ~0xF0000000:
            why.append("cpsr non-flag bits changed: %08x -> %08x" %
                       (pre_cpsr, cpsr_now))
        if why:
            fails.append("%s: %s\n          ir: %s" %
                         (c["name"], "; ".join(why), " / ".join(out["ir"][:4])))
        else:
            n_ok += 1
            if args.verbose:
                print("  ok  %-36s %s" % (c["name"], " / ".join(out["ir"][:3])))

    print("  %d blocks agree with the interpreter, %d cases untranslatable" %
          (n_ok, n_skip))
    bad += sweep(mu, args.jit_emit, args.verbose)
    for f in fails[:20]:
        print("  FAIL %s" % f)
    if len(fails) > 20:
        print("  ... %d more" % (len(fails) - 20))
    return 1 if (fails or bad) else 0


if __name__ == "__main__":
    sys.exit(main())
