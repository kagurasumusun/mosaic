/* SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 * Copyright (c) 2026 the brain-emu authors.  See LICENSE. */
#include "brainemu/bus.h"

#include <cstdlib>
#include <cstring>
#include <vector>

#include <sys/mman.h>

namespace brain {

Bus::Bus() { memset(l1_, 0, sizeof(l1_)); }

Bus::~Bus() {
  for (PageDesc*& l1 : l1_) {
    if (l1) delete[] l1;
  }
  for (u32 i = 0; i < nregions_; ++i) release_region(regions_[i]);
  nregions_ = 0;
}

/* Only memory the bus allocated itself: a caller's buffer (map_host) belongs to
 * the caller, and a board that mapped its ROM in will map it again. */
void Bus::release_region(BusRegion& r) {
  if (r.owned && r.host) munmap(r.host, r.size);
  r.host = nullptr;
  r.owned = false;
}

void Bus::reset() {
  for (PageDesc*& l1 : l1_) {
    if (l1) {
      memset(l1, 0, sizeof(PageDesc) * kL2Size);
    }
  }
  for (u32 i = 0; i < nregions_; ++i) release_region(regions_[i]);
  nregions_ = 0;
  ram_host_ = nullptr;
  ram_base_ = ram_size_ = ram_mask_ = 0;
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
  if (!map_backing(base, size, p, REG_RAM, name, /*owned=*/true)) {
    munmap(p, size);
    return false;
  }
  return true;
}

bool Bus::span_taken(u32 base, u32 size, u32 against) const {
  for (u32 off = 0; off < size; off += 0x1000)
    if (l2_lookup_const((base + off) >> 12).flags & against) return true;
  return false;
}

bool Bus::map_host(u32 base, u32 size, u8* host, u32 flags, const char* name) {
  return map_backing(base, size, host, flags, name, /*owned=*/false);
}

bool Bus::map_backing(u32 base, u32 size, u8* host, u32 flags, const char* name,
                      bool owned) {
  if (!size || nregions_ == kMaxRegions) return false;
  /* A RAM window may not cover a device: the core's identity fast path serves
   * RAM without consulting the per-page table, so a shadowed device would be
   * invisible to the guest. */
  if (span_taken(base, size, PG_DEVICE)) return false;
  BusRegion& r = regions_[nregions_++];
  r = BusRegion{};
  r.base = base;
  r.size = size;
  r.flags = flags;
  r.host = host;
  r.name = name ? name : "ram";
  r.owned = owned;
  for (u32 off = 0; off < size; off += 0x1000) {
    PageDesc& p = l2_lookup((base + off) >> 12);
    p.host = host + off;
    /* regions_ is a fixed array, so a page's region pointer stays valid for
     * the life of the mapping. */
    p.region = &r;
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

u32 Bus::route_read(void* user, u32 addr, u32 size) {
  BusRegion* r = static_cast<BusRegion*>(user);
  for (u32 i = 0; i < r->nsubs; ++i) {
    const BusRegion::Sub& s = r->subs[i];
    if (addr - s.base < s.size) return s.read ? s.read(s.user, addr, size) : 0;
  }
  const BusRegion::Sub& owner = r->subs[0];
  return owner.read ? owner.read(owner.user, addr, size) : 0;
}

void Bus::route_write(void* user, u32 addr, u32 val, u32 size) {
  BusRegion* r = static_cast<BusRegion*>(user);
  for (u32 i = 0; i < r->nsubs; ++i) {
    const BusRegion::Sub& s = r->subs[i];
    if (addr - s.base < s.size) {
      if (s.write) s.write(s.user, addr, val, size);
      return;
    }
  }
  const BusRegion::Sub& owner = r->subs[0];
  if (owner.write) owner.write(owner.user, addr, val, size);
}

bool Bus::map_device(u32 base, u32 size, BusReadFn r, BusWriteFn w,
                     void* user, const char* name) {
  if (!size || (u64)base + size > 0x100000000ull) return false;
  /* The pages this window touches, from the one its first byte is in to the one
   * its last byte is in - a window that starts mid-page can still end in the
   * next one. */
  const u32 page0 = base & ~0xFFFu;
  const u32 npages = ((base + size - 1) >> 12) - (base >> 12) + 1;
  const u32 cover = npages << 12;
  /* Whose pages are these?  Free pages mean the window claims the covering
   * pages, one region, exactly as it always has.  Pages held by another *device*
   * can be shared, as long as the byte windows stay disjoint and both live
   * inside one page. */
  BusRegion* owner = nullptr;
  for (u32 off = 0; off < cover; off += 0x1000) {
    PageDesc& p = l2_lookup((page0 + off) >> 12);
    if (!(p.flags & PG_PRESENT)) continue;
    if (!(p.flags & PG_DEVICE)) return false;            /* RAM/ROM: refuse   */
    if (!owner) owner = p.region;
    else if (owner != p.region) return false;            /* two owners: rare  */
  }
  if (owner) {
    /* Sharing is a single-page affair on both sides: the page's owner must be a
     * one-page window too, or the router would have to cover several pages. */
    if (npages != 1 || owner->size > 0x1000) return false;
    if (owner->nsubs >= BusRegion::kMaxSubs) return false;
    const u32 win = size;
    if (owner->nsubs == 0) {
      /* First rival: the page's device becomes a router over its own window and
       * the new one.  Its exact window is what was asked for, not the page. */
      BusRegion::Sub& own = owner->subs[owner->nsubs++];
      own.base = owner->base;
      own.size = owner->win_size ? owner->win_size : owner->size;
      own.user = owner->user;
      own.read = owner->read;
      own.write = owner->write;
      owner->read = route_read;
      owner->write = route_write;
      owner->user = owner;
    }
    for (u32 i = 0; i < owner->nsubs; ++i) {
      const BusRegion::Sub& s = owner->subs[i];
      if (base < s.base + s.size && s.base < base + win) return false;  /* overlap */
    }
    BusRegion::Sub& sub = owner->subs[owner->nsubs++];
    sub.base = base;
    sub.size = win;
    sub.user = user;
    sub.read = r;
    sub.write = w;
    return true;                    /* pages already point at the router */
  }
  if (nregions_ == kMaxRegions) return false;
  BusRegion& nr = regions_[nregions_++];
  nr = BusRegion{};
  nr.base = base;
  nr.size = cover;
  nr.flags = REG_DEVICE;
  nr.user = user;
  nr.read = r;
  nr.write = w;
  nr.name = name ? name : "dev";
  nr.win_size = size;
  BusRegion* rp = &nr;
  for (u32 off = 0; off < cover; off += 0x1000) {
    PageDesc& p = l2_lookup((page0 + off) >> 12);
    p.host = nullptr;
    p.region = rp;
    p.flags = PG_PRESENT | PG_DEVICE;
    p.gbase = page0 + off;
  }
  return true;
}

bool Bus::unmap(u32 base, u32 size) {
  /* A page shared by several windows is one region, and it goes as one: any
   * window's base tears the page down and every device in it loses the page at
   * once.  (Finer grained detach is not a thing yet; a board that needs it can
   * reset the bus.)  A page with a single device is the ordinary case below. */
  for (u32 i = 0; i < nregions_; ++i) {
    BusRegion& r = regions_[i];
    if (!r.nsubs) continue;
    bool mine = (r.base == base);
    for (u32 k = 0; !mine && k < r.nsubs; ++k) mine = (r.subs[k].base == base);
    if (!mine) continue;
    PageDesc& p = l2_lookup(r.base >> 12);
    p.host = nullptr;
    p.region = nullptr;
    p.flags = 0;
    p.gbase = 0;
    release_region(r);
    regions_[i] = regions_[--nregions_];
    return true;
  }
  for (u32 i = 0; i < nregions_; ++i) {
    if (regions_[i].base == base) {
      /* Release the backing store only if the bus allocated it: a caller's
       * buffer (map_host) is the caller's to free.  The identity RAM window is
       * dropped with the region, so a CPU's cached pointer into it must not be
       * used afterwards. */
      release_region(regions_[i]);
      regions_[i] = regions_[--nregions_];
      if (ram_base_ == base) {
        ram_host_ = nullptr;
        ram_base_ = ram_size_ = ram_mask_ = 0;
      }
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

void Bus::note_store(u32 addr, u32 size) {
  /* Cheap filter: only pages that currently host translated code need the
   * observer's attention, and an interpreter-only machine has no observer to
   * tell.  Both checks are a load and a branch. */
  if (!store_fn_ || !size) return;
  const u64 end = (u64)addr + size;
  for (u64 p = addr & ~(u64)0xFFF; p < end; p += 0x1000) {
    /* One report for the whole range: the observer invalidates by overlap, so a
     * superset costs one range check and is always safe. */
    if (page_has_code((u32)p)) { store_fn_(store_user_, addr, size); return; }
  }
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
  /* A bulk write is the loaders' and a debugger's way into guest memory, and it
   * lands in the host buffer behind the page - the CPU's own store path never
   * sees it, so the translation guard is told here instead. */
  const u64 len = n > 0xFFFFFFFFull ? 0xFFFFFFFFull : (u64)n;
  note_store(addr, (u32)len);
  return true;
}

bool Bus::read_bytes(u32 addr, void* dst, size_t n) const {
  if (!range_readable(addr, n)) return false;
  u8* d = (u8*)dst;
  for (size_t i = 0; i < n; ++i) (void)((Bus*)this)->read_byte(addr + (u32)i, &d[i]);
  return true;
}

}  // namespace brain
