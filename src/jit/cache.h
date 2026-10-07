/* brain-emu JIT: the block cache and dispatcher the control plane (jit.cpp) drives.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0

 * One entry per guest ARM basic block as the frontend carves them (straight-line
 * data processing, ended at the first branch, memory access, R13-R15 reference or
 * anything the IR refuses to own).  The dispatcher loop below is the whole
 * machine: at each block boundary it checks interrupts and the budget exactly as
 * `Cpu::step_one()` does, runs one block, and returns to the interpreter for
 * whatever the block did not cover.  There is no chaining inside generated code -
 * every block ends by returning here with R15 set to the next guest address - so
 * an invalidation can never be outrun by a link written last time.
 *
 * Correctness rests on three invariants, each cheap to state:
 *
 *   - a block stores the `trans_gen` it was built under; any flush, TLB change
 *     or exception bumps the generation, and the dispatcher drops stale blocks
 *     instead of running them;
 *   - guest writes inside a block's `[pc, end_pc)` range invalidate it
 *     (self-modifying code goes through `Cpu::invalidate_code`);
 *   - between blocks the visible machine state is exactly what the
 *     interpreter would have after the same insns: the shadow register file
 *     copies in R0-R12 + CPSR and copies back what the epilogue wrote, then
 *     the dispatcher rebuilds the pipeline view (`cur`, `regs[15]`, `vflag`).
 *     What a block puts in the shadow beyond the register file is its own
 *     business, described by `JitBlock::meta` and jit/abi.h: the x86-64
 *     backend hands its flags over as an EFLAGS image plus a separate V bit,
 *     the AArch64 one writes the CPSR word itself.
 *
 * Budgets matter more than in the interpreter: `test_instr` stops the machine on
 * an exact instruction count and the cycle model feeds `bench`, so a block is
 * only entered when its whole instruction and cycle cost fits in what is left;
 * otherwise the boundary instruction is interpreted one at a time.  Tracing
 * forces the same fallback, so `--trace` output never misses an instruction.
 *
 * On a host with no code generator (see BRAIN_JIT_BACKEND) `Jit::init()` never
 * creates one of these and the interpreter runs unmodified.  The bookkeeping
 * here still compiles, and tests/test_unit.cpp exercises it directly:
 * install/lookup/invalidate/generation/budget decisions with `jit_cache_exec_hook`
 * standing in for the one line of host-code execution.
 */
#ifndef BRAIN_JIT_CACHE_H
#define BRAIN_JIT_CACHE_H

#include <cstdint>
#include <vector>

#include "core/cpu.h"
#include "jit/abi.h"
#include "jit/ir.h"

namespace brain {

struct JitBlock {
  u32 pc;        /* address of the first translated guest instruction     */
  u32 end_pc;    /* R15 after the block; the invalidation range too        */
  u32 entry;     /* byte offset of the emitted code inside `arena`         */
  u32 insns;     /* guest instructions retired (budget + stats)            */
  u32 cycles;    /* their cycle cost under the interpreter's model          */
  u32 gen;       /* Cpu::trans_gen at build time                            */
  u32 next;      /* hash-chain: index + 1 of the next block, 0 = end       */
  u8 meta;       /* JitBlockMeta: what the *dispatcher* must do around the
                  * block.  0 = the block owns its flags (AArch64); 1 = the
                  * block reads and writes the shadow's EFLAGS image and V
                  * (x86-64).  See jit/emit_x64.h. */
};

struct JitCache {
  Cpu* cpu = nullptr;
  std::vector<JitBlock> blocks;
  std::vector<u32> table;      /* index + 1 by hashed pc; 0 = empty slot     */
  u32 tmask = 0;
  u8* arena = nullptr;         /* the code itself; mmap'd, never a malloc'd  */
  u64 cap = 0, used = 0;       /* arena window; exhaustion flushes all        */
  u32 shadow[SH_WORDS];        /* the entry/exit handoff buffer (jit/abi.h)   */
  bool arena_exec = false;     /* false on hosts where mmap(RWX) was refused   */
  /* Negative cache: the last pc the frontend refused to start a block at, with
   * the generation it was refused in.  Without it a loop whose *branch* is
   * interpreted (which is every small loop, since a block ends at the branch)
   * re-runs the whole translation attempt on every iteration. */
  u32 bad_pc = 0, bad_gen = 0;
};

/* ---- created/owned by Jit (jit.cpp) -------------------------------------- */
JitCache* jit_cache_create(Cpu* cpu, u32 megabytes);
void jit_cache_destroy(JitCache*);
void jit_cache_flush(JitCache*);
void jit_cache_invalidate(JitCache*, u32 gaddr, u32 size);

/* ---- the two run-loop entries (called only when a backend executed) ------ */
void jit_cache_run(JitCache*, u64 until_cycles);
void jit_cache_run_insns(JitCache*, u64 n);

/* ---- exposed for tests (also used by the loop above) --------------------
 * `jit_cache_translate` returns false when the frontend cannot start a block
 * at `pc` at all - the caller then interprets.  `jit_cache_enter` is the whole
 * host execution step: copy shadow in, call the block, copy back, retire.
 * On a host with BRAIN_JIT_BACKEND == 0 the call itself is compiled out, so
 * tests exercise the bookkeeping (and the copy-back) without executing code. */
bool jit_cache_translate(JitCache*, u32 pc, JitBlock** out_block = nullptr);
JitBlock* jit_cache_lookup(JitCache*, u32 pc);
void jit_cache_enter(JitCache*, JitBlock*);

/* Installed by tests to stand in for the arena call (see cache.cpp). */
extern void (*jit_cache_exec_hook)(u32* shadow);

}  // namespace brain

#endif
