/* The interrupt controller: several inputs, one output.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The reference device with interrupt *inputs*: a mask, a status/pending pair
 * and one output that is the OR of the unmasked lines, so several sources can
 * share one CPU line - the shape every real SoC has.
 */
#ifndef BRAINEMU_DEVICES_INTC_H
#define BRAINEMU_DEVICES_INTC_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_intc_module();

}  // namespace brain

#endif  /* BRAINEMU_DEVICES_INTC_H */
