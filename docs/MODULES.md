# Modules: the core, and everything that plugs into it

This emulator is **one core plus modules**. The core owns the address space, the
run loop, the machine's lifetime and the C API. A module owns one piece of the
emulated world and registers itself under an id. The ARM926EJ-S — the reason this
project exists — is the *first* CPU module, not the shape of the emulator; a
second processor is a new directory, not a new emulator.

```
include/brainemu/core/bus.h        Bus: regions, per-page descriptors, byte access
include/brainemu/core/module.h     ModuleInfo, CpuModule, ModuleRegistry
include/brainemu/core/machine.h    MachineConfig, Machine (bus + CPU + devices)
include/brainemu/cpu/cpu.h         CpuCore: the whole CPU contract
include/brainemu/device/device.h   Device + FuncDevice/BufferDevice/ConsoleDevice
include/brainemu/loader/loader.h   Loader (probe + load) and the image description
src/core/bus.cpp                   the address space
src/core/device.cpp                the device base class and its adapters
src/core/module.cpp                the registry
src/core/machine.cpp               build, run, reset, load
src/api.cpp                        the C API over the Machine
src/loader/elf.cpp                 the two built-in loader modules
src/modules/builtin.cpp            the list of modules this build contains
src/modules/arm926/                the primary CPU: ARMv5TEJ, CP15 + MMU, JIT
src/modules/toy8/                  a second CPU, in 300 lines, on purpose
```

## The kinds of module

| kind | what it is | who calls it | examples in this tree |
| --- | --- | --- | --- |
| `cpu` | a processor: registers, an instruction set, its own timing | the machine, at slice boundaries | `arm926`, `toy8` |
| `loader` | an image format | `Machine::load_image`, by content | `elf32`, `raw` |
| `device` | something mapped into the address space | the bus, per access | exit port, console, UART |
| `monitor` | an observer of a running machine | the run loop | *(none yet: a tracer or a gdb stub)* |

Only a CPU module is *driven*; the others are called. A CPU module is therefore
the only one with an execution contract, and it is the one this document is
mostly about.

## The CPU contract

`CpuCore` (`include/brainemu/cpu/cpu.h`) is deliberately small. A module says
what it is (`info()`), binds to the address space (`init`), executes when asked
(`run_insns`, `run_cycles`, `step`), can be interrupted and reset, and reports
registers and counters. Everything else — opcodes, exceptions, an MMU, a CP15, a
JIT — belongs to the module and the core never learns about it.

Three rules shape the interface:

1. **Every call is a slice boundary.** `run_insns(n)` means "retire at most `n`
   instructions", not "tell me about each one". Nothing in the core calls into a
   module per instruction, so the virtual calls are noise and a module's inner
   loop stays a plain, non-virtual, monomorphic function over its own state.
   (This is also why there is no `execute_one_instruction()` hook: an ISA with
   variable-length or parallel instructions is exactly the case a
   per-instruction interface punishes. `step()` exists for debuggers, and calls
   `run_insns(1)` by another name.)
2. **The module owns its own view of state.** The core never reads a CPSR. It
   asks `info()` for the register *indices* a caller would use
   (`pc_index`, `status_index`, `saved_status_index`, `sp_index`), and passes
   those through `get_reg`/`set_reg`. A CPU with no flags register says
   `CPU_NO_REG` and the core simply has nothing to show.
3. **Options are named, not enumerated.** `CpuOptions` carries only what every
   machine has (JIT on/off, trace, cycle model, endianness, vector placement). A
   knob that only makes sense for one instruction set is set by name through
   `set_option(key, value)` — the CLI's `--cpu-opt`, the C API's
   `brain_set_option` and `brain_config_t::cpu_options` — and an unknown key is
   *refused* (`false`) rather than ignored, so a typo is a failure.

What the core guarantees a module:

* the `Bus` outlives the CPU, and is the only way to memory — so a module cannot
  be surprised by a device or a board that appeared behind its back;
* one thread per machine; no locks anywhere;
* `init` is called once per machine build, `reset` whenever the machine resets,
  and the module is destroyed before the bus (see `Machine::~Machine`);
* the run loop stops calling `run_*` when `request_stop` was honoured: a module
  that returns from `run_insns` early is normal, and the core will simply ask
  again (which is why `request_stop` must be sticky until `reset`).

What a module must guarantee the core:

* `run_insns(n)`/`run_cycles(n)` return after at most `n` (0 = until stopped),
  and never loop forever on their own;
* `insns()` and `cycles()` are the machine's clock: the device tick and every
  counter the C API reports come from them;
* a fault the module cannot complete is the module's own business to model. The
  core has no exception layer, because an exception *is* an instruction-set
  concept: `pending_exception()` is an opaque id the module chooses.

## Adding a CPU: five steps

The worked example is `src/modules/toy8/`, which is deliberately a
five-register accumulator machine with no MMU and no JIT: it proves the core
does not need an ARM to run.

1. **Write the core class.** A struct or class deriving from `CpuCore`, with its
   own state and its own interpreter. Keep the state in one object and let the
   fast paths be inline member functions — nothing calls them but your own code.
2. **Implement the interface.** The compiler is the checklist: every pure
   virtual is a decision you cannot skip. In practice it is ~120 lines.
3. **Describe it.** Fill in a `CpuInfo` (id, name, ISA, register count and the
   indices of the PC/flags/stack pointer, and whether bit 0 of an entry address
   selects a second instruction set) and a `ModuleInfo`.
4. **Provide a factory.** A `CpuModule` whose `create()` returns a fresh core.
5. **Register it.** One `register_<id>_module()` and one line in
   `src/modules/builtin.cpp`. The Makefile globs `src/modules/*/*.cpp`, so a new
   directory needs no build edit at all.

Then:

```
./build/brainemu --list-cpus                 # it appears, with its description
./build/brainemu --cpu mycpu prog.bin        # ...and a machine can be built on it
```

Two details that are easy to get wrong, and both are visible in `toy8`:

* **The bus is 32-bit guest-physical, and nothing more.** A module with an MMU
  translates before it touches the bus (the ARM one does: `cp15_mmu.cpp` owns
  vaddr→paddr); a module without one passes addresses through. A module whose
  addresses are narrower than 32 bits (toy8's 16-bit space) masks at its own
  boundary. The bus never sees a virtual address, and it never sees an
  instruction-set concept.
* **`set_entry(addr, alternate_state, stack)` is where a loader's work becomes
  the module's state.** A raw load only moves the PC; an ELF load additionally
  wants a stack, and an interworking entry wants bit 0. Only the module knows
  what a stack pointer *is*, so only the module can be asked to seed it — the
  ARM one puts it in the live bank and the User/Supervisor banks a boot would
  use, and toy8's is an 8-bit register.

### A module's own knobs

```cpp
bool MyCore::set_option(const char* key, const char* value) {
  if (!std::strcmp(key, "cache_bits")) { cfg_.cache_bits = (u32)atoi(value); return true; }
  return false;                    /* unknown keys are refused, not ignored */
}
```

```
build/brainemu --cpu mycpu --cpu-opt cache_bits=13 prog.bin
```

The ARM module's keys are `jazelle_trap`, `strict_unpredictable`,
`serror_on_busfault`, `tlb_bits`, `max_block_insns`, `jit_cache_mb`,
`jit_fast_mem` and `big_endian_data` (`src/modules/arm926/arm926.cpp`).

### Adding a loader

Derive from `Loader`, implement `probe` (does this buffer look like mine?) and
`load` (place it through the bus, report `ImageInfo`), and add it in
`loader.h`'s spirit — `tests/test_loader.cpp` contains a complete 20-line
example that registers itself inside the test. Two rules the built-in ones
follow and a new one should too: **write physical memory** (the CPU comes out of
reset with its MMU off) and **refuse before you write** (`Bus::range_writable`
per segment), so a rejected image leaves memory untouched.

## Why the registry is not a static initialiser

The library is a static archive. An object file that nothing references is not
linked into a program, so a module that registered itself from a static
constructor would silently vanish from the build — the worst kind of bug, since
`--list-cpus` would look plausible and the module would simply not be there.

`register_builtin_modules()` (`src/modules/builtin.cpp`) is therefore the
explicit list of what a build contains. Dropping a module from that list is a
linker error naming the missing function, not a runtime surprise. The C API
calls it before it answers anything about modules, so a caller never has to.

## The two registers of the core worth knowing

* `Bus::set_store_observer(fn, user)`: how a CPU that caches translated code
  learns that a store landed in a page holding code. It replaced a direct
  `Bus::cpu` back-pointer, which was the one place the address space knew about
  a processor. A callback plus a user pointer keeps `Bus` and `PageDesc`
  trivially copyable (the MMU code memsets and memcpys them) and keeps a
  non-ARM CPU from having to have a `Cpu` at all.
* `Machine::reinit()`: a full rebuild from the last configuration. The CPU goes
  first (it holds pointers into the bus), then the address space is released and
  rebuilt. `Bus::reset()` releases only the backing stores the bus allocated
  itself — a board's ROM mapped with `map_host`, or a `BufferDevice`, is the
  caller's memory and is left alone.
