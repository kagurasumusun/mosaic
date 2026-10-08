# ARM926EJ-S core implementation (`core/`))

The core-specific half of the ARM926EJ-S: CP15 (system control coprocessor)
and the MMU/TLB, the configuration caches derived from them, the identity
RAM fast path, the run loop and the JIT's out-of-line helpers, and `env` —
the `IsaEnv` table the ISA engine calls through.

The instruction set itself is not here: decoding, execution semantics, the
PSR/flag model, register banking and the architectural half of the exception
model live in `arch/arm/isa/armv5/`, and the pipeline step (ARM/Thumb
dispatch) in `arch/arm/execution/aarch32/`. `Cpu` inherits the architectural
state (`IsaState`) and forwards the engine entry points into those layers.
This is guest CPU logic, not machine/SoC wiring.
