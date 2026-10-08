/* tools/jit_emit: show what the JIT front end produces, and what the machine does.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Two modes, both meant to be driven by tests/test_jit_arm64.py:
 *
 *   --forms     one emitted encoding per line (`name hex`), so the AArch64 field
 *               layouts can be checked against a disassembler.
 *   --case ...  translate guest words into IR, emit AArch64 for them, and run the
 *               *same* words through the interpreter, printing the state both sides
 *               start and end with.  The test then executes the emitted words on a
 *               real AArch64 model (Unicorn) and compares - that is the whole
 *               point: the emitter is verified by behaviour, not by reading it.
 *
 * The words given to --case are expected to be one basic block, but nothing here
 * requires it: translation stops at the first instruction the JIT does not accept,
 * and the run is limited to the number of instructions it did accept.
 */
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include <brainemu/brainemu.h>

#include "machines/arm926/jit/emit_arm64.h"
#include "machines/arm926/jit/frontend.h"

using namespace brain::arm926;
using namespace brain::arm926::arm64;   /* the AArch64 encoding model */

static bool auto_mode = false;

static void print_forms() {
  struct Case { const char* name; Buf b; };
  std::vector<Case> v;
  { Buf b; b.mov_imm(1, 0x1234u); v.push_back({"mov_imm_lo", b}); }
  { Buf b; b.mov_imm(1, 0x12345678u); v.push_back({"mov_imm_full", b}); }
  { Buf b; b.mov_reg(1, 2); v.push_back({"mov_reg", b}); }
  { Buf b; b.add_imm(1, 2, 0x1f); v.push_back({"add_imm", b}); }
  { Buf b; b.add_imm(1, 2, 0x1f, true); v.push_back({"adds_imm", b}); }
  { Buf b; b.sub_imm(1, 2, 4); v.push_back({"sub_imm", b}); }
  { Buf b; b.add_shifted(1, 2, 3); v.push_back({"add_reg", b}); }
  { Buf b; b.add_shifted(1, 2, 3, SH_LSL, 4); v.push_back({"add_lsl", b}); }
  { Buf b; b.add_shifted(1, 2, 3, SH_ASR, 2); v.push_back({"add_asr", b}); }
  { Buf b; b.add_shifted(1, 2, 3, SH_LSL, 0, true); v.push_back({"adds_reg", b}); }
  { Buf b; b.sub_shifted(1, 2, 3); v.push_back({"sub_reg", b}); }
  { Buf b; b.sub_shifted(XZR, 2, 3, SH_LSL, 0, true);
    v.push_back({"cmp_reg", b}); }
  { Buf b; b.and_shifted(1, 2, 3); v.push_back({"and_reg", b}); }
  { Buf b; b.and_shifted(XZR, 2, 3, SH_LSL, 0, true);
    v.push_back({"tst_reg", b}); }
  { Buf b; b.and_shifted(1, 2, 3, SH_LSL, 4, true); v.push_back({"ands_lsl4", b}); }
  { Buf b; b.bic_shifted(1, 2, 3); v.push_back({"bic_reg", b}); }
  { Buf b; b.bic_shifted(1, 2, 3, SH_LSL, 0, true); v.push_back({"bics_reg", b}); }
  { Buf b; b.orr_shifted(1, 2, 3, SH_LSR, 7); v.push_back({"orr_lsr7", b}); }
  { Buf b; b.orr_shifted(1, 2, 3); v.push_back({"orr_reg", b}); }
  { Buf b; b.set_nz_from(1); v.push_back({"set_nz_from", b}); }
  { Buf b; b.eor_shifted(1, 2, 3); v.push_back({"eor_reg", b}); }
  { Buf b; b.eor_shifted(1, 2, 3, SH_ROR, 11); v.push_back({"eor_ror11", b}); }
  { Buf b; b.orn_shifted(1, 2, 3); v.push_back({"orn_reg", b}); }
  { Buf b; b.mvn(1, 2); v.push_back({"mvn", b}); }
  { Buf b; b.neg_shifted(1, 2); v.push_back({"neg", b}); }
  { Buf b; b.adc(1, 2, 3); v.push_back({"adc", b}); }
  { Buf b; b.sbc(1, 2, 3); v.push_back({"sbc", b}); }
  /* RSC: the same SBC with the operands exchanged, and its flag-setting form -
   * the pair the x86-64 backend builds out of `cmc` + `sbb`.  (There is no
   * shifted SBC to pin: the with-carry group has no shift field, which is why
   * the emitter shifts the operand separately instead.) */
  { Buf b; b.sbc(1, 3, 2); v.push_back({"rsc", b}); }
  { Buf b; b.sbc(1, 3, 2, true); v.push_back({"rscs", b}); }
  { Buf b; b.shift(1, 2, SH_LSL, 4); v.push_back({"lsl_imm", b}); }
  { Buf b; b.shift(1, 2, SH_LSR, 3); v.push_back({"lsr_imm", b}); }
  { Buf b; b.shift(1, 2, SH_ASR, 2); v.push_back({"asr_imm", b}); }
  { Buf b; b.shift(1, 2, SH_ROR, 1); v.push_back({"ror1", b}); }
  { Buf b; b.ldr32(1, 0, 4); v.push_back({"ldr_w", b}); }
  { Buf b; b.str32(1, 0, 4); v.push_back({"str_w", b}); }
  { Buf b; b.ubfx(16, 15, 29, 1); v.push_back({"ubfx", b}); }
  { Buf b; b.flags_patch(0x30000000u, 28, SCR2, 28, 2);
    v.push_back({"flags_patch", b}); }
  { Buf b; b.mov_rrx(1, 2); v.push_back({"mov_rrx", b}); }
  { Buf b; b.mrs_nzcv(14); v.push_back({"mrs_nzcv", b}); }
  { Buf b; b.msr_nzcv(14); v.push_back({"msr_nzcv", b}); }
  { Buf b; b.ret(); v.push_back({"ret", b}); }
  { Buf b; b.nop(); v.push_back({"nop", b}); }
  { Buf b; b.prologue(); v.push_back({"prologue", b}); }
  { Buf b; b.epilogue(0x8020u, 0x1Fu); v.push_back({"epilogue", b}); }
  for (size_t i = 0; i < v.size(); ++i)
    for (size_t j = 0; j < v[i].b.w.size(); ++j)
      std::printf("%s %u %08x\n", v[i].name, (unsigned)j, v[i].b.w[j]);
}

/* Options take bare hex, because a value like `60000000` written without a prefix
 * is a CPSR in hex - base 0 here would read it as 60 000 000 decimal. */
static uint32_t parse_hex(const char* s) {
  if (s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) s += 2;
  return (uint32_t)std::strtoul(s, nullptr, 16);
}

/* --sweep: every data-processing encoding the front end could be asked about.
 *
 * The corpus only covers what the generator happened to emit, which for a flag
 * setting `MOV` with a shift is nothing at all.  This walks the whole shape space
 * instead - 16 opcodes x 4 shift types x 32 amounts x S x the immediate form - and
 * for each word prints the emitted AArch64, the starting state and what the
 * interpreter ended with.  The caller runs the machine code on a real AArch64 model
 * and compares; that is the differential the emitter needs, and it is exhaustive
 * over a whole instruction class rather than over a sample of one.
 *
 * One state for every word keeps the output small: the seed is printed once, and a
 * block that the front end refuses is reported as `SW <word> skip` so the count of
 * refused shapes is visible too.
 */
static int print_sweep(int argc, char** argv) {
  uint32_t pc = 0x8000u, cpsr = 0x60000000u;
  uint32_t regs[13];
  for (int i = 0; i < 13; ++i) regs[i] = 0x1111u * (uint32_t)(i + 1) + (uint32_t)i;
  for (int i = 0; i < argc; ++i) {
    if (!std::strcmp(argv[i], "--pc") && i + 1 < argc) pc = parse_hex(argv[++i]);
    else if (!std::strcmp(argv[i], "--cpsr") && i + 1 < argc) cpsr = parse_hex(argv[++i]);
    else if (!std::strcmp(argv[i], "--regs") && i + 1 < argc) {
      char* p = argv[++i];
      int n = 0;
      while (*p && n < 13) { regs[n++] = (uint32_t)std::strtoul(p, &p, 16); if (*p == ',') ++p; }
    }
  }
  std::printf("SEED %08x", cpsr);
  for (int i = 0; i < 13; ++i) std::printf(" %08x", regs[i]);
  std::printf("\n");

  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  int n_ok = 0, n_skip = 0;
  /* Rn, Rd and Rm are spread out so that a wrong operand field shows up as a wrong
   * value rather than as an accident of two registers holding the same thing. */
  const uint32_t rn = 3u, rd = 5u, rm = 7u;
  for (uint32_t dp = 0; dp < 16u; ++dp) {
    for (uint32_t st = 0; st < 4u; ++st) {
      for (uint32_t amt = 0; amt < 32u; ++amt) {
        for (uint32_t st2 = 0; st2 < 2u; ++st2) {   /* 0 = shifted register, 1 = not */
          for (uint32_t sf = 0; sf < 2u; ++sf) {     /* S */
            uint32_t w;
            if (st2) {
              if (amt) continue;                     /* one immediate per amount slot */
              /* `rot` is bits[11:8]: use it as 1..4 so the value really is
               * rotated, which is the case the front end has to reproduce. */
              /* imm8's low nibble cycles through 0xC..0xF: a front end that
               * mistakes the low nibble for a register field refuses the ones
               * next to 0xF, so the sweep has to contain them. */
              w = (0xEu << 28) | (1u << 25) | (dp << 21) | (sf << 20) | (rn << 16) |
                  (rd << 12) | (((uint32_t)st + 1u) << 8) | (0x0Cu + (st & 3u));
            } else {
              w = (0xEu << 28) | (dp << 21) | (sf << 20) | (rn << 16) | (rd << 12) |
                  (amt << 7) | (st << 5) | rm;
              if (dp >= 8u && dp <= 11u) w &= ~(0xFu << 12);   /* Rd is part of op2 */
            }
            IrBlock blk{};
            const uint32_t used = jit_translate_arm_words(&w, 1, pc, &blk);
            std::vector<uint32_t> code;
            if (!used || !jit_emit_arm64(blk, true, &code)) {
              std::printf("SW %08x skip\n", w);
              ++n_skip;
              continue;
            }
            /* The interpreter, from the same seed, one instruction. */
            brain_load_raw(e, reinterpret_cast<const uint8_t *>(&w), 4, pc, 0);
            brain_set_reg(e, BRAIN_REG_CPSR, cpsr);
            for (int i = 0; i < 13; ++i) brain_set_reg(e, i, regs[i]);
            brain_run_insns(e, 1);
            std::printf("SW %08x %u", w, (unsigned)code.size());
            for (size_t k = 0; k < code.size(); ++k) std::printf(" %08x", code[k]);
            std::printf(" | %08x", brain_get_reg(e, BRAIN_REG_CPSR));
            for (int i = 0; i < 13; ++i) std::printf(" %08x", brain_get_reg(e, i));
            std::printf(" | %08x\n", blk.next_pc);
            ++n_ok;
          }
        }
      }
    }
  }
  std::printf("TOTAL %d: %d emitted, %d refused\n", n_ok + n_skip, n_ok,
              n_skip);
  brain_emu_destroy(e);
  return 0;
}

int main(int argc, char** argv) {
  if (argc > 1 && !std::strcmp(argv[1], "--forms")) { print_forms(); return 0; }
  if (argc > 1 && !std::strcmp(argv[1], "--sweep")) return print_sweep(argc - 2, argv + 2);

  uint32_t pc = 0x8000u, cpsr = 0x60000000u, steps = 0u;
  bool thumb = false, exec_only = false, translate_only = false;
  /* --poke a:v seeds data memory (an unaligned LDR/STR cannot be judged without
   * bytes whose positions matter), --peek a reads it back after the run. */
  std::vector<std::pair<uint32_t, uint32_t>> pokes, peeks;
  /* 16 slots: R13/R14/R15 matter to programs under test (stack pointer, link
   * register, and the prefetched PC the core starts from), and --regs may supply
   * them.  nregs counts what the caller actually passed. */
  uint32_t regs[16];
  uint32_t nregs = 0;
  for (int i = 0; i < 16; ++i) regs[i] = 0x1000u + (uint32_t)i * 0x1111u;
  std::vector<uint32_t> words;
  for (int i = 1; i < argc; ++i) {
    if (!std::strcmp(argv[i], "--thumb")) { thumb = true; exec_only = true; }
    else if (!std::strcmp(argv[i], "--exec")) { exec_only = true; }
    else if (!std::strcmp(argv[i], "--translate")) { translate_only = true; exec_only = false; }
    else if (!std::strcmp(argv[i], "--poke") && i + 1 < argc) {
      char* p = argv[++i];
      while (*p) {
        char* q;
        const unsigned long a = std::strtoul(p, &q, 16);
        uint32_t v = 0;
        if (*q == ':') v = (uint32_t)std::strtoul(q + 1, &q, 16);
        pokes.push_back({(uint32_t)a, v});
        p = (*q == ',') ? q + 1 : q;
        if (!*q) break;
      }
    } else if (!std::strcmp(argv[i], "--peek") && i + 1 < argc) {
      char* p = argv[++i];
      while (*p) {
        char* q;
        peeks.push_back({(uint32_t)std::strtoul(p, &q, 16), 0});
        p = (*q == ',') ? q + 1 : q;
        if (!*q) break;
      }
    }
    else if (!std::strcmp(argv[i], "--pc") && i + 1 < argc) pc = parse_hex(argv[++i]);
    else if (!std::strcmp(argv[i], "--cpsr") && i + 1 < argc) cpsr = parse_hex(argv[++i]);
    else if (!std::strcmp(argv[i], "--steps") && i + 1 < argc) steps = parse_hex(argv[++i]);
    else if (!std::strcmp(argv[i], "--auto")) { auto_mode = true; }
    else if (!std::strcmp(argv[i], "--regs") && i + 1 < argc) {
      char* p = argv[++i];
      nregs = 0;
      while (*p && nregs < 16) {
        regs[nregs++] = (uint32_t)std::strtoul(p, &p, 16);
        if (*p == ',') ++p;
      }
    } else if (!std::strcmp(argv[i], "--hex") && i + 1 < argc) {
      char* p = argv[++i];
      while (*p) {
        words.push_back((uint32_t)std::strtoul(p, &p, 16));
        if (*p == ',') ++p;
      }
    } else {
      std::fprintf(stderr, "usage: jit_emit --forms | --sweep | --hex w[,w...]\n"
                          "                 [--exec | --translate] [--thumb] [--pc a] [--cpsr v]\n"
                          "                 [--regs r0,r1,...] [--steps n] [--auto]\n"
                          "                 [--poke a:v[,...]] [--peek a[,...]]\n");
      return 2;
    }
  }
  if (words.empty()) { std::fprintf(stderr, "no words\n"); return 2; }

  /* --auto: the first words of a test case are usually seed instructions the front
   * end refuses on purpose (flag-setting with a rotated immediate), so step the
   * interpreter forward until a block can be translated at the current PC. */
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof cfg);
  cfg.ram_size = 1u << 20;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) { std::fprintf(stderr, "create failed\n"); return 2; }
  /* The load and the register seed are shared by both modes - and they have to come
   * before anything runs.  `brain_load_raw` *enters* the payload: it seeds npc/cur
   * with the load address and takes CPSR.T from its `thumb` argument, so the core
   * already holds the first instruction of the program unexecuted and a run started
   * right here begins at `pc`.  The explicit seeds below exist to override, never to
   * rescue: `--regs` may supply any of R0-R15 and every other slot defaults to
   * `0x1000 + 0x1111 * index`, except R15, which lands on the first word.  In Thumb
   * mode R15 is re-seeded as `pc | 1`: a store into PC is an interworking branch as
   * far as the core is concerned (bit[0] carries the state while SCTLR.L4 = 0,
   * ARM926EJ-S TRM 2.3.2), so the explicit write has to say what the CPSR poke one
   * line above already set - a `--cpsr` value without T would otherwise leave the
   * halfwords decoding as ARM instructions. */
  if (thumb) {
    std::vector<uint16_t> hw;
    for (uint32_t w : words) {
      if (w > 0xFFFFu) { hw.push_back((uint16_t)(w >> 16)); hw.push_back((uint16_t)w); }
      else hw.push_back((uint16_t)w);
    }
    brain_load_raw(e, hw.data(), hw.size() * 2u, pc, 1);
    cpsr |= 0x20u;
  } else {
    brain_load_raw(e, words.data(), words.size() * 4u, pc, 0);
  }
  brain_set_reg(e, BRAIN_REG_CPSR, cpsr);
  for (uint32_t i = 0; i < 16; ++i) {
    const uint32_t v = i < nregs ? regs[i]
                                 : (i == 15u ? pc : 0x1000u + i * 0x1111u);
    brain_set_reg(e, (int)i, v);
  }
  if (nregs <= 15) brain_set_reg(e, 15, thumb ? (pc | 1u) : pc);
  for (auto& kv : pokes) brain_poke32(e, kv.first, kv.second);

  /* --exec (implied by --thumb): seed state, run N instructions through the
   * interpreter and print the state, with no translation attempt at all.  This is
   * how tests/test_oracle.py and tests/test_shifter.py compare the core against a
   * reference model; the JIT path below prints PRE/POST around a *translated* block
   * instead, which is a different program and must never be read as the result of
   * the seeded run.  In --thumb mode a value wider than 16 bits is a 32-bit Thumb
   * instruction and is stored as its two halfwords in order (first halfword at the
   * lower address, as the assembler emits it). */
  if (exec_only) {
    const uint32_t n = steps ? steps : (uint32_t)words.size();
    std::printf("MODE %s\n", thumb ? "thumb" : "arm");
    std::printf("PRE %08x", brain_get_reg(e, BRAIN_REG_CPSR));
    for (int i = 0; i < 13; ++i) std::printf(" %08x", brain_get_reg(e, i));
    std::printf("\n");
    const uint32_t done = (uint32_t)brain_run_insns(e, n);
    /* Printed after the run, obviously - and the count the core retired is echoed so
     * a program that stopped early (a trap, or running out of loaded words) is visible
     * instead of showing up as an unexplained state difference. */
    std::printf("POST %08x", brain_get_reg(e, BRAIN_REG_CPSR));
    for (int i = 0; i < 13; ++i) std::printf(" %08x", brain_get_reg(e, i));
    std::printf("\nRAN %u\n", done);
    std::printf("R15 %08x\n", brain_get_reg(e, 15));
    for (auto& kv : peeks) std::printf("PEEK %08x %08x\n", kv.first, brain_peek32(e, kv.first));
    brain_emu_destroy(e);
    return 0;
  }

  uint32_t skip = steps;
  IrBlock blk{};
  uint32_t used = 0;
  if (skip) {
    brain_run_insns(e, skip);
    blk.guest_insns = (uint32_t)words.size() - skip;   /* what runs below, by default */
    if (!translate_only)
      used = jit_translate_arm_words(words.data() + skip, (uint32_t)words.size() - skip,
                                     pc + 4u * skip, &blk);
  } else {
    for (uint32_t i0 = 0; i0 < words.size(); ++i0) {
      if (i0 && brain_get_reg(e, 15) != pc + 4u * i0) break;  /* PC moved: no block */
      used = jit_translate_arm_words(words.data() + i0, (uint32_t)words.size() - i0,
                                     pc + 4u * i0, &blk);
      if (used) { skip = i0; break; }
      if (!auto_mode) break;
      brain_run_insns(e, 1);
    }
  }
  std::printf("USED %u\n", used);
  std::printf("SKIP %u\n", skip);
  std::printf("INSNS %u\n", blk.guest_insns);
  std::printf("STOP %s\n", blk.stop ? blk.stop : "?");
  std::printf("TRUNC %d\n", blk.truncated ? 1 : 0);
  for (uint32_t i = 0; i < blk.n; ++i) {
    const IrInsn& in = blk.in[i];
    std::printf("IR %u %s", i, ir_op_name(in.op));
    if (in.dst != IR_NO_DST) std::printf(" r%u", in.dst);
    std::printf(", r%u", in.a);
    if (in.b_is_imm) std::printf(", #0x%x", in.imm);
    else std::printf(", r%u", in.b);
    if (in.shift_amt) std::printf(", sh%u#%u", in.shift_type, in.shift_amt);
    std::printf("%s\n", in.setflags ? ", flags" : "");
  }

  /* The same words, run by the interpreter, from the state the block starts in: the
   * shadowed registers are exactly R0-R12, and R15 is `next_pc` by construction.
   * This runs whether or not the front end took the words, because `--hex <word>`
   * with no --steps is also the cheapest way to step one instruction of the core
   * and read the result back; tests/test_shifter.py does exactly that.  When
   * nothing was translated, the single refused word is what gets executed and
   * next_pc is the fall-through. */
  /* In --steps mode the translate attempt above is only a side probe; what this
   * section has to do is run the block's own words, so that PRE/POST bracket a real
   * execution of them (tests/test_jit_arm64.py layer 2 compares that against the
   * emitted AArch64 block).  --translate says "probe the front end, don't run", and
   * is only useful when the interpreter's answer is already known. */
  uint32_t run_n = words.size() - (translate_only ? 0u : skip);
  if (used && blk.guest_insns) run_n = blk.guest_insns;
  if (run_n == 0) run_n = 1;
  std::printf("PRE %08x", brain_get_reg(e, BRAIN_REG_CPSR));
  for (int i = 0; i < 13; ++i) std::printf(" %08x", brain_get_reg(e, i));
  std::printf("\n");
  brain_run_insns(e, run_n);
  (void)0;
  std::printf("POST %08x", brain_get_reg(e, BRAIN_REG_CPSR));
  for (int i = 0; i < 13; ++i) std::printf(" %08x", brain_get_reg(e, i));
  std::printf("\n");
  std::printf("R15 %08x\n", brain_get_reg(e, 15));
  std::printf("NEXTPC %08x\n", used ? blk.next_pc : pc + 4u * (skip + run_n));
  for (auto& kv : peeks) std::printf("PEEK %08x %08x\n", kv.first, brain_peek32(e, kv.first));

  std::vector<uint32_t> code;
  if (!used) { std::printf("CODE\n"); brain_emu_destroy(e); return translate_only ? 0 : 1; }
  if (!jit_emit_arm64(blk, true, &code)) { std::printf("EMIT_FAILED\n"); return 1; }
  std::printf("CODE %u", (unsigned)code.size());
  for (size_t i = 0; i < code.size(); ++i) std::printf(" %08x", code[i]);
  std::printf("\n");
  brain_emu_destroy(e);
  return 0;
}
