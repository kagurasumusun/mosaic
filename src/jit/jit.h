/* brain-emu JIT: control surface used by the core.
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
 * The AArch64 code generator (jit/emit_arm64.h, fed by jit/frontend.h) is complete
 * and verified against a real AArch64 model by tests/test_jit_arm64.py; the block
 * cache and dispatcher that give it a home live in jit/cache.h + jit/cache.cpp,
 * driven by exactly the six entry points below.  The one call `jit_cache_enter`
 * makes into host code is the only line that needs a runnable backend; everything
 * else in the cache is plain bookkeeping, which is what tests/test_unit.cpp
 * exercises (via its exec hook) on hosts where no code executes.
 */
#ifndef BRAIN_JIT_JIT_H
#define BRAIN_JIT_JIT_H

#include "core/state.h"

/* 1 on a host for which a code generator is compiled in (see emit_arm64.h; there
 * is no x64 backend).  Without one, Cpu::use_jit degrades to the interpreter - and
 * while the flag is 0 it degrades to the interpreter *with* the flag set, which is
 * what `--jit` reports: "REQUESTED but no backend retired anything".  Do not turn
 * this on for __aarch64__ until the cache above exists: the run path below calls
 * jit_cache_* functions that are only declared. */
#if !defined(BRAIN_JIT_BACKEND)
#  if defined(__aarch64__)
#    define BRAIN_JIT_BACKEND 1
#  else
#    define BRAIN_JIT_BACKEND 0
#  endif
#endif

namespace brain {

struct JitCache;
struct JitBlock;

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

  /* A conditional branch at the end of a block is only linked once the target
   * is known to be reachable; the interpreter calls this whenever it changes
   * the PC outside a branch (LDR PC, MOV PC, exception return). */
  void flush_pending_link() { pending_link = 0; }

  bool available() const { return ready; }
  JitCache* cache() const { return jc; }

 private:
  void invalidate_all(const char* why);

  Cpu* cpu = nullptr;
  JitCache* jc = nullptr;
  bool ready = false;
  bool retranslate = false;
  u32 pending_link = 0;
};

}  // namespace brain

#endif
