# UART device

A 16550-style serial port: the peripheral a real firmware talks to, modelled
after the 16550 so a guest's existing driver works unchanged — eight byte-wide
registers, a divisor latch behind LCR bit 7, a 16-byte receive FIFO, and one
interrupt output.

Register map (offsets from the base, all byte-wide):

| offset | read | write |
| --- | --- | --- |
| 0 | RBR (DLAB=0) / DLL (DLAB=1) | THR (DLAB=0) / DLL (DLAB=1) |
| 1 | IER (DLAB=0) / DLM (DLAB=1) | IER / DLM |
| 2 | IIR | FCR (bit 1 clears the RX FIFO) |
| 3 | LCR (bit 7 = DLAB) | LCR |
| 4 | MCR (bit 4 = loopback) | MCR |
| 5 | LSR (bit 0 = data ready, bits 5/6 = transmitter empty, always set) | — |
| 6 | MSR (reads 0: no modem lines modelled) | — |
| 7 | SCR (scratch) | SCR |

The device knows nothing about the host: where the bytes go and where they come
from are two function pointers the host installs (`set_tx_hook`,
`set_rx_hook` — `devices/uart/uart_device.h`). A UART wired to stdio, to a
socket, or to a test's buffer is the same device; the integration test
(`tests/integration/test_host_stack.cpp`) shows the wiring. The receive FIFO is
filled by polling the source from `tick`, which the machine calls after each
run slice, so a byte "arrives" without the guest doing anything.

The interrupt is a level, high while the receive FIFO holds a byte and IER bit
0 is set — the honest model of a level-sensitive line, so a guest that reads
late still sees the interrupt.

Default base: `0x10004000` — the next free page after the console
(`0x10000000`), the timer (`0x10001000`) and the interrupt controller
(`0x10002000`).
