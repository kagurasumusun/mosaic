/* brain-emu -- image loaders: the format modules.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A loader turns a host buffer into guest memory.  It is a module like any
 * other, so a new firmware container or a new executable format is a new file
 * that registers itself, not a change to the machine:
 *
 *   * `probe` says whether the loader claims an image by its content (the ELF
 *     one checks the magic number and the class), which is what lets
 *     `Machine::load_image` pick a format without being told;
 *   * `load` fills the address space through the bus and reports the entry
 *     point and the range it touched.
 *
 * The two built-in loaders are the ones an ARM926EJ-S board actually boots from:
 * a flat binary at an address a boot ROM chose, and a static ET_EXEC ELF32 that
 * carries its own addresses.  Both write *physical* memory (the core comes out
 * of reset with the MMU off, so a virtual-only address would be a lie), both
 * refuse rather than half-load, and both leave the CPU's entry state to the
 * caller - `Machine::load_image` is what turns an entry address into a PC.
 */
#ifndef BRAINEMU_LOADER_LOADER_H
#define BRAINEMU_LOADER_LOADER_H

#include <cstddef>
#include <string>

#include "brainemu/core/module.h"
#include "brainemu/types.h"

namespace brain {

class Bus;

/* What a caller needs to know about a loaded image, whoever loaded it. */
struct ImageInfo {
  u32 entry = 0;         /* start address, bit0 = 1 for an alternate-state entry */
  const char* format = "";  /* loader id ("elf32", "raw")                      */
  u32 min = 0;           /* lowest physical byte touched                         */
  u32 max = 0;           /* one past the highest physical byte touched           */
  u32 machine = 0;       /* e_machine, for a format that has one                 */
  u32 type = 0;          /* e_type                                               */
  u32 segments = 0;      /* PT_LOAD count (1 for a raw image)                    */
  bool is_elf = false;
};

class Loader {
 public:
  virtual ~Loader() = default;
  virtual const ModuleInfo& info() const = 0;
  /* Does this loader claim `data` by its content? */
  virtual bool probe(const void* data, u64 size) const = 0;
  /* Place the image.  Returns 0 on success, -1 with `err` filled in. */
  virtual int load(Bus& bus, const void* data, u64 size, ImageInfo* out,
                   std::string& err) const = 0;
};

/* The built-in loaders.  `raw` needs an address (the caller has one: a boot ROM
 * would), so it is never auto-detected - `load` is called with it explicitly. */
const Loader& loader_elf32();
const Loader& loader_raw();

/* The implementations, for a caller that wants one of them directly. */
int elf32_load(Bus& bus, const void* image, size_t size, ImageInfo* out,
               std::string& err);
int raw_load(Bus& bus, const void* image, size_t size, u32 addr,
             std::string& err);

/* Put the built-in loaders in the registry.  Called by
 * `register_builtin_modules()` - and idempotent, so a tool that only wants the
 * loaders can call it on its own. */
void register_builtin_loaders();

}  // namespace brain

#endif  /* BRAINEMU_LOADER_LOADER_H */
