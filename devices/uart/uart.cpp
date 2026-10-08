/* brain-emu -- the UART device: registers, FIFO, interrupt, and the module
 * wrapper for the registry.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Everything a 16550 driver expects, in eight byte-wide registers.  The device
 * is `UartDevice` (`uart_device.h`); this file is its implementation plus the
 * module wrapper, and the two together are the example of a peripheral that is
 * a register map plus a FIFO plus an interrupt line.
 */
#include "devices/uart/uart.h"

#include <cstdio>
#include <cstring>
#include <memory>

#include "brainemu/bus.h"
#include "brainemu/module.h"
#include "devices/uart/uart_device.h"

namespace brain {

UartDevice::UartDevice(u32 base) : base_(base) {
  info_.id = "uart";
  info_.name = "16550-style UART";
  info_.kind = ModuleKind::Device;
  info_.description = "8 byte-wide registers, 16-byte RX FIFO, one interrupt output";
}

void UartDevice::reset() {
  dll_ = dlm_ = ier_ = lcr_ = mcr_ = scr_ = 0;
  rx_head_ = rx_len_ = 0;
  drive_irq(0, 0);
}

u8 UartDevice::reg_read(u32 addr) {
  switch (addr & 7u) {
    case 0:
      if (dlab()) return dll_;
      if (rx_len_ == 0) return 0;
      {
        const u8 b = rx_fifo_[rx_head_];
        rx_head_ = (rx_head_ + 1) % kFifoDepth;
        --rx_len_;
        update_irq();               /* draining the FIFO can drop the line */
        return b;
      }
    case 1: return dlab() ? dlm_ : ier_;
    case 2:                        /* IIR: 0b100 = receive data, 0b001 = none */
      return ((ier_ & 1u) && rx_len_ > 0) ? 0x04 : 0x01;
    case 3: return lcr_;
    case 4: return mcr_;
    case 5:                        /* LSR: THR and transmitter always empty */
      return 0x60 | (rx_len_ > 0 ? 0x01 : 0x00);
    case 6: return 0;              /* MSR: no modem lines modelled */
    case 7: return scr_;
    default: return 0;
  }
}

void UartDevice::reg_write(u32 addr, u8 v) {
  switch (addr & 7u) {
    case 0:
      if (dlab()) dll_ = v;
      else transmit(v);
      break;
    case 1:
      if (dlab()) dlm_ = v;
      else {
        ier_ = v & 0x0Fu;
        update_irq();
      }
      break;
    case 2:                        /* FCR: bit 1 clears the RX FIFO */
      if (v & 0x02u) {
        rx_head_ = rx_len_ = 0;
        update_irq();
      }
      break;
    case 3: lcr_ = v; break;
    case 4: mcr_ = v; break;
    case 5: break;                  /* LSR is read-only */
    case 6: break;                  /* MSR is read-only */
    case 7: scr_ = v; break;
    default: break;
  }
}

u32 UartDevice::read(u32 offset, u32 size) {
  /* A multi-byte access assembles the adjacent byte registers, little-endian -
   * what a word read of a byte-wide register file looks like to a driver. */
  u32 v = 0;
  for (u32 i = 0; i < size && (offset + i) < 8u; ++i)
    v |= (u32)reg_read(offset + i) << (8 * i);
  return v;
}

void UartDevice::write(u32 offset, u32 value, u32 size) {
  for (u32 i = 0; i < size && (offset + i) < 8u; ++i)
    reg_write(offset + i, (u8)(value >> (8 * i)));
}

void UartDevice::tick(u64 cycles) {
  (void)cycles;
  if (!rx_fn_) return;
  /* Drain whatever the source has, up to the FIFO's depth.  A source that
   * would block says -1, and that ends the poll - the next tick tries again. */
  while (rx_len_ < kFifoDepth) {
    const int b = rx_fn_(rx_user_);
    if (b < 0) break;
    push_rx((u8)b);
  }
  update_irq();
}

void UartDevice::transmit(u8 b) {
  ++tx_bytes_;
  if (mcr_ & 0x10u) {
    /* Loopback: the byte comes straight back, which is how a driver tests the
     * port without a peer. */
    push_rx(b);
    update_irq();
    return;
  }
  if (tx_fn_) tx_fn_(tx_user_, b);
}

void UartDevice::push_rx(u8 b) {
  if (rx_len_ >= kFifoDepth) return;   /* a full FIFO drops the byte, like a real one */
  rx_fifo_[(rx_head_ + rx_len_) % kFifoDepth] = b;
  ++rx_len_;
  ++rx_bytes_;
}

void UartDevice::update_irq() {
  drive_irq(0, ((ier_ & 1u) && rx_len_ > 0) ? 1 : 0);
}

const char* UartDevice::field_name(u32 i) const {
  static const char* kNames[4] = {"tx_bytes", "rx_bytes", "rx_fifo", "irq"};
  return i < 4 ? kNames[i] : "";
}

const char* UartDevice::field_value(u32 i) const {
  static char buf[32];
  switch (i) {
    case 0: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)tx_bytes_); break;
    case 1: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)rx_bytes_); break;
    case 2: std::snprintf(buf, sizeof(buf), "%u", rx_len_); break;
    case 3: std::snprintf(buf, sizeof(buf), "%d", irq_level(0)); break;
    default: return "";
  }
  return buf;
}

namespace {

class UartModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new UartDevice(base));
  }
  /* The next free page after the console (0x10000000), the timer (0x10001000)
   * and the interrupt controller (0x10002000). */
  u32 default_base() const override { return 0x10004000u; }

 private:
  ModuleInfo info_{"uart", "16550-style UART", ModuleKind::Device,
                   "serial port: 8 registers, 16-byte RX FIFO, one interrupt"};
};

}  // namespace

void register_uart_module() {
  static const UartModule m;
  ModuleRegistry::instance().add_device(&m);
}

}  // namespace brain
