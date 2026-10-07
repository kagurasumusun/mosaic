/* brain-emu -- modules: the one extension point of the core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The emulator is one core plus modules.  The core owns the address space, the
 * machine's lifetime and the C API; a module owns one piece of the emulated
 * world and registers itself under an id:
 *
 *   cpu      a processor: registers, an instruction set, its own timing
 *   device   something mapped into the address space (UART, timer, exit port)
 *   loader   an image format (ELF, raw, a firmware container)
 *   monitor  an observer of the running machine (tracer, profiler, gdb stub)
 *
 * A CPU module is the only kind the core *drives* (it executes when asked);
 * devices and monitors are called by the bus and the run loop, and loaders fill
 * memory.  Adding a processor therefore touches no core file - which is the
 * whole point of the split: the ARM926EJ-S is the first CPU module, not the
 * shape of the emulator.
 *
 * Registration is explicit rather than magically static-initialised, because
 * this library is a static archive and an unreferenced object file inside one is
 * simply not linked in: `register_builtin_modules()` (src/modules/builtin.cpp)
 * is the list the core calls, and a build that leaves a module out says so once,
 * in that file, instead of dropping it silently.
 */
#ifndef BRAINEMU_CORE_MODULE_H
#define BRAINEMU_CORE_MODULE_H

#include <memory>

#include "brainemu/cpu/cpu.h"
#include "brainemu/types.h"

namespace brain {

enum class ModuleKind : u32 {
  Cpu = 0,
  Device,
  Loader,
  Monitor,
  Count,
};

const char* module_kind_name(ModuleKind k);

struct ModuleInfo {
  const char* id = "";             /* "arm926": the id a caller selects by    */
  const char* name = "";           /* human-readable name                     */
  ModuleKind kind = ModuleKind::Cpu;
  const char* description = "";
};

/* A CPU module: a factory plus the identity of what it builds.  One instance
 * exists per module for the life of the process; the cores it creates are
 * independent of it and of each other. */
class CpuModule {
 public:
  virtual ~CpuModule() = default;
  virtual const ModuleInfo& info() const = 0;
  virtual std::unique_ptr<CpuCore> create() const = 0;
};

/* A loader module: `probe` claims an image by content and `load` places it.
 * Declared here and defined in brainemu/loader/loader.h, so a CPU module never
 * has to pull the loader interface in. */
class Loader;

/* The process-wide registry.  Not thread-safe by design: modules register
 * during start-up, before any machine exists. */
class ModuleRegistry {
 public:
  static ModuleRegistry& instance();

  void add_cpu(const CpuModule* m);
  const CpuModule* cpu_by_id(const char* id) const;
  const CpuModule* cpu_by_index(u32 i) const;
  u32 cpu_count() const;

  void add_loader(const Loader* l);
  const Loader* loader_by_id(const char* id) const;
  /* The loader that claims this image, or null: ELF by magic, raw as the
   * fallback the caller has to ask for explicitly. */
  const Loader* loader_by_probe(const void* data, u64 size) const;
  u32 loader_count() const;

  /* Every module kind, for a `--list-modules` in a tool or a GUI picker. */
  u32 count() const;
  const ModuleInfo* info(u32 i) const;

 private:
  ModuleRegistry() = default;
  static constexpr u32 kMaxCpus = 16;
  static constexpr u32 kMaxLoaders = 16;
  const CpuModule* cpus_[kMaxCpus] = {};
  u32 ncpus_ = 0;
  const Loader* loaders_[kMaxLoaders] = {};
  u32 nloaders_ = 0;
};

/* Called once by the core (Machine::init) - see the note at the top. */
void register_builtin_modules();

}  // namespace brain

#endif  /* BRAINEMU_CORE_MODULE_H */
