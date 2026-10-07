/* brain-emu -- the device base class and the adapters.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Everything here is plumbing between a device's own idea of an address (an
 * offset inside its window) and the bus's (a guest physical address plus a
 * size).  The interesting devices live where they belong - next to the board
 * or the machine that has them - and only need to implement read/write.
 */
#include "brainemu/device/device.h"

#include <cstring>

#include "brainemu/core/bus.h"

namespace brain {

/* The bus calls a device with the *absolute* address; a device thinks in
 * offsets.  The two thunks below are the whole translation, and the user
 * pointer is the Device itself. */
namespace {

u32 device_read(void* user, u32 addr, u32 size) {
  Device* d = static_cast<Device*>(user);
  return d->read(addr - d->base(), size);
}

void device_write(void* user, u32 addr, u32 value, u32 size) {
  Device* d = static_cast<Device*>(user);
  d->write(addr - d->base(), value, size);
}

}  // namespace

bool Device::attach(Bus& bus) {
  if (attached_) return true;
  if (!size()) return false;
  /* The window, not the covering page: the bus claims the page when it is free
   * (so a stray access next to a four-byte register is not an abort) and shares
   * it when another device is already there. */
  if (!bus.map_device(base(), size(), device_read, device_write,
                      this, info().name ? info().name : info().id))
    return false;
  attached_ = true;
  reset();
  return true;
}

/* ------------------------------------------------------------- adapters -- */

FuncDevice::FuncDevice(const char* id, const char* name, u32 base, u32 size,
                       ReadFn rd, WriteFn wr, void* user)
    : base_(base), size_(size), rd_(rd), wr_(wr), user_(user) {
  info_.id = id ? id : "func";
  info_.name = name ? name : "function-backed device";
  info_.kind = ModuleKind::Device;
  info_.description = "a device whose behaviour is a pair of host callbacks";
  /* The window is kept exactly as the caller gave it.  A window smaller than the
   * page still gets the whole page when the page is free (the bus rounds the
   * claim, so a stray access next to a register is not an abort), but an exact
   * window is what lets a second device share the page - see BusRegion. */
  if (!size_) size_ = 4;
}

u32 FuncDevice::read(u32 offset, u32 size) {
  if (!rd_) return 0;
  return rd_(user_, base_ + offset, size);
}

void FuncDevice::write(u32 offset, u32 value, u32 size) {
  if (wr_) wr_(user_, base_ + offset, value, size);
}

BufferDevice::BufferDevice(const char* id, const char* name, u32 base, u32 size,
                           u8* host, bool writable)
    : base_(base), size_(size), host_(host), writable_(writable) {
  info_.id = id ? id : "buffer";
  info_.name = name ? name : "host buffer";
  info_.kind = ModuleKind::Device;
  info_.description = "a caller-owned buffer mapped read-only or writable";
}

u32 BufferDevice::read(u32 offset, u32 size) {
  if (!host_ || offset + size > size_) return 0;
  u32 v = 0;
  for (u32 i = 0; i < size; ++i) v |= (u32)host_[offset + i] << (8 * i);
  return v;
}

void BufferDevice::write(u32 offset, u32 value, u32 size) {
  if (!writable_ || !host_ || offset + size > size_) return;
  for (u32 i = 0; i < size; ++i) host_[offset + i] = (u8)(value >> (8 * i));
}

/* The buffer is mapped as RAM or ROM, which the bus serves straight out of the
 * host pointer, so these two are only reached if somebody calls the device
 * directly (a test, or a future bus that stops special-casing direct maps). */
bool BufferDevice::attach(Bus& bus) {
  if (attached()) return true;
  if (!host_ || (size_ & 0xFFFu) || !size_) return false;
  const u32 flags = writable_ ? REG_RAM : REG_ROM;
  if (!bus.map_host(base_, size_, host_, flags, info().name)) return false;
  mark_attached();
  return true;
}

/* ---------------------------------------------------------------- console -- */

ConsoleDevice::ConsoleDevice(u32 base) : base_(base) {
  info_.id = "console";
  info_.name = "test console";
  info_.kind = ModuleKind::Device;
  info_.description = "bytes written to the window, collected as text";
  log_[0] = 0;
}

u32 ConsoleDevice::read(u32 offset, u32 size) {
  (void)offset; (void)size;
  /* A UART's data register would be here.  The log is drained from the host
   * side (`log()`), so a guest read finds nothing waiting. */
  return 0;
}

void ConsoleDevice::write(u32 offset, u32 value, u32 size) {
  (void)offset;
  for (u32 i = 0; i < size; ++i) {
    if (len_ + 1 >= kLogSize) {
      /* Keep the tail: a guest that prints a lot should still show the end. */
      std::memmove(log_, log_ + kLogSize / 2, kLogSize / 2);
      len_ = kLogSize / 2;
    }
    log_[len_++] = (char)(value >> (8 * i));
  }
  log_[len_] = 0;
}

}  // namespace brain
