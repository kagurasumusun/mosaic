# Modules: the core, and everything that plugs into it

This emulator is **one core plus modules**. The core owns the address space, the
run loop, the machine's lifetime and the C API. A module owns one piece of the
emulated world and registers itself under an id. The ARM926EJ-S — the reason this
project exists — is the *first* CPU module, not the shape of the emulator; a
second processor is a new directory, not a new emulator.

```
core/include/brainemu/bus.h        Bus: regions, per-page descriptors, byte access
core/include/brainemu/module.h     ModuleInfo, CpuModule, ModuleRegistry
core/include/brainemu/machine.h    MachineConfig, Machine (bus + CPU + devices)
core/include/brainemu/cpu.h        CpuCore: the whole CPU contract
core/include/brainemu/device.h     Device + FuncDevice/BufferDevice
core/include/brainemu/loader.h     Loader (probe + load) and the image description
core/include/brainemu/monitor.h    Monitor: an observer the run loop reports to
core/src/                          the address space, the registry, the machine, the C API
arch/arm/cpu/classic/arm9/arm926ej-s/                   the primary CPU: ARMv5TEJ, CP15 + MMU, JIT
arch/experimental/toy8/                     a second CPU, in 300 lines, on purpose
devices/console/                   a device module: a window that collects text
devices/system/exit/                      a device module: a store that stops the run
devices/timer/  devices/interrupt/intc/      a device with an output, a device with inputs
loaders/elf/elf32/  loaders/raw/       one image format per directory
boards/                            named machine presets
builtin/builtin.cpp                the list of what this build contains
testkit/test_support.h             what every suite shares (CHECK, LevelLog, ...)
```

**The layout rule** (the long version is in `docs/ARCHITECTURE.md`): `core/` is the
foundation, and everything else is a component of exactly one kind, one directory
per module.  A component directory holds its sources, the single header that
declares its `register_*()`, and a `tests/` directory with its own suites.  The
build globs the kind's directories and `builtin/builtin.cpp` calls the
registration functions, so **adding a component is a directory plus two lines in
that one file** - nothing else in the tree has to be touched, because nothing else
names it.


## The kinds of module

| kind | what it is | who calls it | examples in this tree |
| --- | --- | --- | --- |
| `cpu` | a processor: registers, an instruction set, its own timing | the machine, at slice boundaries | `arm926`, `toy8` |
| `loader` | an image format | `Machine::load_image`, by content | `elf32`, `raw` |
| `device` | something mapped into the address space | the bus, per access | `exit`, `console`, `timer`, `intc` |
| `monitor` | an observer of a running machine | the run loop, per slice | none built in; the tests declare one |
| `board` | a named machine preset: CPU, devices, wires | `Machine::init`, once | `minimal`, `timer`, `dual-timer` |

All five kinds are *registry* modules and all five are created through a factory
(`CpuModule`, `Loader`, `DeviceModule`, `MonitorModule`, `BoardModule` in
`core/include/brainemu/module.h`), so a machine can be assembled out of ids:
`MachineConfig::cpu_id`, `devices[]`, `monitors[]`, `wires[]` and `board`, or
`Machine::add_device_by_id` / `add_monitor_by_id` / `add_wire` after the fact.
The C API mirrors the latter (`brain_add_device_module`,
`brain_add_monitor_module`, `brain_add_wire`) and `brain_module_*` lists every
kind the build contains.

Only a CPU module is *driven*; the others are called. A CPU module is therefore
the only one with an execution contract, and it is the one this document is
mostly about.

## The CPU contract

`CpuCore` (`core/include/brainemu/cpu.h`) is deliberately small. A module says
what it is (`info()`), binds to the address space (`init`), executes when asked
(`run_until`, and its two one-sided cases `run_insns`/`run_cycles`, plus `step`),
can be interrupted and reset, and reports
registers and counters. Everything else — opcodes, exceptions, an MMU, a CP15, a
JIT — belongs to the module and the core never learns about it.

Three rules shape the interface:

1. **Every call is a slice boundary.** `run_until(max_insns, max_cycles)` means
   "retire at most `max_insns` and charge at most `max_cycles` (0 = no bound on
   that axis), then come back", not "tell me about each one". The two-sided
   bound is what a machine with devices needs: a guest waiting for an interrupt
   retires nothing at all, so a slice bounded on instructions alone would never
   come back and the device that is supposed to wake it would never be ticked.
   Nothing in the core calls into a module per instruction, so the virtual calls
   are noise and a module's inner loop stays a plain, non-virtual, monomorphic
   function over its own state.  (This is also why there is no
   `execute_one_instruction()` hook: an ISA with variable-length or parallel
   instructions is exactly the case a per-instruction interface punishes.
   `step()` exists for debuggers, and is one instruction by another name.)
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
* the run loop calls `run_until` again only while the slice is productive: a
  module that returns early is normal (it stopped, or it is waiting), and a
  slice in which nothing happened ends the call instead of being asked again;
* a stop request belongs to the run it interrupts.  The machine reads
  `stop_requested()` after every slice and ends the call there; a module clears
  it when a run starts, so a stopped machine is runnable again without a reset
  (a caller that wants it to stay stopped keeps its own flag - a monitor's
  watchpoint, an exit device);
* the slice ends in exactly one place - the devices are ticked and the monitors
  are told - so a module never has to know that either exists.

What a module must guarantee the core:

* `run_until(max_insns, max_cycles)` returns once either bound is met, the CPU
  is stopped, or it cannot get anywhere (its state is unchanged and no
  instruction will change it) - a module must never loop for ever on its own
  waiting for something only the machine can give it.  `run_insns(n)` (0 = until
  stopped) and `run_cycles(n)` are the two one-sided cases;
* `insns()` and `cycles()` are the machine's clock: the device tick and every
  counter the C API reports come from them;
* a fault the module cannot complete is the module's own business to model. The
  core has no exception layer, because an exception *is* an instruction-set
  concept: `pending_exception()` is an opaque id the module chooses.

## Adding a CPU: five steps

The worked example is `arch/experimental/toy8/`, which is deliberately a
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
5. **Register it.** A directory `arch/<arch>/cpu/<family>/<id>/` with a header declaring one
   `register_<id>_module()`, and two lines in `builtin/builtin.cpp`: the include
   and the call. The build discovers sources recursively under `arch/` (CPU modules
   and their `jit/` subdirectories included), so no build edit is needed either.

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
`jit_fast_mem` and `big_endian_data` (`arch/arm/cpu/classic/arm9/arm926ej-s/arm926.cpp`).

### Devices and monitors after the machine exists

In C++, before building (the base is optional: no base means *the module's own
default address*, which is the difference between `--exit` and `--exit-port A`):

```cpp
MachineConfig cfg;
cfg.add_device(ModuleSpec("console"));          /* its default base   */
cfg.add_device(ModuleSpec("exit", 0x10000010)); /* and an explicit one */
cfg.add_monitor(ModuleSpec("slicer"));
if (!machine.init(cfg)) { /* last_error() names the id that was refused */ }
```

Through the C API, after creating the machine:

```c
brain_emu_t* e = brain_emu_create(NULL);          /* the default machine   */
brain_add_device_module(e, "console", 0, 0);      /* 0,0 = module default  */
brain_add_device_module(e, "exit", 0x10000010, 1);
brain_add_monitor_module(e, "slicer");
for (uint32_t i = 0; i < brain_device_count(e); ++i) {
  for (uint32_t f = 0; f < brain_device_field_count(e, i); ++f)
    printf("%s=%s\n", brain_device_field_name(e, i, f),
                      brain_device_field_value(e, i, f));
}
```

An id nothing registered is refused with the ids that would have worked
(`no device module named 'uart' (have: exit, console)`), the same way an unknown
`--cpu` is.

### Adding a device

A device is `devices/<id>/`: one `.cpp` with the device class and its
`DeviceModule`, one header declaring `register_<id>_module()`, and two lines in
`builtin/builtin.cpp`. If the device has tests they go in `devices/<id>/tests/`
or `devices/tests/`, and they are picked up by the same glob that builds every
other suite. `devices/timer/` (an output), `devices/interrupt/intc/` (inputs) and
`devices/console/` (no interrupts at all, plus the device class its own header
declares) are the three shapes.


A device module is a factory for a `Device` (`core/include/brainemu/device.h`)
plus its identity: implement `info()`, `create(base)` and, if the window has a
home, `default_base()`. `Machine::add_device_by_id("uart")` then builds it,
calls `on_attach(*this)` so the device can keep a pointer to the machine if it
needs one (the exit port does: it calls `Machine::stop`), attaches it to the bus
and resets it. `--device uart@0x80000000` is the CLI spelling; the C API's is
`brain_add_device_module`.

A device that can interrupt declares its outputs - `irq_count()` names how many,
`irq_name(i)` names them - and drives them with `drive_irq(i, level)`.  It does
not know, and cannot know, where they go: a *board* wires each one to a CPU input
by name (below).  Driving a line nothing is wired to is fine and does nothing,
which is what lets a device be used on its own in a test.

A device that can *be* interrupted declares inputs - `input_count()`,
`input_name(i)` and `set_input(i, level)`, the mirror of `drive_irq` - and a wire
may target those instead of the CPU's (below).  The default has none, so a
peripheral that only speaks outwards needs no code at all; a device that combines
its inputs (an interrupt controller, a GPIO block, a DMA engine's request lines)
drives its own output from `set_input`, and the machine carries that on.  A
device may also need a *per-machine name* when two of them are built from one
module: `Device::set_instance` (usually through `ModuleSpec::as`,
`brain_add_device_named` or `--device ID@A#NAME`) is what a wire and a tool then
call it.  `drive_irq` is
level-sensitive and reports only *changes*, so a peripheral that re-asserts an
already-high line costs nothing.

Two rules the shipped ones follow:

* **A device reports its own state.** `field_count()` / `field_name(i)` /
  `field_value(i)` are name/value text the core never interprets, and `--stats`
  prints them for every device and monitor. That is how the console and the exit
  port show up in a tool that knows nothing about either.
* **One page, several windows.** A device's window is what it answers for; the
  bus claims the whole 4 KiB page when the page is free (so a stray access next
  to a four-byte register is not an abort), and a second device may map into a
  page the first already holds as long as the two windows do not overlap and both
  fit in that one page - which is what a real peripheral map looks like (the
  i.MX28's test ports are four words inside the 0x10000000 page). The page's
  region then routes by address, and an access outside every window still reaches
  the device that owns the page. Unmapping a shared page takes the whole group
  with it.

### Wiring a device to a CPU

Neither end knows the other's type.  A device has outputs (`irq_count`,
`irq_name`, `drive_irq`); a CPU has named inputs (`CpuCore::input_count`,
`input_name`, `set_input` - whose default maps input 0 to IRQ and 1 to FIQ, so
an ARM-shaped CPU needs no code for it); and a machine carries the level from one
to the other:

```cpp
MachineConfig cfg;
cfg.add_device(ModuleSpec("timer"));                  /* 0x10001000 by default */
cfg.add_wire("timer", "irq", "irq");                  /* device.out -> cpu.in  */

/* Two of the same module need instance names, and a sink may be a device: this
 * is a machine where t0 and t1 share one CPU interrupt line. */
cfg.add_device(ModuleSpec("timer", 0x10001000u, true, "t0"));
cfg.add_device(ModuleSpec("timer", 0x10001100u, true, "t1"));
cfg.add_device(ModuleSpec("intc"));                   /* 4 in, 1 out           */
cfg.add_wire("t0", "irq", "intc.irq0");               /* device.out -> dev.in  */
cfg.add_wire("t1", "irq", "intc.irq1");
cfg.add_wire("intc", "irq", "irq");                   /* ...          -> cpu.in */
Machine m;
if (!m.init(cfg)) { /* last_error() names the end that is missing */ }
```

The destination decides the kind: a bare name is a CPU input, a dotted one is a
device input (`intc.irq0`).  `--board dual-timer` is that machine, and
`devices/interrupt/intc/` is the controller in it.

The wire is resolved by name at the moment a level changes, not when it is
declared, which is what makes the order above irrelevant - a board may name a
device the machine has not built yet - and what makes `brain_add_wire` work on a
machine that is already running:

```c
brain_add_wire(e, "timer", "irq", "irq");       /* 0 unless an end is missing */
```

Refusals are specific: an unknown device, an output the device does not have, an
input the CPU does not have - each says what *does* exist.  A missing wire is not
an error at run time, though: an unwired output is simply dropped, so the same
device works in a machine, in a unit test and in a debugger without a flag.

### Adding an interrupt controller

`devices/interrupt/intc/intc.cpp` is the worked example of a device with
*inputs*, and the shape a real collector (the i.MX28's, an ARM PL190) has: N input
pins, a mask register, a status/pending pair, and one output that is the OR of the
unmasked inputs.  The three pieces that make it a device like any other:

* `input_count()`/`input_name(i)` declare the pins a wire may name, and
  `set_input(i, level)` is told every change - the mirror image of `drive_irq`;
* `drive_irq(0, ...)` on the way out, driven from `set_input` when the mask says
  the change matters: that is the whole aggregation, and the machine carries it on
  to whatever the next wire names;
* no `tick` override at all - a controller is combinational, and a device that
  needs no clock simply does not ask for one.

Its registers are `0x00` mask, `0x04` status (raw pin levels), `0x08` pending
(masked, read-only), `0x0C` set and `0x10` clear for software interrupts.  A
controller that needs priorities, edge detection or a per-line enable of its own
grows registers; nothing in the core changes.

### Adding a board

All the boards live in `boards/generic/boards.cpp` with `boards/generic/boards.h` declaring
`register_builtin_boards()`, because a board *is* a preset: it has no behaviour of
its own to grow, and a preset is only meaningful next to the other presets of the
same build.  A board is the answer to "what machine is this?".  It owns no behaviour - it
fills in the same `MachineConfig` a caller would, and the machine then builds it
exactly as if the caller had written it out by hand:

```cpp
class MyBoard final : public BoardModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo k = [] {
      ModuleInfo i;
      i.id = "my-board";
      i.name = "my machine";
      i.kind = ModuleKind::Board;
      i.description = "a CPU, a UART and a timer, with the timer on IRQ";
      return i;
    }();
    return k;
  }
  void configure(MachineConfig& cfg) const override {
    cfg.cpu_id = "arm926";
    cfg.add_device(ModuleSpec("console"));
    cfg.add_device(ModuleSpec("timer"));
    cfg.add_wire("timer", "irq", "irq");
  }
};
```

Register it (`ModuleRegistry::instance().add_board(&mod)`) and it appears in
`--list-modules` and works as `--board my-board`.  `Machine::init` applies the
board's layout first and then the caller's own additions on top, so
`--board my-board --device exit` is the board plus one peripheral, and a board
never takes away an explicit choice.

Boards are also the natural place for a *machine* to be described without writing
any code at all: `MachineConfig` is a plain struct, so a table of them (an array
of `{id, description, devices, wires}`) is a fine board registry for a build that
wants many machines.  What a board must *not* do is reach into a CPU or a device:
it only names them.

### The timer, as the reference device

`devices/timer/timer.cpp` is the smallest device that exercises the
whole path a real peripheral needs, and is meant to be copied:

* a window in the address space (`base()`, `size()`, byte-addressable registers,
  reads and writes at their natural size through `read`/`write`);
* a **clock**: `tick(cycles)` is called by the machine once per slice with the
  cycles that slice charged, which is how a device observes time passing at all;
* an **interrupt output**: `irq_count() == 1`, `irq_name(0) == "irq"`, driven
  high when the counter expires and low when the guest acknowledges - a level,
  latched, so an expiry the CPU could not take is not lost;
* **self-description**: `field_count()`/`field_name()`/`field_value()` give a
  tool `period`, `ticks`, `pending` and `running` without the core knowing what a
  timer is.

Its registers are `0x00` period (writing it restarts the counter), `0x04` count
(read-only), `0x08` control (bit 0 enable), `0x0C` status (bit 0 pending, bit 1
running) and `0x10` acknowledge (write anything).  The acknowledge is its own
register on purpose: a bit inside the control register would make "acknowledge"
and "keep running" the same write, so a guest that acknowledges without
repeating the enable bit would silently stop its own timer.  A device that
models a real chip is this file with that chip's register map; the board that
wires it is ten lines.

### Adding a monitor

A monitor does not execute and is not mapped anywhere: `Machine::add_monitor`
puts it in the run loop, which calls `on_slice(machine, retired)` after every
slice (devices already ticked, CPU already returned) and `on_reset(machine)`
after a reset. A *monitor module* is a `MonitorModule` factory;
`Machine::add_monitor_by_id("profiler")` builds one, the machine owns it, and
`field_*` describes it to a tool exactly like a device.  This is where a tracer, a profiler, a coverage collector, a
watchpoint layer or a gdb stub belongs - the core gains a *report*, not a
feature.  A hook may read registers and memory freely, and may call
`Machine::stop()`: the stop takes effect at the next slice boundary, which is
exactly the semantics a watchpoint wants.  `core/tests/test_machine.cpp` has a
counting monitor and a stopping one.

### Adding a loader

A loader is `loaders/<id>/`: a `.cpp` with the `Loader` subclass and the
registration, a header declaring `register_<id>_loader()`, and two lines in
`builtin/builtin.cpp`.  `loaders/image.h` has the few things two loaders would
otherwise both write (little-endian field readers, a hex formatter, and the
"is this whole range writable?" question every loader has to ask before it writes
a byte).

Derive from `Loader`, implement `probe` (does this buffer look like mine?) and
`load` (place it through the bus, report `ImageInfo`), and add it in
`loader.h`'s spirit — `loaders/tests/test_loader.cpp` contains a complete 20-line
example that registers itself inside the test. Two rules the built-in ones
follow and a new one should too: **write physical memory** (the CPU comes out of
reset with its MMU off) and **refuse before you write** (`Bus::range_writable`
per segment), so a rejected image leaves memory untouched.

## Why the registry is not a static initialiser

The library is a static archive. An object file that nothing references is not
linked into a program, so a module that registered itself from a static
constructor would silently vanish from the build — the worst kind of bug, since
`--list-cpus` would look plausible and the module would simply not be there.

`register_builtin_modules()` (`builtin/builtin.cpp`) is therefore the
explicit list of what a build contains. Dropping a module from that list is a
linker error naming the missing function, not a runtime surprise. The C API
calls it before it answers anything about modules, so a caller never has to.

## The two registers of the core worth knowing

* `Bus::set_store_observer(fn, user)`: how a CPU that caches translated code
  learns that a store landed in a page holding code. It replaced a direct
  `Bus::cpu` back-pointer, which was the one place the address space knew about
  a processor. A callback plus a user pointer keeps `Bus` and `PageDesc`
  trivially copyable (the MMU code memsets and memcpys them) and keeps a
  non-ARM CPU from having to have a `Cpu` at all. `Bus::note_store(addr, size)`
  is the one way to raise it, and *every* writer of guest memory has to call it:
  the CPU's store path (fast RAM and the MMU slow path), `Bus::write_bytes` (the
  loaders, `brain_write_mem`), the ELF loader's bss fill and `brain_poke*`,
  which writes into the host buffer behind a page. A writer that skips it leaves
  a translated block running over bytes that changed underneath it, which is the
  one bug this hook exists to make impossible. A CPU with no code cache has no
  observer, and then the call is one null check.
* `Machine::reinit()`: a full rebuild from the last configuration. The CPU goes
  first (it holds pointers into the bus), then the address space is released and
  rebuilt. `Bus::reset()` releases only the backing stores the bus allocated
  itself — a board's ROM mapped with `map_host`, or a `BufferDevice`, is the
  caller's memory and is left alone.
