/* brain-emu -- the image loaders, over a bare bus.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A loader is a module: it takes a host buffer and fills *memory*, and it does
 * not need a CPU to do it.  Every check here therefore runs on a Bus with no
 * machine and no processor behind it, which is the point - the ARM loader tests
 * live in test_unit.cpp (through the C API), and these are about the module
 * boundary and about the refusals.
 *
 * The ELF images are built by hand in the test rather than by a toolchain: a
 * loader's contract is about headers, so the headers are what the test writes.
 */
#include "brainemu/core/bus.h"
#include "brainemu/device/device.h"
#include "brainemu/loader/loader.h"

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

using namespace brain;

namespace {

int g_checks = 0, g_failed = 0;

void check(bool ok, const char* what) {
  ++g_checks;
  if (!ok) {
    ++g_failed;
    std::printf("  FAILED: %s\n", what);
  }
}

#define CHECK(cond) check((cond), #cond)

/* ------------------------------------------------------------- fixtures -- */

struct ElfBuilder {
  std::vector<std::pair<u32, std::vector<u8>>> segs;
  u32 entry = 0;
  u16 type = 2;          /* ET_EXEC */
  u16 machine = 40;      /* EM_ARM  */
  u8 cls = 1, data = 1;
  u16 phentsize = 32;
  u32 phoff_override = 0;
  u32 memsz_delta = 0;       /* p_memsz = p_filesz + this                  */
  u32 vaddr = 0;             /* p_vaddr, when it should differ from paddr  */

  void add(u32 addr, std::vector<u8> bytes) { segs.emplace_back(addr, bytes); }

  std::vector<u8> build() const {
    const u32 ehsize = 52;
    const u32 phnum = (u32)segs.size();
    u32 off = ehsize + (u32)phentsize * phnum;
    std::vector<std::pair<u32, u32>> where;   /* file offset of each blob   */
    for (const auto& s : segs) {
      where.emplace_back(off, (u32)s.second.size());
      off += (u32)s.second.size();
    }
    std::vector<u8> b(off, 0);
    auto put16 = [&](size_t at, u32 v) { b[at] = (u8)v; b[at + 1] = (u8)(v >> 8); };
    auto put32 = [&](size_t at, u32 v) {
      for (int i = 0; i < 4; ++i) b[at + i] = (u8)(v >> (8 * i));
    };
    b[0] = 0x7f; b[1] = 'E'; b[2] = 'L'; b[3] = 'F';
    b[4] = cls; b[5] = data; b[6] = 1;
    put16(16, type);
    put16(18, machine);
    put32(20, 1);
    put32(24, entry);
    put32(28, phoff_override ? phoff_override : ehsize);
    put16(40, ehsize);
    put16(42, phentsize);
    put16(44, phnum);
    for (u32 i = 0; i < phnum; ++i) {
      const size_t ph = ehsize + (size_t)i * phentsize;
      const u32 filesz = where[i].second;
      put32(ph + 0, 1);                       /* PT_LOAD */
      put32(ph + 4, where[i].first);          /* p_offset */
      put32(ph + 8, segs[i].first + vaddr);   /* p_vaddr  */
      put32(ph + 12, segs[i].first);          /* p_paddr  */
      put32(ph + 16, filesz);
      put32(ph + 20, filesz + memsz_delta);
      put32(ph + 24, 7);                      /* p_flags: rwx */
      put32(ph + 28, 0x1000);                 /* p_align */
      std::memcpy(&b[where[i].first], segs[i].second.data(), filesz);
    }
    return b;
  }
};

u32 peek32(Bus& bus, u32 addr) {
  u32 v = 0;
  for (u32 i = 0; i < 4; ++i) {
    u8 b = 0;
    bus.read_byte(addr + i, &b);
    v |= (u32)b << (8 * i);
  }
  return v;
}

/* --------------------------------------------------------------- sections -- */

void test_raw() {
  std::printf("[raw loader]\n");
  Bus bus;
  CHECK(bus.map_ram(0x8000, 1u << 16, "ram"));

  const u8 img[] = {0x11, 0x22, 0x33, 0x44, 0x55};
  std::string err;

  CHECK(raw_load(bus, img, sizeof img, 0x8000, err) == 0);
  CHECK(err.empty());
  u8 b = 0;
  CHECK(bus.read_byte(0x8000, &b) && b == 0x11);
  CHECK(bus.read_byte(0x8004, &b) && b == 0x55);

  /* The loader writes *physical* memory: a raw image is what a boot ROM has
   * already placed, so it cannot be translated. */
  CHECK(raw_load(bus, img, sizeof img, 0x9000, err) == 0);   /* MMU off      */
  err.clear();

  /* Refusals: nothing mapped, half outside, empty, nothing at all. */
  CHECK(raw_load(bus, img, sizeof img, 0x4000, err) != 0);
  CHECK(!err.empty());
  err.clear();
  CHECK(raw_load(bus, img, sizeof img, 0x17FFE, err) != 0);  /* straddles */
  CHECK(!err.empty());
  err.clear();
  CHECK(raw_load(bus, img, 0, 0x8000, err) != 0);
  CHECK(!err.empty());
  err.clear();
  CHECK(raw_load(bus, nullptr, sizeof img, 0x8000, err) != 0);

  /* A refused load leaves memory alone: the failing case above started at
   * 0x17FFE, which is inside RAM, and nothing may have been written there. */
  CHECK(bus.read_byte(0x17FFE, &b) && b == 0);

  /* ROM is readable but not writable, so a raw image cannot land in it. */
  static u8 rom_page[0x1000];
  BufferDevice rom("rom", "rom", 0x20000, 0x1000, rom_page, /*writable=*/false);
  CHECK(rom.attach(bus));
  CHECK(raw_load(bus, img, sizeof img, 0x20000, err) != 0);
  CHECK(!err.empty());
}

/* The loader interface is enough for a test to add a format of its own, which
 * is the point of loaders being modules. */
void test_custom_loader() {
  std::printf("[a loader module of one's own]\n");
  Bus bus;
  CHECK(bus.map_ram(0, 0x1000, "ram"));
  struct HexLoader final : Loader {
    const ModuleInfo& info() const override {
      static const ModuleInfo k = {"hex", "hex digits", ModuleKind::Loader,
                                   "two characters per byte"};
      return k;
    }
    bool probe(const void* d, u64 n) const override {
      const char* p = (const char*)d;
      return n >= 2 && p[0] == 'x' && p[1] == 'x';
    }
    int load(Bus& bus, const void* d, u64 n, ImageInfo* out,
             std::string& err) const override {
      const char* p = (const char*)d;
      for (u64 i = 2; i + 1 < n; i += 2) {
        auto nib = [](char c) -> int {
          if (c >= '0' && c <= '9') return c - '0';
          if (c >= 'a' && c <= 'f') return c - 'a' + 10;
          return -1;
        };
        const int hi = nib(p[i]), lo = nib(p[i + 1]);
        if (hi < 0 || lo < 0) { err = "not a hex digit"; return -1; }
        if (!bus.write_byte(0 + (u32)((i - 2) / 2), (u8)((hi << 4) | lo))) {
          err = "image does not fit";
          return -1;
        }
      }
      if (out) {
        out->format = "hex";
        out->segments = 1;
        out->max = (u32)((n - 2) / 2);
      }
      return 0;
    }
  };
  static const HexLoader loader;
  ModuleRegistry::instance().add_loader(&loader);
  CHECK(ModuleRegistry::instance().loader_by_id("hex") == &loader);
  CHECK(ModuleRegistry::instance().loader_by_probe("xx4142", 6) == &loader);
  ImageInfo info;
  std::string err;
  CHECK(loader.load(bus, "xx4142", 6, &info, err) == 0);
  CHECK(std::strcmp(info.format, "hex") == 0);
  u8 b = 0;
  CHECK(bus.read_byte(0, &b) && b == 0x41);
  CHECK(bus.read_byte(1, &b) && b == 0x42);
  CHECK(loader.load(bus, "xxzz", 4, nullptr, err) != 0);
  CHECK(!err.empty());
}

void test_elf_ok() {
  std::printf("[elf32 loader]\n");
  Bus bus;
  CHECK(bus.map_ram(0x8000, 0x10000, "ram"));

  ElfBuilder eb;
  eb.entry = 0x8000;
  eb.add(0x8000, {0x01, 0x00, 0xA0, 0xE3});   /* mov r0, #1 */
  eb.memsz_delta = 16;                        /* a bss tail to zero-fill */
  const std::vector<u8> img = eb.build();

  CHECK(loader_elf32().probe(img.data(), img.size()));
  ImageInfo info;
  std::string err;
  CHECK(elf32_load(bus, img.data(), img.size(), &info, err) == 0);
  CHECK(err.empty());
  CHECK(info.is_elf);
  CHECK(std::strcmp(info.format, "elf32") == 0);
  CHECK(info.entry == 0x8000);
  CHECK(info.machine == 40);
  CHECK(info.segments == 1);
  CHECK(info.min == 0x8000);
  CHECK(info.max == 0x8000 + 4 + 16);
  CHECK(peek32(bus, 0x8000) == 0xE3A00001u);
  /* memsz > filesz: the tail must be zero even though RAM here was not. */
  CHECK(peek32(bus, 0x8004) == 0);

  /* Two segments, far apart: the reported range covers both. */
  ElfBuilder two;
  two.entry = 0x9000 | 1u;                    /* an interworking entry */
  two.add(0x8000, {0xAA});
  two.add(0x10000, {0xBB, 0xCC});
  const std::vector<u8> img2 = two.build();
  CHECK(elf32_load(bus, img2.data(), img2.size(), &info, err) == 0);
  CHECK(info.entry == (0x9000u | 1u));
  CHECK(info.segments == 2);
  CHECK(info.min == 0x8000);
  CHECK(info.max == 0x10002);
  u8 b = 0;
  CHECK(bus.read_byte(0x8000, &b) && b == 0xAA);
  CHECK(bus.read_byte(0x10001, &b) && b == 0xCC);

  /* The public `Loader` object loads through the same code. */
  CHECK(loader_elf32().load(bus, img.data(), img.size(), nullptr, err) == 0);

  /* p_vaddr need not equal p_paddr, and this loader places the physical one:
   * the core comes out of reset with the MMU off. */
  ElfBuilder v;
  v.vaddr = 0x40000000u;                      /* a virtual address */
  v.add(0x8000, {0x5A});
  const std::vector<u8> img3 = v.build();
  CHECK(elf32_load(bus, img3.data(), img3.size(), &info, err) == 0);
  CHECK(bus.read_byte(0x8000, &b) && b == 0x5A);
  CHECK(bus.read_byte(0x40000000, &b) == false);
}

void test_elf_refusals() {
  std::printf("[elf32 refusals]\n");
  Bus bus;
  CHECK(bus.map_ram(0x8000, 0x10000, "ram"));
  ElfBuilder eb;
  eb.entry = 0x8000;
  eb.add(0x8000, {0xDE, 0xAD, 0xBE, 0xEF});
  const std::vector<u8> good = eb.build();
  std::string err;
  ImageInfo info;

  auto refuses = [&](const std::vector<u8>& img, const char* what) {
    err.clear();
    CHECK(elf32_load(bus, img.data(), img.size(), &info, err) != 0);
    CHECK(!err.empty());
    if (err.empty()) std::printf("    (%s produced no message)\n", what);
  };

  /* Not an ELF, or not one this core can be. */
  refuses(std::vector<u8>(64, 0), "all zeros");
  refuses(std::vector<u8>{}, "empty");
  std::vector<u8> img = good;
  img[4] = 2;  refuses(img, "ELFCLASS64");
  img = good;  img[5] = 2;  refuses(img, "big-endian");
  img = good;  img[18] = 3; refuses(img, "not EM_ARM");
  img = good;  img[16] = 3; refuses(img, "ET_DYN (PIE)");
  img = good;  img[16] = 1; refuses(img, "ET_REL");
  img = good;  img[16] = 4; refuses(img, "ET_CORE");

  /* Truncation and header damage. */
  refuses(std::vector<u8>(good.begin(), good.begin() + 40), "short header");
  img = good;  img[42] = 8;   refuses(img, "phentsize too small");
  img = good;  img[44] = 0;   refuses(img, "no program headers");
  img = good;  img[44] = 0xFF; refuses(img, "phnum past the file");
  img = good;
  img[52 + 4] = 0xFF; img[52 + 5] = 0xFF;    /* p_offset far past EOF */
  refuses(img, "segment past EOF");

  /* A segment with memsz < filesz is malformed, and one that lands outside
   * writable memory is refused rather than clipped. */
  ElfBuilder bad_mem;
  bad_mem.add(0x8000, {0x11, 0x22, 0x33, 0x44});
  bad_mem.memsz_delta = 0xFFFFFFFFu;          /* memsz wraps below filesz */
  refuses(bad_mem.build(), "memsz < filesz");

  ElfBuilder far;
  far.add(0xF0000000u, {0x11});
  far.entry = 0xF0000000u;
  refuses(far.build(), "segment outside RAM");

  ElfBuilder straddle;
  straddle.add(0x17FFFu, {0x11, 0x22, 0x33, 0x44});
  refuses(straddle.build(), "segment straddles the RAM end");

  /* A refused image left memory untouched - the two-pass check is the reason a
   * later segment cannot strand an earlier one in memory. */
  u8 b = 0;
  CHECK(bus.read_byte(0x8000, &b) && b == 0);
  CHECK(bus.read_byte(0x17FFF, &b) && b == 0);
  CHECK(bus.read_byte(0x8004, &b) && b == 0);
}

void test_elf_zero_fill_boundary() {
  std::printf("[elf32 bss and range]\n");
  Bus bus;
  CHECK(bus.map_ram(0, 0x1000, "ram"));
  /* One page of RAM and an image that exactly fills it: no rounding surprise. */
  ElfBuilder eb;
  eb.add(0, {0x11, 0x22});
  eb.memsz_delta = 0x0FFE;
  const std::vector<u8> img = eb.build();
  ImageInfo info;
  std::string err;
  CHECK(elf32_load(bus, img.data(), img.size(), &info, err) == 0);
  CHECK(info.max == 0x1000);
  u8 b = 0;
  CHECK(bus.read_byte(0, &b) && b == 0x11);
  CHECK(bus.read_byte(0xFFF, &b) && b == 0);

  /* One byte more does not fit. */
  ElfBuilder over;
  over.add(0, {0x11, 0x22});
  over.memsz_delta = 0x0FFF;
  const std::vector<u8> over_img = over.build();
  CHECK(elf32_load(bus, over_img.data(), over_img.size(), &info, err) != 0);
}

}  // namespace

int main() {
  std::printf("brain-emu loader tests\n");
  test_raw();
  test_custom_loader();
  test_elf_ok();
  test_elf_refusals();
  test_elf_zero_fill_boundary();
  if (g_failed) {
    std::printf("FAIL: %d/%d checks passed\n", g_checks - g_failed, g_checks);
    return 1;
  }
  std::printf("PASS: %d/%d checks passed\n", g_checks, g_checks);
  return 0;
}
