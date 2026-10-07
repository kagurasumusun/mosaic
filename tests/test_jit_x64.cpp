/* The x86-64 backend, executed natively.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The AArch64 emitter is checked against a model (tests/test_jit_arm64.py, with
 * Unicorn).  On x86-64 the machine *is* the host, so the check is stronger and
 * simpler: two machines are built with the same code and the same state, one is
 * run through the block cache (which executes the emitted bytes) and the other
 * through the interpreter, and every visible difference is a bug in the
 * backend.  The interpreter is the reference of record here - it is what the
 * golden corpus (tests/data/corpus.txt) is checked against.
 *
 * Four things are tested, in increasing distance from a single instruction:
 *
 *   - the encoder, byte for byte, against the encodings the manuals give (a
 *     backend bug that produces a *different but valid* instruction is invisible
 *     to a behavioural test only if the instruction happens to compute the same
 *     thing, so the encodings are pinned directly as well);
 *   - the shadow ABI: the offsets the generated code hard-codes, checked against
 *     `offsetof` so a layout change cannot silently move a guest register;
 *   - one instruction at a time, swept over every data-processing opcode, both
 *     flag settings, every shift type and a spread of shift amounts and rotated
 *     immediates, over three entry flag sets;
 *   - random blocks of up to seven instructions over all thirteen guest
 *     registers, which is what exercises the register allocator (spilling,
 *     eviction, the reuse of an operand register as the destination, the carry
 *     stash) and the lazy flag materialisation over sequences, plus a block that
 *     deliberately holds more values live than there are host registers.
 *
 * Everything runs through `jit_cache_translate` + `jit_cache_enter`, i.e. the
 * same entry points the dispatcher uses, including the shadow handoff and the
 * flag merge - so a mistake in the ABI shows up here too.
 */
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "arm926/cpu.h"
#include "arm926/jit/cache.h"
#include "arm926/jit/jit.h"
#include "arm926/jit/x64_asm.h"

namespace {

using namespace brain;
using namespace brain::arm926;
using namespace brain::arm926::x64;

int g_checks = 0;
int g_failed = 0;

void check(bool ok, const std::string& what) {
  ++g_checks;
  if (!ok) {
    ++g_failed;
    std::printf("  FAIL: %s\n", what.c_str());
  }
}

/* ---------------------------------------------------------------- encoder -- */

std::string hex(const std::vector<uint8_t>& b) {
  static const char* d = "0123456789abcdef";
  std::string s;
  s.reserve(b.size() * 2);
  for (uint8_t v : b) {
    s += d[v >> 4];
    s += d[v & 15];
  }
  return s;
}

void enc(const char* what, const Buf& b, const std::vector<uint8_t>& want) {
  const std::string g = hex(b.w), w = hex(want);
  check(g == w, std::string(what) + ": encoded " + g + ", expected " + w);
}

/* `std::vector<uint8_t>` from a brace list without repeating the type. */
std::vector<uint8_t> bytes(std::initializer_list<int> v) {
  std::vector<uint8_t> out;
  for (int x : v) out.push_back((uint8_t)x);
  return out;
}

void test_encoder() {
  std::printf("[x64 encoder]\n");
  Buf b;

  b = Buf(); b.mov_rr(RAX, RCX);           enc("mov eax, ecx", b, bytes({0x8B, 0xC1}));
  b = Buf(); b.mov_rr(R8, R9);             enc("mov r8d, r9d", b, bytes({0x45, 0x8B, 0xC1}));
  b = Buf(); b.mov_ri(RAX, 0x12345678);    enc("mov eax, imm32", b,
                                               bytes({0xB8, 0x78, 0x56, 0x34, 0x12}));
  b = Buf(); b.mov_rm(RAX, Mem::bx(RBP, 4)); enc("mov eax, [rbp+4]", b,
                                                 bytes({0x8B, 0x45, 0x04}));
  b = Buf(); b.mov_mem_r(Mem::bx(RBP, 0x44), RAX);
  enc("mov [rbp+0x44], eax", b, bytes({0x89, 0x45, 0x44}));
  b = Buf(); b.mov_rr64(RBP, RDI);         enc("mov rbp, rdi", b, bytes({0x48, 0x8B, 0xEF}));
  b = Buf(); b.push64(RBP);                enc("push rbp", b, bytes({0x55}));
  b = Buf(); b.push64(R15);                enc("push r15", b, bytes({0x41, 0x57}));
  b = Buf(); b.pop64(R15);                 enc("pop r15", b, bytes({0x41, 0x5F}));
  b = Buf(); b.ret();                      enc("ret", b, bytes({0xC3}));
  b = Buf(); b.lea(RAX, Mem::bxi(RCX, RDX, 4, 8));
  enc("lea eax, [rcx+rdx*4+8]", b, bytes({0x8D, 0x44, 0x91, 0x08}));
  b = Buf(); b.alu_rr(A_ADD, RAX, RCX);    enc("add eax, ecx", b, bytes({0x01, 0xC8}));
  b = Buf(); b.alu_rr(A_SUB, R8, R9);      enc("sub r8d, r9d", b, bytes({0x45, 0x29, 0xC8}));
  b = Buf(); b.alu_ri(A_ADD, RSP, 8);      enc("add esp, 8", b, bytes({0x83, 0xC4, 0x08}));
  b = Buf(); b.alu_ri(A_CMP, RAX, 0x1000); enc("cmp eax, 0x1000", b,
                                               bytes({0x81, 0xF8, 0x00, 0x10, 0x00, 0x00}));
  b = Buf(); b.test_rr(RAX, RCX);          enc("test eax, ecx", b, bytes({0x85, 0xC8}));
  b = Buf(); b.alu_rr(A_SBB, R8, R9);      enc("sbb r8d, r9d", b, bytes({0x45, 0x19, 0xC8}));
  b = Buf(); b.not_(RAX);                  enc("not eax", b, bytes({0xF7, 0xD0}));
  b = Buf(); b.neg(RAX);                   enc("neg eax", b, bytes({0xF7, 0xD8}));
  b = Buf(); b.shift_ri(S_SHL, RAX, 5);    enc("shl eax, 5", b, bytes({0xC1, 0xE0, 0x05}));
  b = Buf(); b.shift_ri(S_SAR, R8, 31);    enc("sar r8d, 31", b,
                                               bytes({0x41, 0xC1, 0xF8, 0x1F}));
  b = Buf(); b.shift_ri(S_RCR, RAX, 1);    enc("rcr eax, 1", b, bytes({0xD1, 0xD8}));
  b = Buf(); b.setcc(C_B, R11);            enc("setb r11b", b,
                                               bytes({0x41, 0x0F, 0x92, 0xC3}));
  b = Buf(); b.setcc(C_O, R15);            enc("seto r15b", b,
                                               bytes({0x41, 0x0F, 0x90, 0xC7}));
  b = Buf(); b.bt_ri(R11, 0);              enc("bt r11d, 0", b,
                                               bytes({0x41, 0x0F, 0xBA, 0xE3, 0x00}));
  b = Buf(); b.stc();                      enc("stc", b, bytes({0xF9}));
  b = Buf(); b.clc();                      enc("clc", b, bytes({0xF8}));
  b = Buf(); b.cmc();                      enc("cmc", b, bytes({0xF5}));
  b = Buf(); b.pushfq();                   enc("pushfq", b, bytes({0x9C}));
  b = Buf(); b.popfq();                    enc("popfq", b, bytes({0x9D}));
  b = Buf(); b.push_mem(Mem::bx(RBP, 0x44));
  enc("push qword [rbp+0x44]", b, bytes({0xFF, 0x75, 0x44}));
  b = Buf(); b.mov_mem_i(Mem::bx(RBP, 0x4C), 0);
  enc("mov dword [rbp+0x4C], 0", b, bytes({0xC7, 0x45, 0x4C, 0x00, 0x00, 0x00, 0x00}));

  /* The awkward addressing forms: RBP/R13 bases need an explicit displacement,
   * R13 bases need a SIB byte even at [reg+0], and RSP as an index means "no
   * index" (so the encoder must never emit RSP there by accident). */
  b = Buf(); b.mov_rm(RCX, Mem::bxi(R13, R14, 1, 0));
  enc("mov ecx, [r13+r14]", b, bytes({0x43, 0x8B, 0x4C, 0x35, 0x00}));
  b = Buf(); b.alu_ri(A_SUB, R12, 1);      enc("sub r12d, 1", b,
                                               bytes({0x41, 0x83, 0xEC, 0x01}));
  b = Buf(); b.mov_rm(RDX, Mem::bx(R13, 8)); enc("mov edx, [r13+8]", b,
                                                 bytes({0x41, 0x8B, 0x55, 0x08}));
}

/* --------------------------------------------------------------- machines -- */

constexpr uint32_t kCode = 0x8000;
constexpr uint32_t kRamSize = 1u << 20;

struct Rng {
  uint32_t s;
  explicit Rng(uint32_t seed) : s(seed ? seed : 1) {}
  uint32_t next() { s ^= s << 13; s ^= s >> 17; s ^= s << 5; return s; }
  uint32_t below(uint32_t n) { return next() % n; }
  bool one_in(uint32_t n) { return below(n) == 0; }
};

/* How many instructions the sweeps actually retired on the host.  A sweep that
 * fell back to the interpreter everywhere would compare two interpreters and
 * pass, which is exactly the failure mode this guards against (test_instr does
 * the same with its `--jit` self-check). */
uint64_t g_native = 0;

/* Translate `code` in machine `a`, run it through the cache, run the same words
 * through the interpreter in `b`, and report the first difference.  Returns
 * false when the block was not translatable (nothing to compare). */
bool compare_block(const uint32_t* code, uint32_t n, const uint32_t* regs,
                   uint32_t cpsr, const char* tag, int nregs = 13,
                   uint32_t* npc_out = nullptr) {
  CpuConfig cfg;
  cfg.ram_size = kRamSize;
  /* The Jazelle trap is a *policy* knob, not an ISA property: with it on (the
   * default, and what the CLI runs with) the frontend refuses every `bx`,
   * because the instruction can raise an undefined-instruction trap that only
   * the interpreter reports.  From this file's point of view that just means
   * nothing to compare, so the trap is off and the register branch is exercised
   * for real - the state switch it performs (bit 0 of the target) and the exit
   * path that writes it back are the interesting part. */
  cfg.jazelle_trap = false;
  Cpu a, b;
  if (!a.init(cfg) || !b.init(cfg)) return false;
  a.reset();
  b.reset();
  for (uint32_t i = 0; i < n; ++i) {
    a.phys_write(kCode + i * 4, code[i], 4);
    b.phys_write(kCode + i * 4, code[i], 4);
  }
  for (int i = 0; i < nregs; ++i) a.regs[i] = b.regs[i] = regs[i];
  a.cpsr = b.cpsr = cpsr;
  a.vflag = b.vflag = (cpsr >> 28) & 1u;
  for (Cpu* c : {&a, &b}) {
    c->npc = kCode; c->cur = kCode; c->regs[15] = kCode + 8;
    c->bank[3].r[15] = kCode + 8;
  }

  JitCache* jc = jit_cache_create(&a, 1);
  if (!jc) return false;
  JitBlock* blk = nullptr;
  const bool ok = jit_cache_translate(jc, kCode, &blk) && blk && blk->insns;
  if (!ok) { jit_cache_destroy(jc); return false; }
  const uint32_t retired = blk->insns;
  /* The dispatcher, not one raw `enter`: a block that bails on an access - the
   * address left the identity RAM window, or a store landed on a page holding
   * translated code - hands that instruction back to the interpreter and
   * resumes after it.  Anything less would compare the JIT's *partial* work
   * against the interpreter's whole run and call the difference a bug. */
  jit_cache_run_insns(jc, retired);
  g_native += a.stats.insns_jit;
  b.run_insns(retired);
  jit_cache_destroy(jc);

  if (npc_out) *npc_out = b.npc;
  bool bad = false;
  std::string why;
  /* R0-R14: a translated access may write the stack pointer (a shadow-base
   * writeback) and a `bl` writes the link, so both are outputs a backend can
   * get wrong.  R15 is left out for the same reason test_instr leaves it out -
   * it is the *pipeline* value, refreshed by the next fetch. */
  for (int i = 0; i < 15 && !bad; ++i) {
    if (a.regs[i] != b.regs[i]) {
      char t[128];
      std::snprintf(t, sizeof t, "R%d %08x != %08x", i, a.regs[i], b.regs[i]);
      why = t;
      bad = true;
    }
  }
  if (!bad && a.cpsr != b.cpsr) {
    char t[128];
    std::snprintf(t, sizeof t, "CPSR %08x != %08x", a.cpsr, b.cpsr);
    why = t;
    bad = true;
  }
  if (!bad && a.npc != b.npc) {
    char t[128];
    std::snprintf(t, sizeof t, "npc %08x != %08x", a.npc, b.npc);
    why = t;
    bad = true;
  }
  /* The cycle model is part of what both engines promise (`bench` reads it, and
   * the dispatcher's budget is denominated in it), so a block whose cost is not
   * what the interpreter would have charged is a bug even when the registers
   * agree. */
  if (!bad && a.cycles != b.cycles) {
    char t[128];
    std::snprintf(t, sizeof t, "cycles %llu != %llu", (unsigned long long)a.cycles,
                  (unsigned long long)b.cycles);
    why = t;
    bad = true;
  }
  /* Memory too: a store that landed at the wrong address, or a load that
   * clobbered what it read, is invisible in the register file. */
  if (!bad && std::memcmp(a.ram, b.ram, kRamSize) != 0) {
    for (uint32_t off = 0; off < kRamSize; off += 4) {
      if (std::memcmp(a.ram + off, b.ram + off, 4) != 0) {
        char t[128];
        std::snprintf(t, sizeof t, "mem[%05x] %08x != %08x", off,
                      *(const uint32_t*)(a.ram + off), *(const uint32_t*)(b.ram + off));
        why = t;
        break;
      }
    }
    bad = true;
  }
  if (bad) {
    std::string msg = std::string(tag) + ": " + std::to_string(retired) +
                      " insns: " + why + " |";
    for (uint32_t i = 0; i < n; ++i) {
      char t[16];
      std::snprintf(t, sizeof t, " %08x", code[i]);
      msg += t;
    }
    char t[64];
    std::snprintf(t, sizeof t, " | cpsr-in %08x", cpsr);
    msg += t;
    for (int i = 0; i < 13; ++i) {
      std::snprintf(t, sizeof t, " r%d=%08x", i, regs[i]);
      msg += t;
    }
    check(false, msg);
  }
  return true;
}

/* One random data-processing word.  Deliberately includes encodings the front
 * end refuses (conditional forms, register-specified shifts, SP/PC operands,
 * ADC with a shift, flag-setting immediates): those must end the block, and the
 * *prefix* that does translate has to stay correct.  RSC was one of them until
 * the backend learned it, which is why it is worth having both: the words keep
 * being generated either way, so the day it became translatable it was covered
 * by a battery that had been running against it for as long as it existed. */
uint32_t random_dp(Rng* r) {
  uint32_t w = (r->one_in(16) ? r->below(15) : 0xEu) << 28;
  w |= r->below(16) << 21;                                  /* opcode */
  w |= (r->one_in(2) ? 1u : 0u) << 20;                      /* S      */
  w |= (r->one_in(8) ? 13u + r->below(3) : r->below(13)) << 16;   /* rn */
  w |= (r->one_in(8) ? 13u + r->below(3) : r->below(13)) << 12;   /* rd */
  if (r->below(10) < 3) {
    w |= 1u << 25;
    w |= (r->one_in(3) ? 0u : r->below(16)) << 8;           /* rot */
    w |= r->below(256);                                     /* imm8 */
  } else {
    w |= ((r->one_in(4) ? 0u : r->below(32)) << 7);         /* shift amount */
    w |= (r->below(4) << 5);                                /* shift type   */
    w |= (r->one_in(8) ? 13u + r->below(3) : r->below(13));  /* rm */
    if (r->one_in(8)) w |= 1u << 4;                         /* Rs: refused  */
  }
  return w;
}

/* -------------------------------------------------------- the shadow ABI --- */

/* The shadow buffer is the one interface between plain C++ and machine code,
 * and the offsets in it are hard-coded on the generated side - which is a
 * compiler cannot check for you.  The test therefore hands the block a buffer
 * filled with a sentinel and looks at what it left behind: an access the
 * emitter and jit/abi.h disagree about shows up as a clobbered word, and the
 * values in the words it *should* have written pin the layout itself. */
void test_shadow_layout() {
  using namespace brain;
using namespace brain::arm926;
  std::printf("[x64 shadow ABI]\n");
  check(SH_R0 == 0x00 && SH_R15 == 0x3C, "the register file is the first 16 words");
  check(SH_CPSR == 0x40 && SH_EFLAGS == 0x44 && SH_VFLAG == 0x48,
        "the CPSR word and the two flag halves");
  check(SH_ABORT_PC == 0x4C && SH_WORDS * 4 > SH_ABORT_PC, "the exit words fit");

  CpuConfig cfg;
  cfg.ram_size = kRamSize;
  Cpu cpu;
  if (!cpu.init(cfg)) return;
  /* `subs r1, r2, r3` then `movs r4, r1, lsl #2`: a subtraction (whose V comes
   * out of `seto` and whose C comes out of host CF, inverted) feeding a logical
   * instruction, which then passes V through - so both halves of the flag ABI
   * and the r15 pass-through are exercised in two instructions. */
  static const uint32_t kWords[2] = {0xE0521003u, 0xE1B04101u};
  for (uint32_t i = 0; i < 2; ++i) cpu.phys_write(kCode + i * 4, kWords[i], 4);
  cpu.npc = cpu.cur = kCode;
  cpu.regs[15] = kCode + 8;
  for (int i = 0; i < 13; ++i) cpu.regs[i] = 0x10000000u + (uint32_t)i;
  cpu.regs[2] = 0x80000000u;
  cpu.regs[3] = 1u;
  cpu.cpsr = 0xF0000010u;   /* every flag set: a flag that is forgotten cannot
                             * be mistaken for one that was passed through. */
  cpu.vflag = 1;

  JitCache* jc = jit_cache_create(&cpu, 1);
  check(jc != nullptr, "the cache is created");
  if (!jc) return;
  JitBlock* blk = nullptr;
  const bool ok = jit_cache_translate(jc, kCode, &blk) && blk && blk->insns == 2;
  check(ok, "the sample block translates");
  if (!ok || !jc->arena_exec) {
    jit_cache_destroy(jc);
    return;
  }
  std::vector<uint32_t> s(SH_WORDS, 0xDEADBEEFu);
  for (int i = 0; i < 13; ++i) s[i] = cpu.regs[i];
  s[16] = cpu.cpsr;                     /* the CPSR word the dispatcher copies */
  s[SH_EFLAGS / 4] = 0x202;             /* an EFLAGS image: IF, carry clear      */
  s[SH_VFLAG / 4] = 1;                  /* ...and V set, so the entry V is real  */
  void (*fn)(uint32_t*) = (void (*)(uint32_t*))(jc->arena + blk->entry);
  fn(s.data());

  /* r1 = 0x80000000 - 1 = 0x7FFFFFFF: C set (no borrow), V set (the signed
   * result does not fit), N and Z clear.  r4 = 0x7FFFFFFF << 2 = 0xFFFFFFFC
   * with N set, C from bit 30 of r1, and V carried over from the subtraction. */
  const uint32_t f = s[SH_EFLAGS / 4];
  check(s[SH_VFLAG / 4] == 1u, "V is a whole word in the shadow");
  check((f & 0x0080) != 0, "SF carries N");
  check((f & 0x0040) == 0, "ZF carries Z");
  check((f & 0x0001) != 0, "CF carries C");
  check(s[(SH_R0 + 4 * 1) / 4] == 0x7FFFFFFFu, "R1 lands in the register file");
  check(s[(SH_R0 + 4 * 4) / 4] == 0xFFFFFFFCu, "R4 too");
  check(s[SH_R15 / 4] == kCode + 8, "R15 is the address the block stopped at");
  check(s[SH_ABORT_PC / 4] == 0, "a clean run leaves the abort word zero");
  bool untouched = true;
  for (int i = 0; i < 13; ++i)
    if (i != 1 && i != 4 && s[i] != cpu.regs[i]) untouched = false;
  check(untouched, "no register the block does not write is touched");
  check(s[13] == 0xDEADBEEFu && s[14] == 0xDEADBEEFu,
        "nor the two words past R12, which a block may not name");
  check(s[SH_CPSR / 4] == cpu.cpsr,
        "the block leaves the CPSR word to the dispatcher");
  jit_cache_destroy(jc);
}

/* ------------------------------------------------------- the DP sweep ----- */

/* Every data-processing opcode, both flag settings, immediates (rotated and
 * not), plain register operands and every shift type with a spread of amounts -
 * each entered with three different flag words, because "flags dead", "flags
 * live" and "carry borrowed in" are three different code paths. */
void test_dp_sweep() {
  std::printf("[x64 data-processing sweep]\n");
  static const uint32_t kCpsr[3] = {0x00000013u, 0x20000013u, 0xF0000013u};
  const uint32_t regs[13] = {0x9abcdef0u, 0xffffffffu, 0x00000001u, 0x80000000u,
                             0x0000ffffu, 0x12345678u, 0x00000000u, 0x7fffffffu,
                             0xdeadbeefu, 0x00010000u, 0xffff0000u, 0x00000080u,
                             0xabcdef01u};
  uint32_t compared = 0;
  for (uint32_t op = 0; op < 16; ++op) {
    for (uint32_t s = 0; s < 2; ++s) {
      for (int kind = 0; kind < 3; ++kind) {
        for (uint32_t v = 0; v < 8; ++v) {
          uint32_t w = 0xE0000000u | (op << 21) | (s << 20) | (1u << 16) | (2u << 12);
          if (kind == 0) {
            w |= (1u << 25) | ((v % 16) << 8) | (1u << v);     /* imm, rotated */
          } else if (kind == 1) {
            w |= 3u;                                           /* rm = r3 */
          } else {
            w |= ((v * 4u) << 7) | ((v % 4u) << 5) | 3u;       /* shifted rm */
          }
          for (uint32_t ci = 0; ci < 3; ++ci)
            compared += compare_block(&w, 1, regs, kCpsr[ci], "sweep");
        }
      }
    }
  }
  std::printf("  %u translations compared\n", compared);
}

/* ------------------------------------------------------ random sequences --- */

void test_random_blocks() {
  std::printf("[x64 random blocks]\n");
  Rng r(0x5EED1234u);
  uint32_t compared = 0;
  for (int iter = 0; iter < 30000; ++iter) {
    const uint32_t n = 1 + r.below(7);
    uint32_t code[8];
    for (uint32_t i = 0; i < n; ++i) code[i] = random_dp(&r);
    uint32_t regs[13];
    for (int i = 0; i < 13; ++i)
      regs[i] = r.one_in(2) ? r.next() : (r.one_in(2) ? 0xffffffffu : 0x80000000u);
    compared += compare_block(code, n, regs, (r.below(16) << 28) | 0x13u, "random");
  }
  std::printf("  %u blocks compared\n", compared);
}

/* ---------------------------------------------------------- loads/stores -- */

/* Every addressing form the frontend owns, one access per case, each of them
 * checked the way every other case is: same code, same state, two engines, no
 * visible difference.  The words are what an assembler emits (given here with
 * the mnemonic), so a mis-encoded test word would fail rather than silently
 * test something else - `compare_block` also insists the block translated, and
 * it compares memory as well as registers, which is what makes the store forms
 * meaningful.
 *
 * The addresses are the interesting ones: inside the window, at its very last
 * bytes, crossing a page boundary, unaligned, and outside the window entirely
 * (where the block must bail and let the interpreter own the access - the two
 * engines still have to agree, which is the point of the bail). */
void test_mem_sweep() {
  std::printf("[x64 load/store sweep]\n");
  struct MemCase {
    const char* what;
    uint32_t word;
    uint32_t base, index, val;
  } const cases[] = {
      {"ldr r0,[r1,#8]",        0xE5910008u, 0x8100, 0x10, 0x11223344u},
      {"ldr r0,[r1,#8]!",       0xE5B10008u, 0x8100, 0x10, 0x11223344u},
      {"ldr r0,[r1],#8",        0xE4910008u, 0x8100, 0x10, 0x11223344u},
      {"ldrb r0,[r1,#3]",       0xE5D10003u, 0x8100, 0x10, 0x11223344u},
      {"ldrsb r0,[r1,#3]",      0xE1D100D3u, 0x8100, 0x10, 0x11223344u},
      {"ldrh r0,[r1,#2]",       0xE1D100B2u, 0x8100, 0x10, 0x11223344u},
      {"ldrsh r0,[r1,#2]",      0xE1D100F2u, 0x8100, 0x10, 0x11223344u},
      {"ldr r0,[r1,r2]",        0xE7910002u, 0x8100, 0x24, 0x11223344u},
      {"ldr r0,[r1,r2,lsl #2]", 0xE7910102u, 0x8100, 0x09, 0x11223344u},
      {"ldr r0,[r4,-r5,asr #3]", 0xE71401C5u, 0x8180, 0x40, 0x11223344u},
      {"ldr r0,[pc,#16]",       0xE59F0010u, 0x8100, 0x10, 0x11223344u},
      {"ldr r0,[r1,#0xffc]",    0xE5910FFCu, 0x9004, 0x00, 0x11223344u},  /* page edge */
      {"ldr r0,[r1,#4]",        0xE5910004u, 0x100000, 0, 0x11223344u},   /* outside RAM */
      {"str r0,[r1,#4]",        0xE5810004u, 0x8100, 0x10, 0xDEADBEEFu},
      {"str r0,[r1,#4]!",       0xE5A10004u, 0x8100, 0x10, 0xDEADBEEFu},
      {"str r0,[r1],#-4",       0xE4010004u, 0x8108, 0x10, 0xDEADBEEFu},
      {"strb r0,[r1,#1]",       0xE5C10001u, 0x8100, 0x10, 0xDEADBEEFu},
      {"strh r0,[r1,#2]",       0xE1C100B2u, 0x8100, 0x10, 0xDEADBEEFu},
      {"str r0,[r1,r2,lsl #3]", 0xE7810182u, 0x8100, 0x08, 0xDEADBEEFu},
      {"str r0,[r13,#-4]!",     0xE52D0004u, 0x8F00, 0x10, 0xDEADBEEFu},
      {"str r0,[r1,#4]",        0xE5810004u, 0x100000, 0, 0xDEADBEEFu},   /* outside RAM */
  };
  for (const MemCase& c : cases) {
    uint32_t regs[15] = {0};
    regs[0] = c.val;
    regs[1] = c.base;
    regs[2] = c.index;
    regs[4] = c.base;                       /* the shifted-index case */
    regs[5] = c.index;
    regs[13] = 0x8F00;                      /* a stack the shadow owns */
    const uint32_t cpsr = 0xF0000013u;      /* NZCV set, mode SVC */
    check(compare_block(&c.word, 1, regs, cpsr, c.what, 15), c.what);
  }
  std::printf("  %zu accesses compared\n", sizeof(cases) / sizeof(cases[0]));
}

/* ------------------------------------------------------------- branches -- */

/* Fourteen conditions times the sixteen flag combinations, then the branch
 * shapes that are not a condition: BL, BX (to ARM and to Thumb - the dispatcher
 * decodes the switch out of bit 0), BLX Rm, BLX #imm, and the 0xF encoding that
 * is an undefined instruction rather than a branch.  Which way each one went is
 * not asserted here; that the *two engines agree* on the address, the link, the
 * flags and the cost is, and the sweep counts the outcomes so a run where
 * nothing was ever taken cannot pass. */
void test_branch_sweep() {
  std::printf("[x64 branch sweep]\n");
  uint32_t regs[15] = {0};
  regs[3] = 0x8010;                        /* BX/BLX target: ARM state        */
  int taken = 0, not_taken = 0, compared = 0;
  char name[48];
  for (uint32_t cond = 0; cond < 15; ++cond) {
    const uint32_t word = 0x0A000002u | (cond << 28);     /* b<cond> 0x8010 */
    for (uint32_t f = 0; f < 16; ++f) {
      const uint32_t cpsr = ((f & 8u) ? 0x80000000u : 0u) | ((f & 4u) ? 0x40000000u : 0u) |
                            ((f & 2u) ? 0x20000000u : 0u) | ((f & 1u) ? 0x10000000u : 0u) |
                            0x00000013u;
      std::snprintf(name, sizeof name, "b<cond %u>, flags %x", cond, f);
      uint32_t npc = 0;
      if (compare_block(&word, 1, regs, cpsr, name, 15, &npc)) {
        ++compared;
        if (npc == 0x8010) ++taken; else ++not_taken;
      }
    }
  }
  check(compared == 15 * 16, "every condition translated");
  check(taken > 0 && not_taken > 0, "both outcomes were exercised");

  struct BrCase {
    const char* what;
    uint32_t word;
    uint32_t r3;
    uint32_t cpsr;
  } const brs[] = {
      {"bl 0x8010",     0xEB000002u, 0x0000, 0x00000013u},
      {"bl 0x8010, Z",  0x0B000002u, 0x0000, 0x40000013u},   /* taken           */
      {"bl 0x8010, !Z", 0x0B000002u, 0x0000, 0x00000013u},   /* not taken       */
      {"bx r3 (arm)",   0xE12FFF13u, 0x8010, 0x00000013u},
      {"bx r3 (thumb)", 0xE12FFF13u, 0x8011, 0x00000013u},
      {"blx r3 (arm)",  0xE12FFF33u, 0x8010, 0x00000013u},
      {"blx r3 (thumb)",0xE12FFF33u, 0x8011, 0x00000013u},
      {"blx #imm",      0xFA000002u, 0x0000, 0x00000013u},
      {"blx #imm H",    0xFB000002u, 0x0000, 0x00000013u},   /* H=1: target+2  */
      {"b . (loop)",    0xEAFFFFFEu, 0x0000, 0x00000013u},
      {"beq backward",  0x0AFFFFFDu, 0x0000, 0x40000013u},
  };
  for (const BrCase& c : brs) {
    uint32_t r[15] = {0};
    r[0] = 1;
    r[3] = c.r3;
    check(compare_block(&c.word, 1, r, c.cpsr, c.what, 15), c.what);
  }

  /* A branch with something in front of it: the data-processing instruction has
   * to retire before the branch decides, and its flags are what the branch
   * tests. */
  const uint32_t seq[] = {0xE2900001u, 0x0A000001u};      /* adds r0,r0,#1 ; beq +1 */
  for (uint32_t f = 0; f < 16; ++f) {
    const uint32_t cpsr = ((f & 8u) ? 0x80000000u : 0u) | ((f & 4u) ? 0x40000000u : 0u) |
                          ((f & 2u) ? 0x20000000u : 0u) | ((f & 1u) ? 0x10000000u : 0u) |
                          ((f & 4u) ? 0x10000000u : 0u) | 0x00000013u;
    uint32_t r[15] = {0};
    r[0] = 0xFFFFFFFEu;                      /* +1 sets Z and C, clears N and V */
    check(compare_block(seq, 2, r, cpsr, "adds then beq", 15), "adds then beq");
  }
  std::printf("  %d branches compared (%d taken, %d not taken)\n", compared, taken, not_taken);
}

/* --------------------------------------------------------- memory guards -- */

/* The two rules that let a block own a guest access at all.  Neither is about
 * the emitted code: they are about when the *dispatcher* may enter a block that
 * performs one, and both are checked behaviourally - the block must not run,
 * and the interpreter must do the work instead.
 *
 *   - `jit_fast_mem_ok`: with SCTLR.A set an unaligned access has to fault, and
 *     a block performs its accesses inline, so the whole block is out;
 *   - the code-page guard: an inline store cannot call the bus, so a store that
 *     lands in a page holding translated code bails out to the interpreter -
 *     whose write path is the one that reports to the store observer. */
void test_mem_guards() {
  std::printf("[x64 memory guards]\n");
  CpuConfig cfg;
  cfg.ram_size = kRamSize;
  Cpu cpu;
  check(cpu.init(cfg), "cpu init");
  cpu.reset();
  /* Two loads, so that a dispatcher that refuses the *block* has nothing else
   * to run natively - a single access followed by a data-processing instruction
   * would let the second instruction's own block retire and blur the count. */
  const uint32_t prog[] = {0xE5910004u, 0xE5912008u};     /* ldr r0,[r1,#4] ; ldr r2,[r1,#8] */
  for (uint32_t i = 0; i < 2; ++i) cpu.phys_write(kCode + i * 4, prog[i], 4);
  cpu.phys_write(kCode + 0x104, 0x5A5A5A5Au, 4);          /* the loaded word */
  cpu.phys_write(kCode + 0x108, 0xA5A5A5A5u, 4);
  cpu.regs[1] = kCode + 0x100;
  cpu.npc = cpu.cur = kCode; cpu.regs[15] = kCode + 8;

  JitCache* jc = jit_cache_create(&cpu, 1);
  check(jc != nullptr, "cache create");
  if (!jc) return;

  /* 1. The plain case: both instructions run as host code. */
  jit_cache_run_insns(jc, 2);
  check(cpu.stats.insns_jit == 2, "the load block ran natively");
  check(cpu.regs[0] == 0x5A5A5A5Au && cpu.regs[2] == 0xA5A5A5A5u, "and read the right words");

  /* 2. With the alignment check on, the same block must not be entered.  The
   *    address here is aligned, so the interpreter gets the same answer - which
   *    is the point: the dispatcher refuses on the *rule*, not on the address. */
  const uint64_t j1 = cpu.stats.insns_jit, i1 = cpu.insns;
  cpu.npc = cpu.cur = kCode; cpu.regs[15] = kCode + 8;
  cpu.regs[0] = cpu.regs[2] = 0;
  cpu.align_chk = 1;
  jit_cache_run_insns(jc, 2);
  check(cpu.stats.insns_jit == j1, "with the A bit set the block is not entered");
  check(cpu.insns == i1 + 2, "the interpreter covers it instead");
  check(cpu.regs[0] == 0x5A5A5A5Au && cpu.regs[2] == 0xA5A5A5A5u,
        "and the interpreter reads the same words");
  cpu.align_chk = 0;

  /* 3. A store into a page that holds translated code bails: the block for it
   *    translates, the guard fires, and the interpreter performs the store. */
  const uint32_t st = 0xE5810004u;                        /* str r0,[r1,#4] */
  cpu.phys_write(kCode + 0x40, st, 4);
  cpu.regs[0] = 0x1234ABCDu;
  cpu.regs[1] = kCode;                                    /* the code page   */
  cpu.npc = cpu.cur = kCode + 0x40; cpu.regs[15] = kCode + 0x48;
  const uint64_t j2 = cpu.stats.insns_jit, i2 = cpu.insns;
  JitBlock* b = nullptr;
  check(jit_cache_translate(jc, kCode + 0x40, &b) && b && b->uses_mem,
        "the store block translates with uses_mem");
  jit_cache_run_insns(jc, 1);
  check(cpu.stats.insns_jit == j2, "the guarded store did not run inline");
  check(cpu.insns == i2 + 1, "the interpreter retired it");
  check(cpu.phys_read(kCode + 4, 4) == 0x1234ABCDu, "and the store landed");

  jit_cache_destroy(jc);
}

/* Thirteen values that all stay live to the end of the block: the pool of host
 * registers is smaller than that, so the allocator has to spill and evict in
 * the middle of the block and the comparison has to come out identical. */
void test_register_pressure() {
  std::printf("[x64 register pressure]\n");
  uint32_t code[26];
  /* r0..r12 each become (rn + rm) of earlier values ... */
  for (uint32_t i = 0; i < 13; ++i) {
    code[i] = 0xE0800000u | ((i + 1) % 13) << 16 | (i << 12) | ((i + 2) % 13);
  }
  /* ... and then every one of them is read again at the end. */
  for (uint32_t i = 0; i < 13; ++i) {
    code[13 + i] = 0xE0800000u | (i << 16) | (i << 12) | ((i + 7) % 13);
  }
  uint32_t regs[13];
  for (int i = 0; i < 13; ++i) regs[i] = 0x10000000u * (uint32_t)(i + 1) + 7u;
  for (uint32_t ci = 0; ci < 3; ++ci)
    compare_block(code, 26, regs, 0x20000013u, "pressure");
}

}  // namespace

int main() {
  std::printf("brain-emu x86-64 JIT tests\n");
  if (BRAIN_JIT_BACKEND != 2) {
    std::printf("SKIP: no x86-64 backend compiled in on this host (backend id %d)\n",
                (int)BRAIN_JIT_BACKEND);
    return 0;
  }
  test_encoder();
  test_shadow_layout();
  test_dp_sweep();
  test_mem_sweep();
  test_branch_sweep();
  test_mem_guards();
  test_random_blocks();
  test_register_pressure();
  /* The sweeps above are only worth anything if the host really executed the
   * blocks: two interpreters would agree with each other perfectly. */
  check(g_native > 1000, "the sweeps retired instructions on the host");
  std::printf("%s: %d/%d checks passed\n", g_failed ? "FAILED" : "PASS",
              g_checks - g_failed, g_checks);
  return g_failed ? 1 : 0;
}
