/* brain-emu -- ELF32 loader for the ARM926EJ-S core.
 * SPDX-file-note: see LICENSE.
 *
 * Only what a bootable, statically linked armel binary needs: PT_LOAD segments
 * copied to their *physical* address (the core has an MMU but it comes out of
 * reset disabled, so a virtual address would be a lie), zero-filled bss, and a
 * refusal to guess at anything else.
 */
#ifndef BRAINEMU_LOADER_LOADER_H
#define BRAINEMU_LOADER_LOADER_H

#include <cstddef>
#include <string>

#include "brainemu/types.h"

namespace brain {

struct Cpu;

/* What a caller needs to know about a loaded image. */
struct ImageInfo {
  u32 entry = 0;       /* start address, bit0 = 1 for a Thumb entry          */
  u32 min = 0;        /* lowest physical byte touched                       */
  u32 max = 0;        /* one past the highest physical byte touched         */
  u32 machine = 0;     /* e_machine                                          */
  u32 type = 0;        /* e_type                                             */
  u32 segments = 0;    /* PT_LOAD count                                      */
  bool is_elf = false;
};

/* Load an ELF32 image into the CPU's physical address space.  Returns 0 on
 * success and -1 with `err` filled in. */
int elf32_load(Cpu* cpu, const void* image, size_t size, ImageInfo* out,
               std::string& err);

/* Load a raw binary blob at `addr`, checking that it is covered by RAM. */
int raw_load(Cpu* cpu, const void* image, size_t size, u32 addr,
             std::string& err);

}  // namespace brain

#endif  /* BRAINEMU_LOADER_LOADER_H */
