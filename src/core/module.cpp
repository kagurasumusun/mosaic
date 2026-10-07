/* brain-emu -- the module registry.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * One process-wide list per module kind, filled by `register_builtin_modules()`
 * (src/modules/builtin.cpp) with the modules this build contains.  Lookups are
 * linear over a handful of entries and happen at machine construction, never on
 * an execution path: a run loop that had to find a module per instruction would
 * be a design error, not a slow registry.
 */
#include "brainemu/core/module.h"

#include <cstring>

#include "brainemu/cpu/cpu.h"
#include "brainemu/loader/loader.h"

namespace brain {

const char* module_kind_name(ModuleKind k) {
  switch (k) {
    case ModuleKind::Cpu: return "cpu";
    case ModuleKind::Device: return "device";
    case ModuleKind::Loader: return "loader";
    case ModuleKind::Monitor: return "monitor";
    default: return "?";
  }
}

ModuleRegistry& ModuleRegistry::instance() {
  static ModuleRegistry r;
  return r;
}

void ModuleRegistry::add_cpu(const CpuModule* m) {
  if (!m || ncpus_ >= kMaxCpus) return;
  if (cpu_by_id(m->info().id)) return;              /* registered twice      */
  cpus_[ncpus_++] = m;
}

const CpuModule* ModuleRegistry::cpu_by_id(const char* id) const {
  if (!id || !*id) return nullptr;
  for (u32 i = 0; i < ncpus_; ++i)
    if (cpus_[i]->info().id && !std::strcmp(cpus_[i]->info().id, id))
      return cpus_[i];
  return nullptr;
}

const CpuModule* ModuleRegistry::cpu_by_index(u32 i) const {
  return i < ncpus_ ? cpus_[i] : nullptr;
}

u32 ModuleRegistry::cpu_count() const { return ncpus_; }

void ModuleRegistry::add_loader(const Loader* l) {
  if (!l || nloaders_ >= kMaxLoaders) return;
  if (loader_by_id(l->info().id)) return;
  loaders_[nloaders_++] = l;
}

const Loader* ModuleRegistry::loader_by_id(const char* id) const {
  if (!id || !*id) return nullptr;
  for (u32 i = 0; i < nloaders_; ++i)
    if (loaders_[i]->info().id && !std::strcmp(loaders_[i]->info().id, id))
      return loaders_[i];
  return nullptr;
}

const Loader* ModuleRegistry::loader_by_probe(const void* data, u64 size) const {
  for (u32 i = 0; i < nloaders_; ++i)
    if (loaders_[i]->probe(data, size)) return loaders_[i];
  return nullptr;
}

u32 ModuleRegistry::loader_count() const { return nloaders_; }

u32 ModuleRegistry::count() const { return ncpus_ + nloaders_; }

const ModuleInfo* ModuleRegistry::info(u32 i) const {
  if (i < ncpus_) return &cpus_[i]->info();
  i -= ncpus_;
  if (i < nloaders_) return &loaders_[i]->info();
  return nullptr;
}

}  // namespace brain
