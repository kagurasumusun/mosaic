/* brain-emu JIT: the block cache.  See jit/cache.h for the invariants.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include "machines/arm926/jit/cache.h"

#include <algorithm>
#include <cstring>
#include <sys/mman.h>
#include <unistd.h>

#include "machines/arm926/cpu/ops.h"       /* CycleModel */
#include "machines/arm926/jit/abi.h"        /* SH_*: the shadow layout both sides agree on */
#include "machines/arm926/jit/emit_x64.h"   /* the x86-64 backend's entry point + meta      */
#include "machines/arm926/jit/frontend.h"
#include "machines/arm926/jit/jit.h"        /* BRAIN_JIT_BACKEND, JITSLOW_* */

namespace brain::arm926 {

/* When set, `jit_cache_enter` calls this instead of the arena code.  It lets the
 * dispatcher and the copy-in/copy-back run on hosts with no AArch64 execution;
 * tests/test_unit.cpp drives every block boundary through it. */
void (*jit_cache_exec_hook)(u32* shadow, const JitBlock* b) = nullptr;

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
  jc->codemap.assign(1u << 20, 0);        /* one byte per guest page          */
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
  jit_cache_flush(jc);            /* drop the code-page flags and the guard  */
  if (jc->arena) munmap(jc->arena, jc->cap);
  delete jc;
}

/* Self-modifying code, guest side of it: every guest page a block was built
 * from is flagged in the bus page table, and `Bus::note_store` tests that flag
 * on every store that could land in one (see include/brainemu/core/bus.h).
 * That keeps the common store - RAM that holds data, not code - down to one
 * load and a branch, while a store into a page that *might* hold code calls
 * back here and drops the blocks that cover it.
 *
 * The flag is a conservative over-approximation on purpose.  It is set when a
 * block is installed and cleared only by a flush - the one operation that
 * drops every block - so no bookkeeping can drift out of step with the block
 * list: a set flag may find no block at all (one wasted range check), but a
 * live block can never sit on a page whose flag has been cleared.  A guest
 * that writes its own code therefore cannot outrun its translations, which is
 * the only property that matters here; a page that stops holding code pays for
 * the flag until the next flush. */
static void jit_mark_block_pages(JitCache* jc, u32 pc, u32 end_pc, bool code) {
  Bus* bus = jc->cpu->bus;
  if (end_pc <= pc) end_pc = pc + 4;
  for (u64 p = pc & ~(u64)0xFFF; p < end_pc; p += 0x1000) {
    if (code) {
      /* The bus flag is what the interpreter's store path tests before it
       * reports; the JIT's own map is what generated code tests before it
       * stores.  Both say "this page holds translated code", so they are set
       * and cleared together, here. */
      if (bus) bus->mark_code_page((u32)p, true);
      jc->codemap[(u32)p >> 12] = 1;
    } else if (!bus || bus->page_has_code((u32)p)) {
      if (bus) bus->mark_code_page((u32)p, false);
      jc->codemap[(u32)p >> 12] = 0;
    }
  }
}

/* The store guard's on/off switch: the interpreter and the JIT's slow path ask
 * `Cpu::smc_watch` before every store, so it must be true exactly while some
 * page still carries the flag.  Installing a block makes that true; a flush
 * drops every block and clears every flag, which makes it false again.  The
 * interpreter alone (no Jit, or a host with no backend) never sets it, and its
 * stores stay on the path they would have had anyway. */
static void jit_cache_set_watch(JitCache* jc, bool on) { jc->cpu->smc_watch = on ? 1u : 0u; }

void jit_cache_flush(JitCache* jc) {
  /* Every page any of these blocks came from loses its flag, live or already
   * unlinked: after a flush the cache is empty, so no page may keep the flag. */
  for (size_t i = 0; i < jc->blocks.size(); ++i)
    jit_mark_block_pages(jc, jc->blocks[i].pc, jc->blocks[i].code_end, false);
  jc->blocks.clear();
  /* Nothing is left to have come from any page, so the whole map goes - cheaper
   * than walking it, and it cannot disagree with the empty block list. */
  std::fill(jc->codemap.begin(), jc->codemap.end(), (u8)0);
  std::memset(jc->table.data(), 0, jc->table.size() * sizeof(u32));
  jc->used = 0;
  jit_cache_set_watch(jc, false);
}

JitBlock* jit_cache_lookup(JitCache* jc, u32 pc) {
  /* The thumb bit is compared, not just hashed: the same pc decoded in the two
   * states is two different blocks, and entering the wrong one would run ARM
   * words as Thumb - or the other way round, which is worse because it looks
   * like it works. */
  const u32 want = jc->cpu ? jc->cpu->thumb : 0u;
  for (u32 i = jc->table[hash_pc(pc)]; i; i = jc->blocks[i - 1].next) {
    JitBlock& b = jc->blocks[i - 1];
    if (b.pc == pc && b.thumb == want) return &b;
  }
  return nullptr;
}

/* The one place that knows which host code generator is compiled in.
 *
 * AArch64's emitter writes 32-bit words and returns false only when the *host*
 * is not one it can be executed on (its words are still produced, and on a host
 * where nothing runs them the cache is never entered anyway); x86-64's emitter
 * returns false when it cannot emit the block, and says so through `meta`.
 * Keeping the difference here means the rest of the cache - and every test that
 * drives it with `jit_cache_exec_hook` - is backend-agnostic. */
bool backend_emit(const IrBlock& blk, std::vector<u8>* out, u8* meta) {
  *meta = 0;
#if BRAIN_JIT_BACKEND == 2
  JitBlockMeta m;
  if (!jit_emit_x64(blk, /*with_entry_exit=*/true, out, &m)) return false;
  *meta = (u8)(m.entry_flags | (m.exit_flags << 1));
  return true;
#elif BRAIN_JIT_BACKEND == 1
  std::vector<uint32_t> words;
  jit_emit_arm64(blk, /*with_entry_exit=*/true, &words);
  out->assign((const u8*)words.data(), (const u8*)words.data() + words.size() * 4);
  return !words.empty();
#else
  (void)blk; (void)out; (void)meta;
  return false;
#endif
}

bool jit_cache_translate(JitCache* jc, u32 pc, JitBlock** out_block) {
  Cpu* c = jc->cpu;
  if (jc->blocks.size() >= kBlockCap) jit_cache_flush(jc);
  const bool thumb = c->thumb != 0;
  pc &= thumb ? ~1u : ~3u;            /* a pc is halfword or word aligned */
  IrBlock blk;
  if (!jit_translate_block(*c, pc, c->cfg.max_block_insns, &blk)) return false;
  std::vector<u8> code;
  u8 meta = 0;
  if (!backend_emit(blk, &code, &meta)) return false;
  const u64 bytes = code.size();
  if (bytes == 0 || bytes > jc->cap) return false;
  if (jc->used + bytes > jc->cap) {
    jit_cache_flush(jc);           /* arena exhausted: drop everything and refill */
    if (jc->used + bytes > jc->cap) return false;
  }
  std::memcpy(jc->arena + jc->used, code.data(), bytes);
  JitBlock b;
  b.pc = pc;
  b.code_end = blk.code_end ? blk.code_end : blk.start_pc + (thumb ? 2u : 4u);
  b.end_pc = blk.next_pc;
  b.entry = (u32)jc->used;
  /* The front end summed the per-instruction cost under the interpreter's own
   * model (kIssue everywhere, plus kAccess for an access and kBranch for a
   * taken branch), so a block's cycles are exactly what the interpreter would
   * have charged for the same instructions - which is what lets the cycle
   * budget in the dispatcher treat the two engines interchangeably. */
  b.insns = blk.guest_insns;
  b.cycles = blk.cycles;
  b.gen = c->trans_gen;
  b.meta = meta;
  b.uses_mem = blk.uses_mem ? 1 : 0;
  b.mem_gbase = blk.mem_gbase;
  b.mem_mask = blk.mem_mask;
  b.br = 0;
  if (blk.n && blk.in[blk.n - 1].op == IR_BR)
    b.br = blk.in[blk.n - 1].cond == 0xEu ? 1 : 2;
  b.dead = 0;
  b.thumb = thumb ? 1 : 0;
  jc->used += bytes;
  jc->blocks.push_back(b);
  jit_mark_block_pages(jc, b.pc, b.code_end, true);
  jit_cache_set_watch(jc, true);
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
      if ((u64)b.pc < end && (u64)b.code_end > lo) {
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

#if BRAIN_JIT_BACKEND == 2
/* x86-64 keeps N, Z and C in the host flag register and V in a register of its
 * own, because those are the four operations the host has: SF/ZF/CF fall out of
 * `add`/`sub`/`and`/shifts exactly where ARM wants them, while OF has no cheap
 * writer (`seto` sets it, nothing restores it).  So the handoff is an EFLAGS
 * image - loadable with one `popfq`, readable with one `pushfq` - plus a 0/1 V
 * word.  Building the image here rather than in the block's prologue is worth
 * five instructions per entry, and it keeps the layout knowledge in one file. */
/* True while a guest access may be done inline: RAM straight through, no
 * alignment check to perform, no halfword lane swap.  When this is false a
 * block translates no access at all and the interpreter runs those
 * instructions - which is also what happens whenever the MMU is on, so a
 * translated block is only fast on the identity-mapped RAM the fast path
 * exists for.
 *
 * The store watch (`Cpu::smc_watch`) is deliberately *not* part of this.  A
 * block *does* perform guest stores, but an inline store cannot call the
 * observer, so the emitter guards every store with the code-page map the watch
 * maintains and leaves the block (bailing to the interpreter, whose write path
 * does the invalidating) the moment the address is code.  Adding the watch here
 * would only turn the fast path off wholesale; the guard is what keeps the
 * "every writer reports" invariant at a cost of one load. */
bool jit_fast_mem_ok(const Cpu* c) {
  return c->cfg.jit_fast_mem && c->ram_fast && !c->align_chk && !c->be_data;
}

static u32 eflags_image(const Cpu* c) {
  u32 f = 0x2;                            /* bit 1 of RFLAGS reads as one    */
  if (c->cpsr & (1u << 29)) f |= 0x0001;  /* guest C -> CF                   */
  if (c->cpsr & (1u << 30)) f |= 0x0040;  /* guest Z -> ZF                   */
  if (c->cpsr & (1u << 31)) f |= 0x0080;  /* guest N -> SF                   */
  return f;
}

static void jit_cache_prepare(JitCache* jc, u32* s) {
  Cpu* c = jc->cpu;
  s[SH_EFLAGS / 4] = eflags_image(c);
  s[SH_VFLAG / 4] = (c->cpsr >> 28) & 1;
  s[SH_ABORT_PC / 4] = 0;
  s[SH_RETIRED / 4] = 0;
  /* The Cpu* and the fast-memory verdict: one load each in a block that wants
   * a helper or a guest access, instead of re-deriving the world per access. */
  u64 cpu_ptr = (u64)(uintptr_t)c;
  std::memcpy(&s[SH_CPU / 4], &cpu_ptr, sizeof cpu_ptr);
  /* The two pointers generated code needs to do an access itself: the host base
   * of the identity RAM window, and the code-page map a store tests. */
  u64 ram_ptr = (u64)(uintptr_t)c->ram;
  std::memcpy(&s[SH_RAM / 4], &ram_ptr, sizeof ram_ptr);
  u64 map_ptr = (u64)(uintptr_t)jc->codemap.data();
  std::memcpy(&s[SH_CODEMAP / 4], &map_ptr, sizeof map_ptr);
  s[SH_MEM_FAST / 4] = jit_fast_mem_ok(c) ? 1u : 0u;
}

/* Merge what the block left in the shadow back into the CPSR.  The block wrote
 * neither the CPSR word nor anything but the flag halves, so this is the same
 * arithmetic `interp_arm.cpp` does when it updates `vflag` after a flag write. */
static void jit_cache_finish(JitCache* jc, u32* s) {
  (void)jc;
  const u32 f = s[SH_EFLAGS / 4];
  u32 cpsr = s[SH_CPSR / 4] & ~0xF0000000u;
  if (f & 0x0080) cpsr |= 0x80000000u;
  if (f & 0x0040) cpsr |= 0x40000000u;
  if (f & 0x0001) cpsr |= 0x20000000u;
  if (s[SH_VFLAG / 4] & 1) cpsr |= 0x10000000u;
  s[SH_CPSR / 4] = cpsr;
}
#endif

u32 jit_cache_enter(JitCache* jc, JitBlock* b) {
  Cpu* c = jc->cpu;
  u32* s = jc->shadow;
  /* SH_REGS_IN words, not thirteen: R13 and R14 are part of the block's window
   * for the same reason they are on the way back out (see below). */
  for (u32 i = 0; i < SH_REGS_IN; ++i) s[i] = c->regs[i];
  s[16] = c->cpsr;                       /* CPSR_OFF: the shadow's 17th word   */
#if BRAIN_JIT_BACKEND == 2
  /* meta&1: the block reads the entry flags; meta&2: it writes the flag halves
   * at exit (and so needs the entry V even if it never reads a flag).  A block
   * that performs accesses needs the handoff words under any meta: it loads
   * the RAM window pointer and the fast-memory verdict from them. */
  if ((b->meta & 3) || b->uses_mem) jit_cache_prepare(jc, s);
#endif
  if (jit_cache_exec_hook) {
    jit_cache_exec_hook(s, b);
  } else if (BRAIN_JIT_BACKEND && jc->arena_exec) {
    void (*fn)(u32*) = (void (*)(u32*))(jc->arena + b->entry);
    fn(s);
  }
  /* R0-R12 plus R13/R14: a block may use the stack pointer as a base (with a
   * writeback that lands in the shadow) and it *does* write the link register
   * for a `bl`/`blx`, so both words are outputs as well as inputs - and the
   * copy *in* has to cover them too, or the block would read whatever the
   * previous entry left there.  A register the block never touched still holds
   * the entry value, so the copy back is a no-op for it.  R15 is the address to
   * resume at, not a register, and is handled below. */
  for (u32 i = 0; i < SH_REGS_IN; ++i) c->regs[i] = s[i];
#if BRAIN_JIT_BACKEND == 2
  /* Same merge the epilogue documents: NZCV from the host flags, everything
   * else already in the word the prologue loaded, and vflag kept in step with
   * the copy `interp_arm.cpp` maintains (`vflag = (cpsr >> 28) & 1`). */
  if (b->meta & 2) jit_cache_finish(jc, s);
#endif
  c->cpsr = s[16];
  c->vflag = (s[16] >> 28) & 1;

  /* What the block did, and where it left the machine.  A block that ran to its
   * exit wrote the address to resume at (the target of the branch it took, or
   * the fall-through) and its own cost; one that bailed wrote the address of the
   * instruction it would not perform and the count/cost of the part it did. */
  u32 retired = b->insns, cycles = b->cycles, resume = b->end_pc;
  bool bailed = false;
  u32 branch = 0;                        /* 0 none, 1 taken, 2 not taken */
#if BRAIN_JIT_BACKEND == 2
  retired = s[SH_RETIRED / 4];
  cycles = s[SH_CYCLES / 4];
  resume = s[SH_R15 / 4];
  bailed = s[SH_ABORT_PC / 4] != 0;
  if (!bailed) {
    if (b->br == 1) branch = 1;
    else if (b->br == 2) branch = s[SH_BRANCH / 4] ? 1 : 2;
  }
#else
  /* The AArch64 backend does not translate branches (see jit_emit_arm64), so a
   * block always retires whole and leaves the fall-through. */
  (void)branch;
#endif
  /* The pipeline view the interpreter's own fetch would leave: `cur` is the
   * instruction that has just retired and R15 the value it read, while `npc` is
   * where the next fetch starts (`Cpu::fetch_and_execute` does the same three
   * assignments at the top of a step).  For everything but a bail "just retired"
   * is the block's last word; a bail never ran that word, so there the last one
   * is its predecessor and `npc` is the word the interpreter must now run. */
  /* A block hands back the address to resume at with the *next state* in bit 0,
   * which is the same convention the instruction set itself uses for a
   * `bx`/`blx` target: 1 = Thumb, 0 = ARM.  The frontend puts it there (every
   * Thumb exit address is `| 1`, a BLX leaving Thumb is masked to a word, a
   * register branch keeps the register's own bit), and turning it into the
   * machine's state is the dispatcher's job, because `thumb` and CPSR.T belong
   * to the interpreter.  A block that ends in ARM state only ever runs in ARM
   * state, so for it the bit can only add a switch *into* Thumb - but a block
   * built in Thumb state may legitimately leave for ARM, and then the same rule
   * takes the state away. */
  const bool thumb_out = (resume & 1u) != 0;
  if (thumb_out != (c->thumb != 0)) {
    c->set_thumb(thumb_out ? 1 : 0);
    c->cpsr = (c->cpsr & ~(CPSR_T | CPSR_J)) | (thumb_out ? u32(CPSR_T) : 0u);
  }
  resume &= thumb_out ? ~1u : ~3u;
  const u32 isize = b->thumb ? 2u : 4u;
  const u32 last = bailed ? resume - isize : b->code_end - isize;
  c->cur = last;
  /* The pipeline value the interpreter's own fetch would have left: `cur + 8`
   * in ARM state, `cur + 4` in Thumb (A2.4.3), and R15 is not a register a
   * block can name, so this is the one place it is reconstructed. */
  c->regs[15] = last + (b->thumb ? 4u : 8u);
  c->npc = resume;
  c->insns += retired;
  c->cycles += cycles;
  c->stats.insns_jit += retired;
  c->stats.cycles_jit += cycles;
  /* `stats.branch_taken/branch_not` are part of what a run reports and the
   * interpreter counts them per branch instruction; a block performs one branch
   * (its last instruction) and is the only thing that knows the outcome.  A
   * bail never reached the branch, so nothing is counted - its instruction
   * count says so and the interpreter will count it when it gets there. */
  if (branch == 1) c->stats.branch_taken++;
  else if (branch == 2) c->stats.branch_not++;
  return retired;
}

/* ------------------------------------------------------------------ loop -- */

/* One decision, used by the one run loop: run the block at `npc`, or fall back
 * to the interpreter for one instruction.  Returns true if a block ran.
 * `max_insns`/`until_cycles` are the loop's absolute limits on the two counters
 * (`~0ull` = none), and a block that would overrun either is not entered - the
 * instruction that crosses the line is interpreted instead, so the loop never
 * overshoots the instruction budget and overshoots a cycle budget by at most the
 * one instruction the interpreter would also have run.  Conditions checked at the
 * boundary mirror `Cpu::step_one` exactly - an interrupt cannot be delivered
 * inside a block, so it must be seen before entry, and tracing forces the
 * slow path so every instruction still prints. */
namespace {

u32 jit_block_boundary(JitCache* jc, u64 max_insns, u64 until_cycles) {
  Cpu* c = jc->cpu;
  if (c->trace_en || c->cfg.trace || c->cfg.trace_jit) return 0;
  /* An asserted line must reach the machine at the next *instruction*, and
   * test_instr's interrupt-timing cases count instructions - inside a block
   * nobody can observe one, so a pending line keeps the dispatcher honest. */
  if (BRAIN_UNLIKELY(c->irq_line || c->fiq_line)) return 0;
  if (c->insns >= max_insns) return 0;
  const u32 pc = c->npc & (c->thumb ? ~1u : ~3u);
  if (BRAIN_LIKELY(pc == jc->bad_pc && jc->bad_gen == c->trans_gen &&
                   jc->bad_thumb == (c->thumb ? 1u : 0u)))
    return 0;
  JitBlock* b = jit_cache_lookup(jc, pc);
  if (b && b->dead) return 0;
  /* A block that performs accesses baked the RAM window it saw into its
   * guards, so it may only be entered while that window is still the machine's
   * - and while a translated access is still something the interpreter's own
   * fast path would do (MMU off, alignment check off, byte order unchanged).
   * Those last three live in SCTLR, and a write to SCTLR flushes every block
   * (see Cpu::on_translation_env_changed), so only the bus window can actually
   * move under a live block - the rest of the test is what the front end asked
   * at translation time, asked again because it costs three compares. */
  if (b && b->uses_mem &&
      (!jit_fast_mem_ok(c) || b->mem_gbase != c->ram_gbase ||
       b->mem_mask != c->ram_mask))
    return 0;
  if (b && b->gen != c->trans_gen) {
    jit_cache_flush(jc);
    c->jit_slow = JITSLOW_STALE;
    return 0;
  }
  if (!b) {
    if (!jit_cache_translate(jc, pc)) {
      /* Remember the refusal: the next iteration of this loop will ask again,
       * and the answer cannot change until the translation regime does. */
      jc->bad_pc = pc;
      jc->bad_gen = c->trans_gen;
      jc->bad_thumb = c->thumb ? 1 : 0;
      c->jit_slow = JITSLOW_INTERPRET;
      return 0;
    }
    b = jit_cache_lookup(jc, pc);
    if (!b) return 0;
  }
  if (c->insns + b->insns > max_insns ||
      (until_cycles != ~0ull && c->cycles + b->cycles > until_cycles))
    return 0;                            /* would over-run the budget          */
  const u32 retired = jit_cache_enter(jc, b);
  if (retired == 0) {
    /* The block bailed at its first instruction: the machine cannot get
     * anywhere by entering it again, and the instruction it refused must run in
     * the interpreter.  Remember that, so the next boundary does not spin. */
    b->dead = 1;
    return 0;
  }
  return retired;
}

}  // namespace

void jit_cache_run_until(JitCache* jc, u64 max_insns, u64 max_cycles) {
  Cpu* c = jc->cpu;
  /* The two budgets as absolute targets, set once: a block is entered only while
   * both still have room, and the *counters* are what the loop tests, so a
   * halted CPU - which charges cycles and retires nothing - still reaches the
   * cycle bound and hands control back to the machine, whose devices are what
   * will wake it up. */
  const u64 insn_stop = max_insns ? c->insns + max_insns : ~0ull;
  const u64 cyc_stop = max_cycles ? c->cycles + max_cycles : ~0ull;
  while (c->insns < insn_stop && c->cycles < cyc_stop && !c->exit_req) {
    if (BRAIN_UNLIKELY(c->halted)) {
      const bool irq = c->irq_line && !(c->cpsr & CPSR_I);
      const bool fiq = c->fiq_line && !(c->cpsr & CPSR_F);
      if (!irq && !fiq) {
        c->cycles += 1;
        continue;
      }
      c->halted = 0;
    }
    /* take_exception() already calls Jit::on_exception (cpu.cpp); the loop
     * must not bump the generation a second time. */
    if (BRAIN_UNLIKELY(c->check_interrupts())) continue;
    if (BRAIN_UNLIKELY(c->exectrap)) { c->take_exception(c->exectrap); continue; }
    if (!jit_block_boundary(jc, insn_stop, cyc_stop) && c->insns < insn_stop &&
        c->cycles < cyc_stop) {
      const u64 before = c->insns;
      c->step_one();                     /* advances one, or takes the abort   */
      if (c->insns == before) continue;  /* exception entry retired nothing    */
    }
  }
  c->jit_slow = JITSLOW_EXIT;
}

}  // namespace brain::arm926
