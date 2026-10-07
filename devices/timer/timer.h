/* The timer: a down-counter with an interrupt output.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The reference device for the parts of the core a board needs - a window, a
 * clock (`tick`), self-description (`field_*`) and an interrupt output a board
 * wires wherever it likes.  It models no particular chip.
 */
#ifndef BRAINEMU_DEVICES_TIMER_H
#define BRAINEMU_DEVICES_TIMER_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_timer_module();

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_TIMER_H */
