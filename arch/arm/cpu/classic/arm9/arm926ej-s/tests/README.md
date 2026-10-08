# ARM926EJ-S tests

Instruction, MMU, JIT, and integration tests for the ARM926EJ-S core. These
suites are also the conformance pin for the ARMv5TEJ ISA layer
(`arch/arm/isa/armv5/`): the instruction corpus, the instruction-level unit
tests, the MMU tests and the x86-64 JIT tests all exercise the ISA through
the core that embeds it. Test data and reference-model generators live here
too.
