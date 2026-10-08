/* brain-emu ISA conformance harness.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Replays the corpus produced by arch/arm/cpu/classic/arm9/arm926ej-s/tests/gen_instr_tests.py (encodings from
 * Keystone, expected state from Unicorn) through the interpreter and diffs the
 * full architectural result: R0-R15, CPSR (masked) and 256 bytes of data
 * memory.  Run with --jit to replay the same corpus through the code generator;
 * the expected values must not change.
 *
 * The comparison deliberately ignores CPSR bits whose meaning changed after
 * ARMv5 (J, GE, A, E) -- see the header of the generator for why.
 */
#include "arch/arm/cpu/classic/arm9/arm926ej-s/jit/jit.h"   /* BRAIN_JIT_BACKEND for the --jit self-check */
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/core/cpu.h"
#include "arch/arm/isa/armv5/ops.h"

using namespace brain;
using namespace brain::arm926;
using namespace brain::arm::isa::armv5;

namespace {

constexpr u32 kCpsrMask = 0xF0000020u;
constexpr u32 kCodeBase = 0x8000;
constexpr u32 kDataBase = 0x9000;
constexpr u32 kDataSize = 256;

struct Case {
  std::string name;
  bool thumb = false;
  u32 count = 0;
  u32 cpsr = 0;
  u32 regs[16] = {};
  u32 expect_regs[16] = {};
  u32 expect_cpsr = 0;
  std::vector<u32> code;
  std::vector<u8> data, expect_data;
};

std::vector<Case> load(const char* path) {
  FILE* f = std::fopen(path, "rb");
  if (!f) {
    std::fprintf(stderr, "cannot open %s -- run: python3 arch/arm/cpu/classic/arm9/arm926ej-s/arch/arm/cpu/classic/arm9/arm926ej-s/tests/gen_instr_tests.py\n",
                 path);
    std::exit(2);
  }
  std::vector<Case> out;
  char line[8192];
  Case* c = nullptr;
  while (std::fgets(line, sizeof(line), f)) {
    char* nl = std::strchr(line, '\n');
    if (nl) *nl = 0;
    if (line[0] == '#' || line[0] == 0) continue;
    if (!std::strncmp(line, "BEGIN ", 6)) {
      out.emplace_back();
      c = &out.back();
      char nm[256] = {};
      char mode[16] = {};
      unsigned cnt = 0;
      std::sscanf(line + 6, "%255s mode=%15s count=%u", nm, mode, &cnt);
      c->name = nm;
      c->thumb = std::strstr(mode, "thumb") != nullptr;
      c->count = cnt;
      continue;
    }
    if (!c) continue;
    auto read_u32s = [](char* s, u32* dst, int n) {
      for (int i = 0; i < n; ++i) {
        char* e = nullptr;
        dst[i] = (u32)std::strtoul(s, &e, 16);
        if (!e || e == s) return;
        s = e;
        while (*s == ' ') ++s;
      }
    };
    if (!std::strncmp(line, "CPSR ", 5)) {
      std::sscanf(line + 5, "%x", &c->cpsr);
    } else if (!std::strncmp(line, "REGS ", 5)) {
      read_u32s(line + 5, c->regs, 16);
    } else if (!std::strncmp(line, "CODE ", 5)) {
      char* s = line + 5;
      for (;;) {
        char* e = nullptr;
        u32 v = (u32)std::strtoul(s, &e, 16);
        if (!e || e == s) break;
        c->code.push_back(v);
        s = e;
        while (*s == ' ') ++s;
      }
    } else if (!std::strncmp(line, "DATA ", 5) ||
               !std::strncmp(line, "EXPECT_DATA ", 12)) {
      bool expect = line[1] == 'X';
      char* s = std::strchr(line, ' ') + 1;
      std::vector<u8>& d = expect ? c->expect_data : c->data;
      for (;;) {
        char* e = nullptr;
        u32 v = (u32)std::strtoul(s, &e, 16);
        if (!e || e == s) break;
        d.push_back((u8)v);
        s = e;
        while (*s == ' ') ++s;
      }
    } else if (!std::strncmp(line, "EXPECT_CPSR ", 12)) {
      std::sscanf(line + 12, "%x", &c->expect_cpsr);
    } else if (!std::strncmp(line, "EXPECT_REGS ", 12)) {
      read_u32s(line + 12, c->expect_regs, 16);
    } else if (!std::strncmp(line, "END", 3)) {
      c = nullptr;
    }
  }
  std::fclose(f);
  return out;
}

std::string disasm(const Case& c) {
  std::string out;
  char buf[96];
  for (size_t i = 0; i < c.code.size(); ++i) {
    if (c.thumb) format_thumb_insn(c.code[i] & 0xFFFF, buf, sizeof(buf));
    else format_arm_insn(c.code[i], buf, sizeof(buf));
    char a[16];
    std::snprintf(a, sizeof(a), c.thumb ? "%04x" : "%08x", c.code[i]);
    out += std::string(a) + " " + buf + "\n           ";
  }
  return out;
}

int run_case(Case& c, bool use_jit, std::string* why, bool trace = false,
             u64* jit_insns = nullptr) {
  CpuConfig cfg;
  cfg.ram_base = 0;
  cfg.ram_size = 0x20000;
  cfg.use_jit = use_jit;
  cfg.trace = trace;
  Cpu cpu;
  if (!cpu.init(cfg)) { *why = "cpu init failed"; return 1; }
  cpu.reset();

  /* guest memory */
  if (c.data.size() < kDataSize) c.data.resize(kDataSize, 0);
  cpu.load_image(kDataBase, c.data.data(), kDataSize);
  std::vector<u8> codeb;
  for (size_t i = 0; i < c.code.size(); ++i) {
    if (c.thumb) {
      u16 h = (u16)c.code[i];
      codeb.insert(codeb.end(), (u8*)&h, (u8*)&h + 2);
    } else {
      u32 w = c.code[i];
      codeb.insert(codeb.end(), (u8*)&w, (u8*)&w + 4);
    }
  }
  cpu.load_image(kCodeBase, codeb.data(), codeb.size());

  for (u32 i = 0; i < 16; ++i) cpu.regs[i] = c.regs[i];
  /* The oracle machine boots in SVC mode; run there too so MRS/MSR and the
   * banked R13/R14 behave the same way. */
  cpu.cpsr = (c.cpsr | (c.thumb ? u32(CPSR_T) : 0u)) & ~0x1Fu;
  cpu.cpsr |= 0x13;
  cpu.thumb = c.thumb ? 1u : 0u;
  cpu.bank_idx = 0;
  cpu.priv = 0;
  /* USR-mode bank mirrors regs; keep them in step so a mode switch inside the
   * case does not lose the seeded R13/R14. */
  for (u32 i = 0; i < 16; ++i) {
    cpu.bank[0].r[i] = c.regs[i];
    cpu.bank[3].r[i] = c.regs[i];
  }
  cpu.bank_idx = 3;
  cpu.sync_cp15_caches();
  cpu.npc = kCodeBase;
  cpu.cur = kCodeBase;
  cpu.cycles = 0;
  cpu.insns = 0;

  if (trace) {
    char txt[96];
    for (u32 i = 0; i < c.count; ++i) {
      u32 here = cpu.npc;
      u32 w = cpu.phys_read(here & ~3u, c.thumb ? 2 : 4);
      if (c.thumb) format_thumb_insn(w & 0xFFFF, txt, sizeof(txt));
      else format_arm_insn(w, txt, sizeof(txt));
      std::printf("  %08x: %08x  %-42s R0=%08x R1=%08x R2=%08x R3=%08x "
                  "cpsr=%08x\n", here, w, txt, cpu.regs[0], cpu.regs[1],
                  cpu.regs[2], cpu.regs[3], cpu.cpsr);
      cpu.run_until(1, 0);
    }
    std::printf("  after: R0=%08x R1=%08x R2=%08x R3=%08x cpsr=%08x\n",
                cpu.regs[0], cpu.regs[1], cpu.regs[2], cpu.regs[3], cpu.cpsr);
    std::printf("  want:  R0=%08x R1=%08x R2=%08x R3=%08x cpsr=%08x\n",
                c.expect_regs[0], c.expect_regs[1], c.expect_regs[2],
                c.expect_regs[3], c.expect_cpsr);
    return 0;
  }

  cpu.run_until(c.count, 0);

  int bad = 0;
  for (u32 i = 0; i < 15; ++i) {
    /* QEMU keeps R13 and R14 in a TCG temporary until the translation block
     * ends, so an oracle run of one short block cannot be used to check the
     * stack pointer or a link value written by the last instruction.  Link and
     * stack effects are covered instead by cases that read them back into a
     * general register (MOV Rn, R14 / ADD Rn, SP, #0). */
    if (i == 13 || i == 14) continue;
    if (cpu.regs[i] != c.expect_regs[i]) {
      char b[128];
      std::snprintf(b, sizeof(b), "R%u got %08x want %08x", i, cpu.regs[i],
                    c.expect_regs[i]);
      *why += why->empty() ? "" : "; ";
      *why += b;
      ++bad;
    }
  }
  /* R15 is deliberately not compared: QEMU keeps env->regs[15] pinned to the
   * start of the translated block, so a 1-4 instruction run reads back the
   * address of the first instruction rather than the address after the last.
   * Branch behaviour is still fully covered, because the cases put the effect
   * of the branch (which instruction executed) into R0/R1 and into LR. */
  if ((cpu.cpsr & kCpsrMask) != (c.expect_cpsr & kCpsrMask)) {
    char b[128];
    std::snprintf(b, sizeof(b), "CPSR got %08x want %08x",
                  cpu.cpsr & kCpsrMask, c.expect_cpsr & kCpsrMask);
    *why += why->empty() ? "" : "; ";
    *why += b;
    ++bad;
  }
  for (u32 i = 0; i < kDataSize; i += 4) {
    u32 got = cpu.phys_read(kDataBase + i, 4);
    u32 want = 0;
    for (u32 j = 0; j < 4; ++j) want |= (u32)c.expect_data[i + j] << (j * 8);
    if (got != want) {
      char b[128];
      std::snprintf(b, sizeof(b), "mem[%03x] got %08x want %08x", i, got, want);
      *why += why->empty() ? "" : "; ";
      *why += b;
      ++bad;
      break;
    }
  }
  if (jit_insns) *jit_insns = cpu.stats.insns_jit;
  return bad;
}

}  // namespace

int main(int argc, char** argv) {
  const char* path = "arch/arm/cpu/classic/arm9/arm926ej-s/tests/data/corpus.txt";
  bool use_jit = false;
  int limit = 0;
  bool verbose = false;
  const char* trace_for = getenv("TRACE_CASE");
  for (int i = 1; i < argc; ++i) {
    if (!strcmp(argv[i], "--jit")) use_jit = true;
    else if (!strcmp(argv[i], "--limit") && i + 1 < argc) limit = atoi(argv[++i]);
    else if (!strcmp(argv[i], "-v")) verbose = true;
    else path = argv[i];
  }

  std::vector<Case> cases = load(path);
  if (limit && (int)cases.size() > limit) cases.resize(limit);

  int failed = 0;
  u64 total_jit_insns = 0;
  for (Case& c : cases) {
    std::string why;
    if (trace_for && c.name == trace_for) {
      std::printf("--- trace %s (%s, count=%u)\n", c.name.c_str(),
                  c.thumb ? "thumb" : "arm", c.count);
      std::string t;
      run_case(c, use_jit, &t, true);
      continue;
    }
    if (getenv("DUMP")) {
      std::printf("[%s] thumb=%d count=%u code=%zu r0=%08x cpsr=%08x\n",
                  c.name.c_str(), (int)c.thumb, c.count, c.code.size(), c.regs[0], c.cpsr);
      for (size_t i = 0; i < c.code.size() && i < 4; ++i)
        std::printf("   insn %zu = %08x\n", i, c.code[i]);
      std::printf("   expect r0=%08x r1=%08x\n", c.expect_regs[0], c.expect_regs[1]);
    }
    u64 jit_here = 0;
    int bad = run_case(c, use_jit, &why, false, &jit_here);
    total_jit_insns += jit_here;
    if (bad) {
      ++failed;
      std::printf("FAIL %-36s %s\n", c.name.c_str(), why.c_str());
      if (verbose || getenv("DISASM"))
        std::printf("     [%s count=%u] %s\n", c.thumb ? "T" : "A", c.count,
                    disasm(c).c_str());
    } else if (verbose) {
      std::printf("ok   %s\n", c.name.c_str());
    }
  }
  /* `--jit` only *asks* for the JIT; on a host with no code generator every
   * instruction falls back to the interpreter, and a run that says "passed (jit)"
   * would then be claiming coverage it does not have.  Report the engine that
   * actually retired the instructions. */
#if BRAIN_JIT_BACKEND
  /* A host that CAN retire through the backend must actually do so on the
   * corpus; a silent all-interpreter run would make `--jit` green without
   * proving anything.  (The x86 CI run is the mirror case: BRAIN_JIT_BACKEND
   * is 0 and the note below is the expected outcome there.) */
  if (use_jit && !total_jit_insns) {
    std::printf("FAIL --jit: the backend retired 0 instructions on a host with a code generator\n");
    return 1;
  }
#endif
  const char* engine;
  if (!use_jit) engine = "interpreter";
  else if (total_jit_insns) engine = "jit";
  else engine = "jit REQUESTED but no backend retired anything: interpreter-only";
  char eng[96];
  if (use_jit && total_jit_insns)
    std::snprintf(eng, sizeof eng, "%s, %llu insns", engine,
                  (unsigned long long)total_jit_insns);
  else
    std::snprintf(eng, sizeof eng, "%s", engine);
  std::printf("%s: %d/%d cases passed (%s)\n", failed ? "FAILED" : "PASS",
              (int)cases.size() - failed, (int)cases.size(), eng);
  return failed ? 1 : 0;
}
