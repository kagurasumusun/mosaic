/* The exit port: a write stops the machine.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A bare-metal test image has to say "I am done" somewhere, and the value it
 * leaves behind is the result.  A write-only word, and a device module like any
 * other - which is the point: even the thing that stops the emulator goes
 * through the same window, the same bus and the same registry as a UART.
 */
#ifndef BRAINEMU_DEVICES_EXIT_H
#define BRAINEMU_DEVICES_EXIT_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_exit_module();

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_EXIT_H */
