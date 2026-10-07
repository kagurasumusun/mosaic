/* The console: bytes written there become text.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A device with a window, a log and no interrupts - the one a bare-metal image
 * with no UART driver prints to.  The device class is `ConsoleDevice`, declared
 * in `console_device.h` and implemented in `console.cpp` next to this file: it is
 * *this* device, and nothing else in the tree needs to name it, because the core
 * only ever sees `Device`.
 */
#ifndef BRAINEMU_DEVICES_CONSOLE_H
#define BRAINEMU_DEVICES_CONSOLE_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_console_module();

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_CONSOLE_H */
