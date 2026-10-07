/* JIT control plane.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * With no host backend compiled in (see BRAIN_JIT_BACKEND) this file is the
 * whole JIT: it keeps the invalidation/versioning protocol that the core
 * expects, so `use_jit` simply degrades to the interpreter instead of changing
 * behaviour.  The AArch64 backend plugs into the same entry points; it owns the
 * code cache and reads `trans_gen` from the block epilogue.
 */
#include <cstdio>

#include "machines/arm926/jit/jit.h"
#include "machines/arm926/jit/cache.h"

namespace brain::arm926 {

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
  /* Every block is dropped: the handler is fetched through the dispatcher, so
   * its vector (which depends on CP15 c1[V]) is re-read, and no block may still
   * be holding a translation of the code that was interrupted. */
  ++cpu->trans_gen;
}

void Jit::run_until(u64 max_insns, u64 max_cycles) {
#if BRAIN_JIT_BACKEND
  if (ready) { jit_cache_run_until(jc, max_insns, max_cycles); return; }
#else
  (void)max_insns;
  (void)max_cycles;
#endif
  cpu->run_until(max_insns, max_cycles);   /* unreachable: init() failed     */
}

}  // namespace brain::arm926
