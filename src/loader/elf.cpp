/* brain-emu -- the ELF32 and raw image loaders.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Both write *physical* memory through the bus (the core comes out of reset
 * with the MMU off, so a virtual-only address would be a lie), both refuse
 * rather than half-load, and neither touches a CPU: an image is placed in the
 * address space, and pointing the processor at it is the machine's job
 * (`Machine::load_elf32`).  That is what makes a loader a module.
 *
 * The ELF subset is what a bootable, statically linked armel binary needs:
 * ELFCLASS32, little-endian, ET_EXEC, PT_LOAD segments with memsz >= filesz.
 * Everything else is refused by name, because a loader that guesses is worse
 * than one that says "link with -no-pie".
 */
#include "brainemu/loader/loader.h"

#include <cstring>

#include "brainemu/core/bus.h"

namespace brain {
namespace {

constexpr size_t kEhSize = 52;
constexpr size_t kPhSize = 32;

enum : u32 {
  ELFCLASS32_ = 1,
  ELFDATA2LSB_ = 1,
  ET_EXEC_ = 2,
  ET_DYN_ = 3,
  EM_ARM_ = 40,
  PT_LOAD_ = 1,
};

struct Ehdr {
  u8 ident[16];
  u16 type, machine;
  u32 version, entry, phoff, shoff, flags;
  u16 ehsize, phentsize, phnum, shentsize, shnum, shstrndx;
};

struct Phdr {
  u32 type, offset, vaddr, paddr, filesz, memsz, flags, align;
};

u32 get_le32(const u8* p) {
  return (u32)p[0] | ((u32)p[1] << 8) | ((u32)p[2] << 16) | ((u32)p[3] << 24);
}
u16 get_le16(const u8* p) { return (u16)((u32)p[0] | ((u32)p[1] << 8)); }

std::string hex(u32 v) {
  static const char kDig[] = "0123456789abcdef";
  char buf[11];
  buf[0] = '0';
  for (int i = 0; i < 8; ++i) buf[1 + i] = kDig[(v >> (28 - i * 4)) & 0xF];
  buf[9] = 0;
  return std::string(buf);
}

void parse_ehdr(const u8* b, Ehdr* h) {
  std::memcpy(h->ident, b, 16);
  h->type = get_le16(b + 16);
  h->machine = get_le16(b + 18);
  h->version = get_le32(b + 20);
  h->entry = get_le32(b + 24);
  h->phoff = get_le32(b + 28);
  h->shoff = get_le32(b + 32);
  h->flags = get_le32(b + 36);
  h->ehsize = get_le16(b + 40);
  h->phentsize = get_le16(b + 42);
  h->phnum = get_le16(b + 44);
  h->shentsize = get_le16(b + 46);
  h->shnum = get_le16(b + 48);
  h->shstrndx = get_le16(b + 50);
}

void parse_phdr(const u8* b, Phdr* p) {
  p->type = get_le32(b);
  p->offset = get_le32(b + 4);
  p->vaddr = get_le32(b + 8);
  p->paddr = get_le32(b + 12);
  p->filesz = get_le32(b + 16);
  p->memsz = get_le32(b + 20);
  p->flags = get_le32(b + 24);
  p->align = get_le32(b + 28);
}

/* Does the bus have writable RAM for every byte of [addr, addr+n)?  A loader
 * has to know *before* it writes anything, or a refused image leaves a
 * half-loaded machine behind. */
bool writable_range(Bus& bus, u32 addr, size_t n) { return bus.range_writable(addr, n); }

/* --------------------------------------------------------------- modules -- */

class Elf32Loader final : public Loader {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo kMod = {"elf32", "ELF32 little-endian (ARM)",
                                    ModuleKind::Loader,
                                    "ET_EXEC PT_LOAD segments at their physical "
                                    "addresses; no PIE, no REL, no dynamic"};
    return kMod;
  }
  bool probe(const void* data, u64 size) const override {
    const u8* b = static_cast<const u8*>(data);
    return size >= kEhSize && b[0] == 0x7f && b[1] == 'E' && b[2] == 'L' &&
           b[3] == 'F' && b[4] == ELFCLASS32_ && b[5] == ELFDATA2LSB_;
  }
  int load(Bus& bus, const void* data, u64 size, ImageInfo* out,
           std::string& err) const override {
    return elf32_load(bus, data, (size_t)size, out, err);
  }
};

class RawLoader final : public Loader {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo kMod = {"raw", "flat binary", ModuleKind::Loader,
                                    "an image with no header, placed at the "
                                    "address the caller gives"};
    return kMod;
  }
  /* A flat image has nothing to recognise: it is what a boot ROM has when the
   * design decided load address and image together, so it is never
   * auto-detected - the caller asks for it (Machine::load_raw). */
  bool probe(const void*, u64) const override { return false; }
  int load(Bus& bus, const void* data, u64 size, ImageInfo* out,
           std::string& err) const override {
    /* Without an address a flat image cannot be placed; the machine passes one
     * through load_raw, and this path exists so the interface is total. */
    ImageInfo info;
    info.format = "raw";
    info.entry = 0;
    info.segments = 1;
    info.max = (u32)size;
    if (size && !writable_range(bus, 0, (size_t)size)) {
      err = "a raw image needs a load address (see --addr)";
      return -1;
    }
    if (size && !bus.write_bytes(0, data, (size_t)size)) {
      err = "raw image does not fit in writable memory";
      return -1;
    }
    if (out) *out = info;
    return 0;
  }
};

}  // namespace

const Loader& loader_elf32() {
  static const Elf32Loader kLoader;
  return kLoader;
}

const Loader& loader_raw() {
  static const RawLoader kLoader;
  return kLoader;
}

/* ------------------------------------------------------ implementations -- */

int raw_load(Bus& bus, const void* image, size_t size, u32 addr,
             std::string& err) {
  if (!image || !size) {
    err = "empty image";
    return -1;
  }
  if (!writable_range(bus, addr, size)) {
    err = "image does not fit in writable memory";
    return -1;
  }
  if (!bus.write_bytes(addr, image, size)) {
    err = "image does not fit in mapped memory";
    return -1;
  }
  return 0;
}

int elf32_load(Bus& bus, const void* image, size_t size, ImageInfo* out,
               std::string& err) {
  const u8* b = static_cast<const u8*>(image);
  if (size < kEhSize) {
    err = "buffer smaller than the ELF header";
    return -1;
  }
  if (b[0] != 0x7f || b[1] != 'E' || b[2] != 'L' || b[3] != 'F') {
    err = "not an ELF file";
    return -1;
  }
  Ehdr h;
  parse_ehdr(b, &h);
  if (h.ident[4] != ELFCLASS32_) {
    err = "not ELFCLASS32: this core is 32-bit only";
    return -1;
  }
  if (h.ident[5] != ELFDATA2LSB_) {
    err = "not little-endian: use --be32 for a byte-swapped raw image";
    return -1;
  }
  if (h.machine != EM_ARM_) {
    err = "e_machine is not EM_ARM";
    return -1;
  }
  if (h.type == ET_DYN_) {
    /* A PIE/shared object needs a relocation pass and an address the loader
     * picks; both are out of scope, so say so instead of half-loading it. */
    err = "ET_DYN (PIE) is not supported: link with -no-pie";
    return -1;
  }
  if (h.type != ET_EXEC_) {
    err = "only ET_EXEC images can be loaded";
    return -1;
  }
  if (h.phentsize < kPhSize || !h.phnum) {
    err = "no program headers";
    return -1;
  }
  if ((size_t)h.phoff + (size_t)h.phnum * h.phentsize > size) {
    err = "program headers run past the end of the file";
    return -1;
  }

  /* Two passes: check every segment before writing any.  A loader that fails
   * halfway leaves the machine in a state no caller asked for. */
  for (u32 i = 0; i < h.phnum; ++i) {
    Phdr p;
    parse_phdr(b + h.phoff + (size_t)i * h.phentsize, &p);
    if (p.type != PT_LOAD_ || p.memsz == 0) continue;
    if ((size_t)p.offset + p.filesz > size) {
      err = "PT_LOAD segment extends past the file";
      return -1;
    }
    if (p.memsz < p.filesz) {
      err = "PT_LOAD segment has memsz < filesz";
      return -1;
    }
    if (!writable_range(bus, p.paddr, p.memsz)) {
      err = "PT_LOAD segment at 0x" + hex(p.paddr) +
            " is not inside writable memory";
      return -1;
    }
  }

  ImageInfo info;
  info.is_elf = true;
  info.format = "elf32";
  info.entry = h.entry;
  info.machine = h.machine;
  info.type = h.type;
  info.min = 0xFFFFFFFFu;
  info.max = 0;

  for (u32 i = 0; i < h.phnum; ++i) {
    Phdr p;
    parse_phdr(b + h.phoff + (size_t)i * h.phentsize, &p);
    if (p.type != PT_LOAD_ || p.memsz == 0) continue;
    /* Everything is placed at the physical address: the core boots with the
     * MMU off, so a virtual-only address would not be reachable.  For the
     * static armel binaries this loader is meant for they are equal anyway. */
    if (p.filesz && !bus.write_bytes(p.paddr, b + p.offset, p.filesz)) {
      err = "PT_LOAD segment at 0x" + hex(p.paddr) + " could not be written";
      return -1;
    }
    /* Zero-fill the rest of the segment's memory.  The range is checked first
     * so a segment that does not fit leaves nothing behind, and the fill is
     * reported to the bus the same way a bulk write is: the loader may be
     * running over bytes some translated block was built from. */
    const u32 bss = p.memsz - p.filesz;
    if (bss && !bus.range_writable(p.paddr + p.filesz, bss)) {
      err = "bss of the segment at 0x" + hex(p.paddr) + " is not writable";
      return -1;
    }
    for (u32 o = 0; o < bss; ++o) bus.write_byte(p.paddr + p.filesz + o, 0);
    if (bss) bus.note_store(p.paddr + p.filesz, bss);
    info.segments++;
    if (p.paddr < info.min) info.min = p.paddr;
    if (p.paddr + p.memsz > info.max) info.max = p.paddr + p.memsz;
  }
  if (!info.segments) {
    err = "no PT_LOAD segment in the image";
    return -1;
  }
  if (out) *out = info;
  return 0;
}

namespace {

/* Registered by `register_builtin_modules()` through the loader registry, so
 * `Machine::load_image` can find them by content or by id. */
struct LoaderRegistrar {
  LoaderRegistrar() {
    ModuleRegistry::instance().add_loader(&loader_elf32());
    ModuleRegistry::instance().add_loader(&loader_raw());
  }
};

}  // namespace

void register_builtin_loaders() {
  static LoaderRegistrar once;
  (void)once;
}

}  // namespace brain
