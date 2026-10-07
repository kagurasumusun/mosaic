/* brain-emu JIT: control surface used by the core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The core treats the JIT as an optional accelerator with exactly the six
 * operations below, all cheap enough to call from the interpreter:
 *
 *   run/run_insns        execute until a cycle/instruction budget is met
 *   flush_all            drop every translated block (env change, reset)
 *   invalidate_range     drop blocks whose source bytes were written (SMC)
 *   on_tlb_flush         a CP15 operation changed the translation regime
 *   on_env_change        anything a compiled block assumed about the world
 *   on_exception         stop chaining: the next block must come from the
 *                        dispatcher so the handler is looked up afresh
 *
 * Translation-visible assumptions are versioned with `Cpu::trans_gen`: every
 * block records the generation it was built in and returns to the dispatcher
 * when it no longer matches, so a stale block can never execute an extra
 * instruction after a control-register write.
 *
 * A block is deliberately boring: one entry point per guest basic block, a fixed
 * prologue that loads the shadowed registers and the incoming flags out of
 * CpuState, and an epilogue that merges the flags back, stores only the registers
 * the block dirtied, sets R15 to the next PC and returns to the dispatcher.  The
 * ARM-specific logic exists once, in the IR of jit/ir.h, and a backend only ever
 * sees that.
 *
 * Two backends exist and the host picks one (BRAIN_JIT_BACKEND below): the
 * AArch64 one (jit/emit_arm64.h, checked against a model by
 * tests/test_jit_arm64.py) and the x86-64 one (jit/emit_x64.cpp, whose bytes
 * tests/test_jit_x64.cpp executes in-process against the interpreter).  Both are
 * fed by the same front end and see only the IR of jit/ir.h.  The block cache and
 * dispatcher live in jit/cache.h + jit/cache.cpp, driven by exactly the six entry
 * points below; tests/test_unit.cpp exercises them on any host, because its exec
 * hook stands in for the generated code.
 */
#ifndef BRAIN_JIT_JIT_H
#define BRAIN_JIT_JIT_H

#include "arm926/state.h"

/* Which host code generator is compiled in, and therefore whether the JIT can
 * retire anything at all:
 *
 *   1 = AArch64 (jit/emit_arm64.h, fed by jit/frontend.h; validated with
 *       tests/test_jit_arm64.py)
 *   2 = x86-64  (jit/emit_x64.h, fed by the same front end; validated with
 *       tests/test_jit_x64.cpp, which executes the emitted bytes in-process)
 *   0 = none of the above: Cpu::use_jit degrades to the interpreter even with
 *       the flag set, which is what `--jit` reports as "REQUESTED but no
 *       backend retired anything".
 *
 * The backend is picked by the host, not by the guest: a block is host machine
 * code, so only the machine it runs on can be the machine it is translated to.
 * Defining BRAIN_JIT_BACKEND yourself overrides the choice (a test compiling
 * against the emitter of a foreign host does exactly that, to *check* the
 * bytes without running them). */
#if !defined(BRAIN_JIT_BACKEND)
#  if defined(__aarch64__)
#    define BRAIN_JIT_BACKEND 1
#  elif defined(__x86_64__)
#    define BRAIN_JIT_BACKEND 2
#  else
#    define BRAIN_JIT_BACKEND 0
#  endif
#endif

namespace brain::arm926 {

struct JitCache;
struct JitBlock;

/* May a translated block access memory itself?  True only for the
 * identity-mapped RAM the fast path exists for: RAM enabled as a window, byte
 * order unchanged, and no alignment check to perform.  Both halves of the JIT
 * ask - the emitter, while translating, and the dispatcher, once per entry into
 * a block (as the `SH_MEM_FAST` word the emitted guard reads) - because a block
 * baked the window it saw at translation time into the guard and the window can
 * move. */
bool jit_fast_mem_ok(const Cpu* c);

/* Reasons for leaving generated code; reported in Cpu::jit_slow. */
enum : u32 {
  JITSLOW_NONE = 0,
  JITSLOW_EXIT,        /* budget reached                             */
  JITSLOW_EXCEPTION,   /* an exception was taken                     */
  JITSLOW_INTERPRET,   /* unsupported instruction: helper runs it    */
  JITSLOW_SMC,         /* code self-modified                         */
  JITSLOW_STALE,       /* trans_gen mismatch                         */
  JITSLOW_HALT,        /* WFI / stop request                        */
  JITSLOW_COUNT
};

class Jit {
 public:
  explicit Jit(Cpu* c) : cpu(c) {}
  ~Jit();

  /* Backend availability is decided here: if `init()` returns false the core
   * keeps the block cache absent and runs interpreted. */
  bool init();
  void shutdown();

  void run(u64 until_cycles);
  void run_insns(u64 n);

  void flush_all(const char* why);
  void invalidate_range(u32 gaddr, u32 size);
  void on_tlb_flush() { invalidate_all("tlb"); }
  void on_env_change(const char* why) { flush_all(why); }
  void on_exception();
  void request_retranslate() { retranslate = true; }

  bool available() const { return ready; }
  JitCache* cache() const { return jc; }

 private:
  void invalidate_all(const char* why);

  Cpu* cpu = nullptr;
  JitCache* jc = nullptr;
  bool ready = false;
  bool retranslate = false;
};

}  // namespace brain::arm926

#endif
