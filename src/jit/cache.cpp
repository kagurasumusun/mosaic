/* brain-emu JIT: the block cache.  See jit/cache.h for the invariants. */
#include "jit/cache.h"

#include <cstring>
#include <sys/mman.h>
#include <unistd.h>

#include "core/ops.h"       /* CycleModel */
#include "jit/frontend.h"
#include "jit/jit.h"        /* BRAIN_JIT_BACKEND, JITSLOW_* */

namespace brain {

/* When set, `jit_cache_enter` calls this instead of the arena code.  It lets the
 * dispatcher and the copy-in/copy-back run on hosts with no AArch64 execution;
 * tests/test_unit.cpp drives every block boundary through it. */
void (*jit_cache_exec_hook)(u32* shadow) = nullptr;

namespace {

constexpr u32 kTableBits = 13;
constexpr u32 kTableSize = 1u << kTableBits;
constexpr size_t kBlockCap = 1u << 14;   /* pointer-stability reserve; see below */

inline u32 hash_pc(u32 pc) { return (pc * 0x9E3779B1u) >> (32 - kTableBits); }

}  // namespace

/* Blocks live in `blocks` and lookup hands out pointers, so the vector must
 * never reallocate: it is pre-reserved and creation is refused past the
 * reserve.  Invalidation unlinks from the hash only; the arena is bump-style
 * and space returns at the next flush. */
JitCache* jit_cache_create(Cpu* cpu, u32 megabytes) {
  if (megabytes < 1) megabytes = 1;
  if (megabytes > 512) megabytes = 512;
  JitCache* jc = new JitCache();
  jc->cpu = cpu;
  jc->table.assign(kTableSize, 0);
  jc->tmask = kTableSize - 1;
  jc->blocks.reserve(kBlockCap);
  u64 cap = (u64)megabytes << 20;
  int prot = PROT_READ | PROT_WRITE | PROT_EXEC;
  void* p = mmap(nullptr, cap, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (p == MAP_FAILED) {           /* W^X policy: keep the bookkeeping testable */
    prot &= ~PROT_EXEC;
    p = mmap(nullptr, cap, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  }
  if (p == MAP_FAILED) { delete jc; return nullptr; }
  jc->arena = (u8*)p;
  jc->cap = cap;
  jc->arena_exec = (prot & PROT_EXEC) != 0;
  return jc;
}

void jit_cache_destroy(JitCache* jc) {
  if (!jc) return;
  if (jc->arena) munmap(jc->arena, jc->cap);
  delete jc;
}

void jit_cache_flush(JitCache* jc) {
  jc->blocks.clear();
  std::memset(jc->table.data(), 0, jc->table.size() * sizeof(u32));
  jc->used = 0;
}

JitBlock* jit_cache_lookup(JitCache* jc, u32 pc) {
  for (u32 i = jc->table[hash_pc(pc)]; i; i = jc->blocks[i - 1].next) {
    JitBlock& b = jc->blocks[i - 1];
    if (b.pc == pc) return &b;
  }
  return nullptr;
}

bool jit_cache_translate(JitCache* jc, u32 pc, JitBlock** out_block) {
  Cpu* c = jc->cpu;
  if (jc->blocks.size() >= kBlockCap) jit_cache_flush(jc);
  IrBlock blk;
  if (!jit_translate_arm_block(*c, pc & ~3u, c->cfg.max_block_insns, &blk))
    return false;
  std::vector<uint32_t> words;
  jit_emit_arm64(blk, /*with_entry_exit=*/true, &words);
  const u64 bytes = words.size() * sizeof(uint32_t);
  if (bytes == 0 || bytes > jc->cap) return false;
  if (jc->used + bytes > jc->cap) {
    jit_cache_flush(jc);           /* arena exhausted: drop everything and refill */
    if (jc->used + bytes > jc->cap) return false;
  }
  std::memcpy(jc->arena + jc->used, words.data(), bytes);
  JitBlock b;
  b.pc = pc & ~3u;
  b.end_pc = blk.next_pc;
  b.entry = (u32)jc->used;
  /* Straight-line DP only, and the interpreter charges every retired insn one
   * kIssue at entry to exec_arm_dec; extra cost (barrel shifts by register,
   * memory) can only appear on shapes the frontend refuses.  So the block's
   * cycle cost is exactly its instruction count under any model setting. */
  b.insns = blk.guest_insns;
  b.cycles = blk.guest_insns * (u32)CycleModel::kIssue;
  b.gen = c->trans_gen;
  jc->used += bytes;
  jc->blocks.push_back(b);
  JitBlock& installed = jc->blocks.back();
  const u32 idx = (u32)jc->blocks.size();            /* slot = idx - 1 */
  installed.next = jc->table[hash_pc(installed.pc)];
  jc->table[hash_pc(installed.pc)] = idx;
  c->stats.blocks++;
  c->stats.code_bytes += bytes;
  if (out_block) *out_block = &installed;
  return true;
}

void jit_cache_invalidate(JitCache* jc, u32 gaddr, u32 size) {
  if (!size) return;
  const u64 lo = gaddr, hi = (u64)gaddr + size;
  const u64 end = hi > 0xFFFFFFFFull ? 0xFFFFFFFFull : hi;
  for (u32 slot = 0; slot < kTableSize; ++slot) {
    u32 head = 0;                       /* rebuilt chain for this bucket */
    for (u32 i = jc->table[slot]; i;) {
      JitBlock& b = jc->blocks[i - 1];
      const u32 next = b.next;
      if ((u64)b.pc < end && (u64)b.end_pc > lo) {
        b.next = 0;                     /* unlink; entry stays until flush */
      } else {
        b.next = head;
        head = i;
      }
      i = next;
    }
    jc->table[slot] = head;
  }
}

void jit_cache_enter(JitCache* jc, JitBlock* b) {
  Cpu* c = jc->cpu;
  u32* s = jc->shadow;
  for (u32 i = 0; i < 13; ++i) s[i] = c->regs[i];
  s[16] = c->cpsr;                       /* CPSR_OFF: the shadow's 17th word   */
  if (jit_cache_exec_hook) {
    jit_cache_exec_hook(s);
  } else if (BRAIN_JIT_BACKEND && jc->arena_exec) {
    void (*fn)(u32*) = (void (*)(u32*))(jc->arena + b->entry);
    fn(s);
  }
  for (u32 i = 0; i < 13; ++i) c->regs[i] = s[i];
  /* Same merge the epilogue documents: NZCV from the host flags, everything
   * else already in the word the prologue loaded, and vflag kept in step with
   * the copy `interp_arm.cpp` maintains (`vflag = (cpsr >> 28) & 1`). */
  c->cpsr = s[16];
  c->vflag = (s[16] >> 28) & 1;
  c->cur = b->end_pc - 4;
  c->regs[15] = c->cur + 8;              /* the pipeline view ARM state reads  */
  c->npc = b->end_pc;
  c->insns += b->insns;
  c->cycles += b->cycles;
  c->stats.insns_jit += b->insns;
  c->stats.cycles_jit += b->cycles;
}

/* ------------------------------------------------------------------ loop -- */

/* One decision shared by both entry points: run the block at `npc`, or fall
 * back to the interpreter for one instruction.  Returns true if a block ran.
 * `max_insns` caps retirement (insn budgets; u64max for the cycle loop) and
 * `until_cycles` does the same for `Cpu::run`.  Conditions checked at the
 * boundary mirror `Cpu::step_one` exactly - an interrupt cannot be delivered
 * inside a block, so it must be seen before entry, and tracing forces the
 * slow path so every instruction still prints. */
namespace {

u32 jit_block_boundary(JitCache* jc, u64 max_insns, u64 until_cycles) {
  Cpu* c = jc->cpu;
  if (c->thumb || c->trace_en || c->cfg.trace || c->cfg.trace_jit) return 0;
  /* An asserted line must reach the machine at the next *instruction*, and
   * test_instr's interrupt-timing cases count instructions - inside a block
   * nobody can observe one, so a pending line keeps the dispatcher honest. */
  if (BRAIN_UNLIKELY(c->irq_line || c->fiq_line)) return 0;
  if (c->insns >= max_insns) return 0;
  JitBlock* b = jit_cache_lookup(jc, c->npc & ~3u);
  if (b && b->gen != c->trans_gen) {
    jit_cache_flush(jc);
    c->jit_slow = JITSLOW_STALE;
    return 0;
  }
  if (!b) {
    if (!jit_cache_translate(jc, c->npc & ~3u)) {
      c->jit_slow = JITSLOW_INTERPRET;
      return 0;
    }
    b = jit_cache_lookup(jc, c->npc & ~3u);
    if (!b) return 0;
  }
  if (c->insns + b->insns > max_insns ||
      (until_cycles != ~0ull && c->cycles + b->cycles > until_cycles))
    return 0;                            /* would over-run the budget          */
  const u32 retired = b->insns;
  jit_cache_enter(jc, b);
  return retired;
}

}  // namespace

void jit_cache_run_insns(JitCache* jc, u64 n) {
  Cpu* c = jc->cpu;
  const u64 start = c->insns;
  const u64 stop_at = start + n;
  while (c->insns < stop_at && !c->exit_req) {
    if (BRAIN_UNLIKELY(c->halted)) {
      const bool irq = c->irq_line && !(c->cpsr & CPSR_I);
      const bool fiq = c->fiq_line && !(c->cpsr & CPSR_F);
      if (!irq && !fiq) { c->cycles += 1; continue; }
      c->halted = 0;
    }
    /* take_exception() already calls Jit::on_exception (cpu.cpp); the loop
     * must not bump the generation a second time. */
    if (BRAIN_UNLIKELY(c->check_interrupts())) continue;
    if (BRAIN_UNLIKELY(c->exectrap)) { c->take_exception(c->exectrap); continue; }
    if (!jit_block_boundary(jc, stop_at, ~0ull) && c->insns < stop_at) {
      const u64 before = c->insns;
      c->step_one();                     /* advances one, or takes the abort   */
      if (c->insns == before) continue;  /* exception entry retired nothing    */
    }
  }
  c->jit_slow = JITSLOW_EXIT;
}

void jit_cache_run(JitCache* jc, u64 until_cycles) {
  Cpu* c = jc->cpu;
  while (!c->exit_req && c->cycles < until_cycles) {
    if (BRAIN_UNLIKELY(c->halted)) {
      const bool irq = c->irq_line && !(c->cpsr & CPSR_I);
      const bool fiq = c->fiq_line && !(c->cpsr & CPSR_F);
      if (!irq && !fiq) {
        c->cycles += 1;
        if (c->stop_after) { c->exit_req = 1; break; }
        continue;
      }
      c->halted = 0;
    }
    if (BRAIN_UNLIKELY(c->check_interrupts())) continue;
    if (BRAIN_UNLIKELY(c->exectrap)) { c->take_exception(c->exectrap); continue; }
    const u64 budget = c->stop_after ? c->insns + c->stop_after : ~0ull;
    const u64 got = jit_block_boundary(jc, budget, until_cycles);
    if (got) {
      if (c->stop_after)
        c->stop_after = c->stop_after > got ? (u32)(c->stop_after - got) : 0;
    } else {
      if (c->stop_after && c->insns >= budget) break;
      c->step_one();
      if (c->stop_after) c->stop_after -= 1;
    }
  }
  c->jit_slow = JITSLOW_EXIT;
}

}  // namespace brain
