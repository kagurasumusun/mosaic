/* brain-emu -- ELF32 / raw image loading.
 * SPDX-file-note: see LICENSE.
 */
#include "loader/loader.h"

#include <cstring>

#include "core/cpu.h"

namespace brain {

/* The subset of the ELF32 layout this loader reads.  Kept here rather than
 * pulling in <elf.h> so the file builds the same on every host. */
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

/* Decode the fixed-size header tables from a little-endian byte stream.  Only
 * called after the size and identification checks below. */
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

}  // namespace

int raw_load(Cpu* cpu, const void* image, size_t size, u32 addr,
             std::string& err) {
  if (!image || !size) {
    err = "empty image";
    return -1;
  }
  if (!cpu->bus->range_writable(addr, size)) {
    err = "image does not fit in writable memory";
    return -1;
  }
  if (!cpu->load_image(addr, image, size)) {
    err = "image does not fit in mapped memory";
    return -1;
  }
  return 0;
}

int elf32_load(Cpu* cpu, const void* image, size_t size, ImageInfo* out,
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

  ImageInfo info;
  info.is_elf = true;
  info.entry = h.entry;
  info.machine = h.machine;
  info.type = h.type;
  info.min = 0xFFFFFFFFu;
  info.max = 0;

  for (u32 i = 0; i < h.phnum; ++i) {
    Phdr p;
    parse_phdr(b + h.phoff + (size_t)i * h.phentsize, &p);
    if (p.type != PT_LOAD_) continue;
    if (p.memsz == 0) continue;
    if ((size_t)p.offset + p.filesz > size) {
      err = "PT_LOAD segment extends past the file";
      return -1;
    }
    if (p.memsz < p.filesz) {
      err = "PT_LOAD segment has memsz < filesz";
      return -1;
    }
    /* Everything is placed at the physical address: the core boots with the
     * MMU off, so a virtual-only address would not be reachable.  For the
     * static armel binaries this loader is meant for they are equal anyway. */
    if (!cpu->bus->range_writable(p.paddr, p.memsz)) {
      err = "PT_LOAD segment at 0x" + hex(p.paddr) +
            " is not inside writable memory";
      return -1;
    }
    if (!cpu->load_image(p.paddr, b + p.offset, p.filesz)) {
      err = "PT_LOAD segment at 0x" + hex(p.paddr) + " could not be written";
      return -1;
    }
    for (u32 o = p.filesz; o < p.memsz; ++o) cpu->phys_write(p.paddr + o, 0, 1);

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

}  // namespace brain
