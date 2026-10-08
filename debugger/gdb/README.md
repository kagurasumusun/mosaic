# `debugger/gdb/` — a gdb stub

`GdbStub` speaks the gdb remote serial protocol (RSP) to one `GdbTransport` and
drives the machine through the C API (`brain_emu_t*`) — the same handle the CLI
and a GUI hold — so it debugs any CPU module, JIT or interpreter, without
knowing a single instruction. What it can do is what the C API can do: read and
write registers, read and write memory, single-step, run, and stop.

| file | what it holds |
| --- | --- |
| `transport.h` | `GdbTransport` — the byte stream the stub talks over: `read` (bytes, or -1 when closed), `write`, `connected`. `MemTransport` (in/out buffers, for tests) lives here too. |
| `tcp_transport.{h,cpp}` | `TcpTransport` — POSIX listen/accept on a port, one client at a time. This is the `--gdb PORT` half: the CLI listens, waits for a debugger, serves it until it detaches, then runs normally. |
| `gdbstub.{h,cpp}` | `GdbStub` — the protocol: `$data#cksum` framing, `+`/`-` acks, `QStartNoAckMode`, `qSupported`, `?` → `S05`, `g`/`G`/`p`/`P` (registers, little-endian byte order on the wire), `m`/`M`/`X` (memory; `X` handles the `0x7d` escape), `Z0`/`z0`/`Z1`/`z1` (breakpoints), `c`/`s` (continue/step), `k`/`D` (kill/detach), `H`/`T`/`qC`/`qfThreadInfo`/`qsThreadInfo`/`qOffsets`/`qAttached`/`qSymbol`, `vCont?`, and `qXfer:features:read:target.xml` (the register layout, built from `CpuInfo`, so what the stub sends and what gdb expects are the same list). |
| `tests/test_gdbstub.cpp` | the suite: a scripted session over `MemTransport` (framing, checksums, acks, no-ack mode, breakpoints, the `X` escape, unsupported packets), plus a real TCP session over a loopback socket. |

## Breakpoints

Breakpoints are the stub's own: `Z0`/`z0` add and remove addresses from a
vector, and `c` (continue) steps the machine instruction by instruction while
the vector is non-empty, checking the PC each time — a breakpoint is a stop,
not a filter, so this is the only honest way to honour one at an address the
core's run loop does not know about. With no breakpoints, `c` runs whole
slices and reports when the machine stops on its own (an exit device, a
fault): `brain_run_insns` returning 0 is how the caller learns.

## Wire details worth knowing

- Registers travel as hex of their bytes in target byte order: a 32-bit value
  `0x12345678` goes on the wire as `78563412` (this project's targets are
  little-endian; BE8 data is a CPU option the stub does not special-case).
- `m` reads RAM/ROM in bulk, and anything else the way the guest reads it, so
  a device's registers answer. A read of an unmapped address takes a data abort
  in the machine, exactly as a guest read would — the stub does not hide that.
- `M`/`X` writes go to RAM/ROM only (`brain_write_mem`, all-or-nothing).
- No FPU is modelled; the register layout is exactly the core's own order (the
  GPRs, then the status word, then the saved status word when the CPU has one).
- An unsupported packet gets the empty reply (`$#00`), which is how RSP says
  "no" — never an error string.

## Using it

```sh
brainemu --gdb 1234 image.bin        # then, in gdb: target remote :1234
```

The stub serves one debugger, then the run continues as it would have without
it. A test drives the same stub in-process with `MemTransport` — see
`tests/test_gdbstub.cpp` and `tests/integration/test_host_stack.cpp`.
