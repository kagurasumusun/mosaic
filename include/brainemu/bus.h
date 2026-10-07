/* brain-emu: memory bus / address space.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The bus is byte-addressable and endianness-agnostic: the ARM926EJ-S core
 * performs BE8 data swapping itself, so the bus only ever sees byte
 * addresses and returns host-order bytes.
 *
 * Regions are published at 4 KiB granularity through a two-level page table
 * so both engines (interpreter and JIT) share one fast lookup.
 */
#ifndef BRAINEMU_BUS_H
#define BRAINEMU_BUS_H

#include <cstddef>
#include <cstdint>
#include <cstring>

#include "brainemu/types.h"

namespace brain {

class Cpu;

/* Device callbacks: `addr` is the absolute guest address, size is 1/2/4. */
using BusReadFn  = u32 (*)(void* user, u32 addr, u32 size);
using BusWriteFn = void  (*)(void* user, u32 addr, u32 val, u32 size);

enum RegionFlag : u32 {
  REG_RAM     = 1u << 0,   /* writable, directly mapped on the host          */
  REG_ROM     = 1u << 1,   /* read-only, directly mapped                     */
  REG_DEVICE  = 1u << 2,   /* callbacks, uncached, no JIT fast path          */
};

struct BusRegion {
  u32 base = 0;
  u32 size = 0;
  u32 flags = 0;
  u8* host = nullptr;      /* RAM/ROM backing store (null for devices)       */
  void* user = nullptr;
  BusReadFn read = nullptr;
  BusWriteFn write = nullptr;
  const char* name = "";
};

enum PageFlag : u32 {
  PG_PRESENT = 1u << 0,
  PG_RAM     = 1u << 1,
  PG_ROM     = 1u << 2,
  PG_DEVICE  = 1u << 3,
  PG_CODE    = 1u << 4,    /* translated JIT blocks live in this page        */
};

/* One 4 KiB page mapping.  Kept trivial (no member initialisers) so the table
 * can be bulk-zeroed with memset and memcpy'd around by the MMU code. */
struct PageDesc {
  u8* host;                /* host base of this page (null for devices)      */
  BusRegion* region;
  u32 flags;
  u32 gbase;               /* guest address of `host` (== page start)        */

  void clear() { host = nullptr; region = nullptr; flags = 0; gbase = 0; }
};
static_assert(sizeof(PageDesc) == 24, "page table entry layout");

class Bus {
 public:
  Bus();
  ~Bus();

  void reset();

  /* Map a RAM window; backing memory is allocated (and zeroed) by the bus. */
  bool map_ram(u32 base, u32 size, const char* name);
  /* Adopt an existing host buffer as RAM/ROM. */
  bool map_host(u32 base, u32 size, u8* host, u32 flags, const char* name);
  /* Map a device (MMIO) region. */
  bool map_device(u32 base, u32 size, BusReadFn r, BusWriteFn w, void* user,
                  const char* name);
  bool unmap(u32 base, u32 size);

  /* Non-zero when any page of [base, base+size) has `against` set.  Mapping
   * over an existing region is refused rather than silently shadowed: the
   * identity fast path for RAM and a device that must be seen by every access
   * cannot both win, and a guest that reads back a device it never wrote is an
   * extremely hard bug to spot from inside. */
  bool span_taken(u32 base, u32 size, u32 against) const;

  PageDesc* page(u32 addr) { return &l2_lookup(addr >> 12); }
  const PageDesc* page_c(u32 addr) const { return &l2_lookup_const(addr >> 12); }
  bool is_ram(u32 addr) const {
    return (l2_lookup_const(addr >> 12).flags & (PG_RAM | PG_ROM)) != 0;
  }
  bool is_writable(u32 addr) const {
    return (l2_lookup_const(addr >> 12).flags & PG_RAM) != 0;
  }

  /* Whole-range checks, used to make a bulk transfer all-or-nothing: an image
   * that does not fit must leave memory untouched rather than half-loaded.
   * `range_writable` accepts RAM only, `range_readable` RAM and ROM. */
  bool range_writable(u32 addr, size_t n) const;
  bool range_readable(u32 addr, size_t n) const;

  /* Byte level access; false when the address is unmapped. */
  bool read_byte(u32 addr, u8* out);
  bool write_byte(u32 addr, u8 v);

  /* Host pointer for a RAM/ROM guest address, else null. */
  u8* host_ptr(u32 addr) {
    PageDesc& p = l2_lookup(addr >> 12);
    return (p.flags & (PG_RAM | PG_ROM)) ? p.host + (addr & 0xFFFu) : nullptr;
  }

  /* Notified by the core after every store so the JIT can drop stale code. */
  void note_store(u32 addr);
  void mark_code_page(u32 addr, bool code);
  bool page_has_code(u32 addr) const {
    return (l2_lookup_const(addr >> 12).flags & PG_CODE) != 0;
  }

  bool write_bytes(u32 addr, const void* src, size_t n);
  bool read_bytes(u32 addr, void* dst, size_t n) const;

  u32 ram_base() const { return ram_base_; }
  u32 ram_size() const { return ram_size_; }
  u8* ram_host() const { return ram_host_; }
  u32 ram_mask() const { return ram_mask_; }

  BusRegion* region_at(u32 addr);
  u32 region_count() const { return nregions_; }
  BusRegion& region(u32 i) { return regions_[i]; }

  u64 stat_reads = 0, stat_writes = 0, stat_device = 0;
  Cpu* cpu = nullptr;   /* owner used for code invalidation (may be null) */

 private:
  static constexpr u32 kL1Bits = 10;   /* 1024 L1 entries                   */
  static constexpr u32 kL2Bits = 10;   /* 1024 pages/table = 4 MiB          */
  static constexpr u32 kL2Size = 1u << kL2Bits;

  PageDesc& l2_lookup(u32 vpn) {
    PageDesc** l1 = &l1_[vpn >> kL2Bits];
    if (__builtin_expect(*l1 == nullptr, 0)) *l1 = new PageDesc[kL2Size]();
    return (*l1)[vpn & (kL2Size - 1)];
  }
  const PageDesc& l2_lookup_const(u32 vpn) const {
    PageDesc* l1 = l1_[vpn >> kL1Bits];
    static const PageDesc kEmpty{nullptr, nullptr, 0, 0};
    return l1 ? l1[vpn & (kL2Size - 1)] : kEmpty;
  }

  PageDesc* l1_[1 << kL1Bits];
  /* A fixed array, not a vector: PageDesc keeps raw pointers to its region and
   * those must survive further map_* calls.  An SoC with more regions than this
   * needs the vector variant; 64 is generous for every ARM926EJ-S board design
   * (RAM, flash, a handful of MMIO blocks). */
  static constexpr u32 kMaxRegions = 64;
  BusRegion regions_[kMaxRegions];
  u32 nregions_ = 0;
  u8* ram_host_ = nullptr;
  bool owns_ram_ = false;
  u32 ram_base_ = 0, ram_size_ = 0, ram_mask_ = 0;
};

/* Test-bench console device: every byte written to the region is appended to a
 * host-visible log.  Handy for bare-metal regression images that have no UART
 * driver of their own -- the test just prints ConsoleDevice::log() at the end.
 * Map it with
 *   bus.map_device(0x10000000, 4, ConsoleDevice::Read, ConsoleDevice::Write,
 *                  ConsoleDevice::user(), "console");
 */
struct ConsoleDevice {
  static u32 Read(void* user, u32 addr, u32 size);
  static void Write(void* user, u32 addr, u32 val, u32 size);

  static void* user();            /* the `user` argument to map_device        */
  static const char* log();       /* NUL-terminated log (oldest bytes first)  */
  static u32 log_len();
  static void log_clear();
};

}  // namespace brain

#endif  /* BRAINEMU_BUS_H */
