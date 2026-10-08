/* brain-emu -- the UART device.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A 16550-style serial port: eight byte-wide registers at the base, a divisor
 * latch behind LCR bit 7, a 16-byte receive FIFO and one interrupt output.
 * A guest's existing 16550 driver runs against this unchanged - it is the same
 * register map, including the parts a driver pokes and never reads back (the
 * divisor latch, the scratch register).
 *
 * The device knows nothing about the host.  Where the bytes go and where they
 * come from are two function pointers the host installs (`set_tx_hook`,
 * `set_rx_hook`): a UART wired to stdio, to a socket, to a test's buffer - the
 * device is the same, and a device used outside a machine simply has no hooks
 * and talks to itself (transmit counts the byte, receive finds nothing).
 *
 * Register map (offsets from the base, all byte-wide):
 *
 *   0  RBR (read,  DLAB=0)  THR (write, DLAB=0)  DLL (DLAB=1)
 *   1  IER (DLAB=0)                              DLM (DLAB=1)
 *   2  IIR (read)             FCR (write)
 *   3  LCR   bit 7 = DLAB
 *   4  MCR   bit 4 = loopback
 *   5  LSR   bit 0 = data ready, bit 5 = THR empty, bit 6 = transmitter empty
 *   6  MSR
 *   7  SCR   scratch
 *
 * Interrupts: the single output "irq" is a level, high while the receive FIFO
 * holds a byte and IER bit 0 (receive-data enable) is set - the honest model of
 * a level-sensitive line, so a guest that reads late still sees the interrupt.
 */
#ifndef BRAINEMU_DEVICES_UART_DEVICE_H
#define BRAINEMU_DEVICES_UART_DEVICE_H

#include <cstdint>

#include "brainemu/device.h"
#include "brainemu/types.h"

namespace brain {

class UartDevice final : public Device {
 public:
  /* Where a transmitted byte goes. */
  using TxFn = void (*)(void* user, u8 b);
  /* Where a received byte comes from: the byte, or -1 when nothing is
   * waiting.  Polled from `tick`, so a source that would block must say -1
   * rather than wait. */
  using RxFn = int (*)(void* user);

  explicit UartDevice(u32 base = 0x10004000u);

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  /* Eight byte-wide registers; the bus maps the page they sit in. */
  u32 size() const override { return 8; }

  void reset() override;
  u32 read(u32 offset, u32 size) override;
  void write(u32 offset, u32 value, u32 size) override;

  /* The clock: the receive FIFO is filled by polling the source, which is what
   * makes a byte "arrive" without the guest doing anything - the machine calls
   * this after each run slice, and a UART wired to stdin gets its input the
   * same way a UART wired to a test buffer does. */
  void tick(u64 cycles) override;

  /* The host's half: where the bytes go and come from. */
  void set_tx_hook(TxFn fn, void* user) { tx_fn_ = fn; tx_user_ = user; }
  void set_rx_hook(RxFn fn, void* user) { rx_fn_ = fn; rx_user_ = user; }

  /* One interrupt output, and a name a board wires by. */
  u32 irq_count() const override { return 1; }
  const char* irq_name(u32 i) const override { return i == 0 ? "irq" : ""; }

  /* Self-description: how much the port moved, and the state of the line. */
  u32 field_count() const override { return 4; }
  const char* field_name(u32 i) const override;
  const char* field_value(u32 i) const override;

  /* What a test reads back (the hooks are write-only by design). */
  u64 tx_bytes() const { return tx_bytes_; }
  u64 rx_bytes() const { return rx_bytes_; }
  u32 rx_fifo_level() const { return rx_len_; }

  static constexpr u32 kFifoDepth = 16;

 private:
  u8 reg_read(u32 addr);
  void reg_write(u32 addr, u8 v);
  void transmit(u8 b);
  void push_rx(u8 b);
  void update_irq();

  ModuleInfo info_{};
  u32 base_ = 0;

  /* Register state. */
  u8 dll_ = 0, dlm_ = 0;      /* the divisor latch (DLAB=1) */
  u8 ier_ = 0;               /* interrupt enable */
  u8 lcr_ = 0;               /* line control (bit 7 = DLAB) */
  u8 mcr_ = 0;               /* modem control (bit 4 = loopback) */
  u8 scr_ = 0;               /* scratch */
  bool dlab() const { return (lcr_ & 0x80u) != 0; }

  /* The receive FIFO, as a ring. */
  u8 rx_fifo_[kFifoDepth] = {};
  u32 rx_head_ = 0;          /* next byte to read */
  u32 rx_len_ = 0;

  TxFn tx_fn_ = nullptr;
  void* tx_user_ = nullptr;
  RxFn rx_fn_ = nullptr;
  void* rx_user_ = nullptr;

  u64 tx_bytes_ = 0;
  u64 rx_bytes_ = 0;
};

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_UART_DEVICE_H */
