# `host/` — the host-side services

Everything the emulator needs from the machine running it, kept out of the
guest's way: logging, byte I/O, filesystems, and the observers and consoles a
user drives. Nothing in here knows any CPU, any device, or any instruction —
the core and the modules never depend on this layer, and this layer talks to
them only through the C API (`brain_emu_t*`) or through function pointers a
device installs.

| directory | what it is | who uses it |
| --- | --- | --- |
| `log/` | the one logging facility: levels, a swappable sink, printf-style entry points | every component, tests capture it |
| `io/` | byte sinks and sources (stdio, buffers), the console facade, and the adapters that turn them into the function pointers a device takes | devices that want host I/O, tests |
| `fs/` | the filesystem service: the `Fs` interface, a host-directory implementation (`HostFs`), an in-memory one (`MemFs`) | a future guest-facing file bridge, tests, tooling |
| `monitor/` | the monitors: a `stats` observer module for `--monitor`, and the command monitor (a debugger's console) over the C API | the CLI (`--monitor-cmd`), tests, embedding code |

The direction of the dependencies is one-way: `host/` may use the core's C API
and its own modules, and nothing in `core/`, `arch/`, `devices/` or `boards/`
imports anything from here. A device that wants its bytes on the host takes
plain function pointers (`devices/uart/` is the example); it does not include
`host/io/`.

## The C API is the seam

The command monitor and the gdb stub (`debugger/gdb/`) drive a machine through
`brain_emu_t*` — the same handle the CLI and an embedding GUI hold — so they
work on any CPU module, JIT or interpreter, and adding a CPU never touches
them. That is the whole point of putting them in `host/` and `debugger/` rather
than in `core/`.
