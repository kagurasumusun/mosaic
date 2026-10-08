# `debugger/`

Debugger integrations and inspection services: things that watch or drive a
machine from outside, without being part of the machine. Guest execution logic
does not belong here — what belongs here is the *protocol* side of debugging.

| directory | what it is |
| --- | --- |
| `gdb/` | a gdb stub: the remote serial protocol over a transport (TCP for `--gdb PORT`, memory for tests), driving any machine through the C API |

Like `host/`, nothing in `core/`, `arch/`, `devices/` or `boards/` imports
from here, and this layer talks to the machine only through the C API
(`brain_emu_t*`) — so a stub session works on any CPU module, JIT or
interpreter, and adding a CPU never touches it.
