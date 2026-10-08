/* brain-emu -- the UART's test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Driven the way a guest drives it: through the bus, byte by byte, against the
 * 16550 register map.  The host's half (where the bytes go and come from) is two
 * hooks the test installs, so the whole port - registers, FIFO, interrupt -
 * is exercised without a machine, and the module wrapper is checked against the
 * registry the build fills.
 */
#include <cstring>
#include <string>

#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/module.h"
#include "devices/uart/uart_device.h"
#include "testkit/test_support.h"

namespace {

constexpr u32 kBase = 0x10004000u;

std::string g_tx;
void tx_hook(void*, u8 b) { g_tx.push_back((char)b); }

struct RxSrc {
  std::string data;
  size_t pos = 0;
};
int rx_hook(void* user) {
  RxSrc* s = static_cast<RxSrc*>(user);
  if (s->pos >= s->data.size()) return -1;
  return (u8)s->data[s->pos++];
}

/* The line's resting level: `drive_irq` notifies on change only, so a level
 * that never moved is never reported. */
int g_irq_level = 0;
int g_irq_changes = 0;
void irq_hook(void*, brain::Device& d, u32 output, int level) {
  (void)d; (void)output;
  g_irq_level = level;
  ++g_irq_changes;
}

u8 rd(brain::Bus& bus, u32 addr) {
  u8 v = 0;
  bus.read_byte(addr, &v);
  return v;
}

}  // namespace

int main() {
  using brain::Bus;
  using brain::UartDevice;

  brainemutest::ensure_builtins();

  /* The module wrapper: the registry knows the uart, and its default page is
   * the next free one after the console, the timer and the interrupt
   * controller. */
  {
    const brain::DeviceModule* m = brain::ModuleRegistry::instance().device_by_id("uart");
    CHECK(m != nullptr);
    if (m) {
      CHECK(std::strcmp(m->info().id, "uart") == 0);
      CHECK(m->default_base() == 0x10004000u);
      auto d = m->create(0x20000000u);
      CHECK(d != nullptr);
      CHECK(d->base() == 0x20000000u);
      CHECK(d->size() == 8u);
    }
  }

  Bus bus;
  UartDevice uart(kBase);
  CHECK(uart.attach(bus));
  uart.set_tx_hook(tx_hook, nullptr);
  uart.set_irq_notify(irq_hook, nullptr);

  /* Transmit: a byte written to THR (offset 0) goes to the hook, and the
   * transmitter is always empty (LSR bits 5 and 6). */
  g_tx.clear();
  CHECK(bus.write_byte(kBase + 0, 'h'));
  CHECK(bus.write_byte(kBase + 0, 'i'));
  CHECK(g_tx == "hi");
  CHECK(uart.tx_bytes() == 2);
  CHECK((rd(bus, kBase + 5) & 0x60) == 0x60);

  /* Receive: the source is polled by `tick`, the FIFO delivers in order, LSR
   * bit 0 says data is ready, and reading RBR drains it. */
  RxSrc src{"ok!"};
  uart.set_rx_hook(rx_hook, &src);
  uart.tick(1);
  CHECK(uart.rx_fifo_level() == 3);
  CHECK((rd(bus, kBase + 5) & 0x01) == 0x01);
  CHECK(rd(bus, kBase + 0) == 'o');
  CHECK(rd(bus, kBase + 0) == 'k');
  CHECK(rd(bus, kBase + 0) == '!');
  CHECK(uart.rx_fifo_level() == 0);
  CHECK((rd(bus, kBase + 5) & 0x01) == 0x00);   /* drained: DR clear */

  /* The interrupt: a level, high while the FIFO holds a byte *and* IER bit 0
   * is set - and low again once the FIFO is drained or the enable is off. */
  src.data = "x";
  src.pos = 0;
  uart.tick(1);
  CHECK(uart.rx_fifo_level() == 1);
  CHECK(g_irq_level == 0);                      /* IER is still 0: no interrupt */
  CHECK(bus.write_byte(kBase + 1, 0x01));       /* IER: receive-data enable */
  CHECK(g_irq_level == 1);
  CHECK(rd(bus, kBase + 0) == 'x');             /* drain the FIFO */
  CHECK(g_irq_level == 0);
  src.data = "y";
  src.pos = 0;
  uart.tick(1);
  CHECK(g_irq_level == 1);
  CHECK(bus.write_byte(kBase + 2, 0x02));       /* FCR bit 1: clear the RX FIFO */
  CHECK(uart.rx_fifo_level() == 0);
  CHECK(g_irq_level == 0);
  CHECK(g_irq_changes >= 4);

  /* The divisor latch: LCR bit 7 gates DLL/DLM, and the data registers are
   * untouched by it. */
  CHECK(bus.write_byte(kBase + 3, 0x80));       /* LCR: DLAB on */
  CHECK(bus.write_byte(kBase + 0, 0x0C));       /* DLL */
  CHECK(bus.write_byte(kBase + 1, 0x00));       /* DLM */
  CHECK(rd(bus, kBase + 0) == 0x0C);
  CHECK(rd(bus, kBase + 1) == 0x00);
  CHECK(bus.write_byte(kBase + 3, 0x03));       /* LCR: 8N1, DLAB off */
  g_tx.clear();
  CHECK(bus.write_byte(kBase + 0, 'z'));        /* THR again, not DLL */
  CHECK(g_tx == "z");
  CHECK(rd(bus, kBase + 0) == 0);               /* RBR empty: reads as zero */

  /* Loopback: MCR bit 4 routes THR into the receive FIFO, which is how a
   * driver tests the port with no peer. */
  CHECK(bus.write_byte(kBase + 4, 0x10));
  g_tx.clear();
  CHECK(bus.write_byte(kBase + 0, 'L'));
  CHECK(g_tx.empty());                          /* loopback: nothing goes out */
  CHECK(uart.rx_fifo_level() == 1);
  CHECK(rd(bus, kBase + 0) == 'L');
  CHECK(bus.write_byte(kBase + 4, 0x00));       /* loopback off */

  /* Multi-byte access: a word read assembles the adjacent byte registers
   * (MCR, LSR, MSR, SCR) little-endian, the way a word read of a byte-wide
   * register file looks to a driver - and a word write reaches them all.
   * (Bus::read_bytes covers RAM/ROM only, so this goes through the device.) */
  CHECK(bus.write_byte(kBase + 7, 0x5A));       /* SCR */
  const u32 word = uart.read(4, 4);
  CHECK((word & 0xFFu) == 0x00);                /* MCR (loopback off) */
  CHECK(((word >> 8) & 0xFFu) == 0x60);         /* LSR: THR + transmitter empty */
  CHECK(((word >> 16) & 0xFFu) == 0x00);        /* MSR */
  CHECK(((word >> 24) & 0xFFu) == 0x5A);        /* SCR */
  g_tx.clear();
  uart.write(0, 'A' | (0x01 << 8), 2);          /* THR, then IER */
  CHECK(g_tx == "A");
  CHECK(rd(bus, kBase + 1) == 0x01);            /* the second byte hit IER */

  /* Fields, for a tool printing the device. */
  CHECK(uart.field_count() == 4);
  CHECK(std::strcmp(uart.field_name(0), "tx_bytes") == 0);
  CHECK(std::strcmp(uart.field_name(3), "irq") == 0);
  CHECK(std::strcmp(uart.field_value(2), "0") == 0);

  /* Reset: the registers and the FIFO go back to their power-on state. */
  CHECK(bus.write_byte(kBase + 1, 0x01));       /* IER on */
  src.data = "q";
  src.pos = 0;
  uart.tick(1);
  CHECK(uart.rx_fifo_level() == 1);
  uart.reset();
  CHECK(uart.rx_fifo_level() == 0);
  CHECK(rd(bus, kBase + 1) == 0x00);            /* IER cleared */
  CHECK(rd(bus, kBase + 3) == 0x00);            /* LCR cleared (DLAB off) */
  CHECK(g_irq_level == 0);

  return brainemutest::summary("devices/uart");
}
