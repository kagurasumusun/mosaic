/* JIT control plane.
 *
 * With no host backend compiled in (see BRAIN_JIT_BACKEND) this file is the
 * whole JIT: it keeps the invalidation/versioning protocol that the core
 * expects, so `use_jit` simply degrades to the interpreter instead of changing
 * behaviour.  The AArch64 backend plugs into the same entry points; it owns the
 * code cache and reads `trans_gen` from the block epilogue.
 */
#include <cstdio>

#include "jit/jit.h"

namespace brain {

Jit::~Jit() { shutdown(); }

bool Jit::init() {
#if !BRAIN_JIT_BACKEND
  return false;                      /* interpreter only on this host      */
#else
  jc = jit_cache_create(cpu, cpu->cfg.jit_cache_mb);
  if (!jc) return false;
  ready = true;
  return true;
#endif
}

void Jit::shutdown() {
  if (!jc) return;
#if BRAIN_JIT_BACKEND
  jit_cache_destroy(jc);
#endif
  jc = nullptr;
  ready = false;
}

/* Every invalidation is also a generation bump: a block that is still being
 * executed notices the change at its next boundary and refuses to link. */
void Jit::flush_all(const char* why) {
  ++cpu->trans_gen;
  if (why && cpu->cfg.trace) std::fprintf(stderr, "[jit] flush_all: %s\n", why);
  cpu->stats.flushes++;
#if BRAIN_JIT_BACKEND
  if (jc) jit_cache_flush(jc);
#endif
}

void Jit::invalidate_all(const char* why) { flush_all(why); }

void Jit::invalidate_range(u32 gaddr, u32 size) {
  cpu->stats.smc_flushes++;
#if BRAIN_JIT_BACKEND
  if (jc) jit_cache_invalidate(jc, gaddr, size);
#else
  (void)gaddr;
  (void)size;
#endif
}

void Jit::on_exception() {
  /* Stop block chaining; the handler is fetched through the dispatcher so its
   * vector (which depends on CP15 c1[V]) is re-read. */
  pending_link = 0;
  ++cpu->trans_gen;
}

void Jit::run(u64 until_cycles) {
#if BRAIN_JIT_BACKEND
  if (ready) { jit_cache_run(jc, until_cycles); return; }
#else
  (void)until_cycles;
#endif
  cpu->run(until_cycles);            /* unreachable: init() failed         */
}

void Jit::run_insns(u64 n) {
#if BRAIN_JIT_BACKEND
  if (ready) { jit_cache_run_insns(jc, n); return; }
#else
  (void)n;
#endif
  cpu->run_insns(n);
}

}  // namespace brain
