#include "brainemu/bus.h"

#include <cstdlib>
#include <cstring>
#include <vector>

#include <sys/mman.h>

#include "core/cpu.h"

namespace brain {

Bus::Bus() { memset(l1_, 0, sizeof(l1_)); }

Bus::~Bus() {
  for (PageDesc*& l1 : l1_) {
    if (l1) delete[] l1;
  }
  if (owns_ram_ && ram_host_) {
    munmap(ram_host_, ram_size_);
  }
}

void Bus::reset() {
  for (PageDesc*& l1 : l1_) {
    if (l1) {
      memset(l1, 0, sizeof(PageDesc) * kL2Size);
    }
  }
  for (u32 i = 0; i < nregions_; ++i) {
    BusRegion& r = regions_[i];
    if ((r.flags & REG_RAM) && owns_ram_ && r.host == ram_host_) continue;
    if (r.host && (r.flags & REG_RAM)) free(r.host);
  }
  nregions_ = 0;
  if (owns_ram_ && ram_host_) memset(ram_host_, 0, ram_size_);
  stat_reads = stat_writes = stat_device = 0;
}

static u8* alloc_backed(u32 size) {
  void* p = mmap(nullptr, size, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  return p == MAP_FAILED ? nullptr : (u8*)p;
}

bool Bus::map_ram(u32 base, u32 size, const char* name) {
  u8* p = alloc_backed(size);
  if (!p) return false;
  if (!map_host(base, size, p, REG_RAM, name)) {
    munmap(p, size);
    return false;
  }
  if (ram_host_ == p) owns_ram_ = true;
  return true;
}

bool Bus::span_taken(u32 base, u32 size, u32 against) const {
  for (u32 off = 0; off < size; off += 0x1000)
    if (l2_lookup_const((base + off) >> 12).flags & against) return true;
  return false;
}

bool Bus::map_host(u32 base, u32 size, u8* host, u32 flags, const char* name) {
  if (!size || nregions_ == kMaxRegions) return false;
  /* A RAM window may not cover a device: the core's identity fast path serves
   * RAM without consulting the per-page table, so a shadowed device would be
   * invisible to the guest. */
  if (span_taken(base, size, PG_DEVICE)) return false;
  BusRegion& r = regions_[nregions_++];
  r = BusRegion{base, size, flags, host, nullptr, nullptr, nullptr,
                name ? name : "ram"};
  for (u32 off = 0; off < size; off += 0x1000) {
    PageDesc& p = l2_lookup((base + off) >> 12);
    p.host = host + off;
    p.region = &r;      /* NOTE: invalidated by later push_back; refreshed
                         * below whenever a region vector grows. */
    p.flags = PG_PRESENT | ((flags & REG_RAM) ? u32(PG_RAM) : 0u) |
              ((flags & REG_ROM) ? u32(PG_ROM) : 0u);
    p.gbase = base + off;
  }
  if (flags & REG_RAM) {
    /* keep the "primary" RAM window for the identity fast path */
    if (ram_size_ == 0 || size > ram_size_) {
      ram_base_ = base;
      ram_size_ = size;
      ram_host_ = host;
      ram_mask_ = (size & (size - 1)) == 0 ? size - 1 : 0;
    }
  }
  return true;
}

bool Bus::map_device(u32 base, u32 size, BusReadFn r, BusWriteFn w,
                     void* user, const char* name) {
  if (nregions_ == kMaxRegions) return false;
  if (span_taken(base, size, PG_PRESENT)) return false;   /* no shadowing */
  BusRegion& nr = regions_[nregions_++];
  nr = BusRegion{base, size, REG_DEVICE, nullptr, user, r, w,
                 name ? name : "dev"};
  BusRegion* rp = &nr;
  for (u32 off = 0; off < size; off += 0x1000) {
    PageDesc& p = l2_lookup((base + off) >> 12);
    p.host = nullptr;
    p.region = rp;
    p.flags = PG_PRESENT | PG_DEVICE;
    p.gbase = base + off;
  }
  return true;
}

bool Bus::unmap(u32 base, u32 size) {
  for (u32 i = 0; i < nregions_; ++i) {
    if (regions_[i].base == base) {
      regions_[i] = regions_[--nregions_];
      break;
    }
  }
  for (u32 off = 0; off < size; off += 0x1000) {
    PageDesc& p = l2_lookup((base + off) >> 12);
    p.host = nullptr;
    p.region = nullptr;
    p.flags = 0;
    p.gbase = 0;
  }
  return true;
}

BusRegion* Bus::region_at(u32 addr) {
  for (u32 i = 0; i < nregions_; ++i) {
    BusRegion& r = regions_[i];
    if (addr - r.base < r.size) return &r;
  }
  return nullptr;
}

bool Bus::read_byte(u32 addr, u8* out) {
  PageDesc& p = l2_lookup(addr >> 12);
  if (p.flags & (PG_RAM | PG_ROM)) {
    *out = p.host[addr & 0xFFF];
    ++stat_reads;
    return true;
  }
  if (p.flags & PG_DEVICE) {
    ++stat_device;
    BusRegion* r = p.region;
    u32 v = r->read ? r->read(r->user, addr, 1) : 0;
    *out = (u8)v;
    return true;
  }
  return false;
}

bool Bus::write_byte(u32 addr, u8 v) {
  PageDesc& p = l2_lookup(addr >> 12);
  if (p.flags & PG_RAM) {
    p.host[addr & 0xFFF] = v;
    ++stat_writes;
    return true;
  }
  if (p.flags & PG_DEVICE) {
    ++stat_device;
    BusRegion* r = p.region;
    if (r->write) r->write(r->user, addr, v, 1);
    return true;
  }
  return false;   /* ROM or unmapped: caller raises the external abort     */
}

void Bus::note_store(u32 addr) {
  /* Cheap filter: only pages that currently host translated code need the
   * JIT's attention.  `cpu` is null for interpreter-only builds. */
  if (!cpu) return;
  if (page_has_code(addr)) cpu->invalidate_code(addr, 4);
}

void Bus::mark_code_page(u32 addr, bool code) {
  PageDesc& p = l2_lookup(addr >> 12);
  if (code) p.flags |= PG_CODE;
  else {
    /* clear only when the caller says no block remains for that page */
    p.flags &= ~PG_CODE;
  }
}

bool Bus::range_writable(u32 addr, size_t n) const {
  if (!n) return false;
  const u64 end = (u64)addr + n;
  if (end > 0x100000000ull) return false;      /* wraps the 32-bit space     */
  for (u32 vpn = addr >> 12, last = (u32)((end - 1) >> 12);; ++vpn) {
    if (!(l2_lookup_const(vpn).flags & PG_RAM)) return false;
    if (vpn == last) return true;
  }
}

bool Bus::range_readable(u32 addr, size_t n) const {
  if (!n) return false;
  const u64 end = (u64)addr + n;
  if (end > 0x100000000ull) return false;
  for (u32 vpn = addr >> 12, last = (u32)((end - 1) >> 12);; ++vpn) {
    if (!(l2_lookup_const(vpn).flags & (PG_RAM | PG_ROM))) return false;
    if (vpn == last) return true;
  }
}

bool Bus::write_bytes(u32 addr, const void* src, size_t n) {
  /* Validate first: a load that lands partly on RAM and partly on nothing must
   * not leave the first half behind. */
  if (!range_writable(addr, n)) return false;
  const u8* s = (const u8*)src;
  for (size_t i = 0; i < n; ++i) write_byte(addr + (u32)i, s[i]);
  return true;
}

bool Bus::read_bytes(u32 addr, void* dst, size_t n) const {
  if (!range_readable(addr, n)) return false;
  u8* d = (u8*)dst;
  for (size_t i = 0; i < n; ++i) (void)((Bus*)this)->read_byte(addr + (u32)i, &d[i]);
  return true;
}

/* ------------------------------------------------------------ console -- */

namespace {
/* One process-wide console: the ARM926EJ-S SoCs this core targets all have a
 * UART at a fixed address, and a test only ever needs "the bytes the guest
 * printed".  The buffer wraps; log() always returns the last kCap bytes. */
struct Console {
  static constexpr u32 kCap = 64u * 1024u;
  char data[kCap];
  u64 total = 0;          /* bytes ever written, monotonic                  */
};
Console g_console;

inline void console_put(char c) {
  g_console.data[g_console.total % Console::kCap] = c;
  ++g_console.total;
}
}  // namespace

u32 ConsoleDevice::Read(void* user, u32 addr, u32 size) {
  /* A UART's data register would be here; the log is drained from the host
   * side instead, so reads are simply "no data".  `user` is unused. */
  (void)user; (void)addr; (void)size;
  return 0;
}

void ConsoleDevice::Write(void* user, u32 addr, u32 val, u32 size) {
  (void)user;
  if (addr & 3u) {
    console_put((char)val);            /* byte lane write                    */
    return;
  }
  for (u32 i = 0; i < size; ++i) console_put((char)(val >> (i * 8)));
}

void* ConsoleDevice::user() { return &g_console; }

const char* ConsoleDevice::log() {
  /* The buffer is not NUL-terminated in guest-visible order, so terminate into
   * a static string: log_len() <= kCap and the copy is one per dump. */
  static char out[Console::kCap + 1];
  u32 n = (u32)(g_console.total < Console::kCap ? g_console.total
                                                : Console::kCap);
  u32 start = (u32)(g_console.total % Console::kCap);
  if (g_console.total >= Console::kCap) {
    for (u32 i = 0; i < n; ++i) out[i] = g_console.data[(start + i) % Console::kCap];
  } else {
    for (u32 i = 0; i < n; ++i) out[i] = g_console.data[i];
  }
  out[n] = 0;
  return out;
}

u32 ConsoleDevice::log_len() {
  return (u32)(g_console.total < Console::kCap ? g_console.total
                                               : Console::kCap);
}

void ConsoleDevice::log_clear() { g_console.total = 0; }

}  // namespace brain
