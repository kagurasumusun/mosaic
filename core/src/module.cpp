/* brain-emu -- the module registry.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * One process-wide list per module kind, filled by `register_builtin_modules()`
 * (src/modules/builtin.cpp) with the modules this build contains.  Lookups are
 * linear over a handful of entries and happen at machine construction, never on
 * an execution path: a run loop that had to find a module per instruction would
 * be a design error, not a slow registry.
 */
#include "brainemu/module.h"

#include <cstring>

#include "brainemu/cpu.h"
#include "brainemu/loader.h"

namespace brain {

const char* module_kind_name(ModuleKind k) {
  switch (k) {
    case ModuleKind::Cpu: return "cpu";
    case ModuleKind::Device: return "device";
    case ModuleKind::Loader: return "loader";
    case ModuleKind::Monitor: return "monitor";
    case ModuleKind::Board: return "board";
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

void ModuleRegistry::add_device(const DeviceModule* m) {
  if (!m || ndevices_ >= kMaxDevices) return;
  if (device_by_id(m->info().id)) return;
  devices_[ndevices_++] = m;
}

const DeviceModule* ModuleRegistry::device_by_id(const char* id) const {
  if (!id || !*id) return nullptr;
  for (u32 i = 0; i < ndevices_; ++i)
    if (devices_[i]->info().id && !std::strcmp(devices_[i]->info().id, id))
      return devices_[i];
  return nullptr;
}

const DeviceModule* ModuleRegistry::device_by_index(u32 i) const {
  return i < ndevices_ ? devices_[i] : nullptr;
}

u32 ModuleRegistry::device_count() const { return ndevices_; }

void ModuleRegistry::add_monitor(const MonitorModule* m) {
  if (!m || nmonitors_ >= kMaxMonitors) return;
  if (monitor_by_id(m->info().id)) return;
  monitors_[nmonitors_++] = m;
}

const MonitorModule* ModuleRegistry::monitor_by_id(const char* id) const {
  if (!id || !*id) return nullptr;
  for (u32 i = 0; i < nmonitors_; ++i)
    if (monitors_[i]->info().id && !std::strcmp(monitors_[i]->info().id, id))
      return monitors_[i];
  return nullptr;
}

const MonitorModule* ModuleRegistry::monitor_by_index(u32 i) const {
  return i < nmonitors_ ? monitors_[i] : nullptr;
}

u32 ModuleRegistry::monitor_count() const { return nmonitors_; }

void ModuleRegistry::add_board(const BoardModule* b) {
  if (!b || nboards_ >= kMaxBoards) return;
  if (board_by_id(b->info().id)) return;
  boards_[nboards_++] = b;
}

const BoardModule* ModuleRegistry::board_by_id(const char* id) const {
  if (!id || !*id) return nullptr;
  for (u32 i = 0; i < nboards_; ++i)
    if (boards_[i]->info().id && !std::strcmp(boards_[i]->info().id, id))
      return boards_[i];
  return nullptr;
}

const BoardModule* ModuleRegistry::board_by_index(u32 i) const {
  return i < nboards_ ? boards_[i] : nullptr;
}

u32 ModuleRegistry::board_count() const { return nboards_; }

u32 ModuleRegistry::count() const {
  return ncpus_ + nloaders_ + ndevices_ + nmonitors_ + nboards_;
}

const ModuleInfo* ModuleRegistry::info(u32 i) const {
  if (i < ncpus_) return &cpus_[i]->info();
  i -= ncpus_;
  if (i < nloaders_) return &loaders_[i]->info();
  i -= nloaders_;
  if (i < ndevices_) return &devices_[i]->info();
  i -= ndevices_;
  if (i < nmonitors_) return &monitors_[i]->info();
  i -= nmonitors_;
  if (i < nboards_) return &boards_[i]->info();
  return nullptr;
}

const char* ModuleRegistry::ids(ModuleKind kind) const {
  char* out = idbuf_;
  char* end = idbuf_ + sizeof(idbuf_) - 1;
  u32 n = 0;
  const ModuleInfo* mi = nullptr;
  auto append = [&](const ModuleInfo& m) {
    if (out >= end) return;
    if (n++) {
      *out++ = ',';
      *out++ = ' ';
      *out++ = ' ';
    }
    for (const char* p = m.id; *p && out < end; ++p) *out++ = *p;
  };
  for (u32 i = 0; (mi = info(i)) != nullptr; ++i)
    if (mi->kind == kind) append(*mi);
  if (!n) {
    const char* none = "(none)";
    for (const char* p = none; *p && out < end; ++p) *out++ = *p;
  }
  *out = 0;
  return idbuf_;
}

}  // namespace brain
