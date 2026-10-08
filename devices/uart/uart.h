/* The UART: a 16550-style serial port.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The peripheral a real firmware talks to, modelled after the 16550 so a guest's
 * existing driver works unchanged: eight byte-wide registers, a divisor latch,
 * a 16-byte receive FIFO, and one interrupt output.  The device itself is
 * `UartDevice`, declared in `uart_device.h` and implemented in `uart.cpp`; the
 * core only ever sees `Device`.
 */
#ifndef BRAINEMU_DEVICES_UART_H
#define BRAINEMU_DEVICES_UART_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_uart_module();

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_UART_H */
