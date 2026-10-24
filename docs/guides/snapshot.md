# Runtime snapshots (P4 initial slice)

`mosaic-core-runtime` now supports an **in-memory runtime checkpoint** that restores
these state domains together:

- the deterministic event queue and virtual time;
- the complete byte contents of every RAM/ROM backing block;
- typed State from every device registered with `register_snapshot_device`;
- clock-tree dynamics (gate enable, mux select, divisor), with structure checked as topology.

This is a runtime checkpoint, **not yet a whole-VM or persistent snapshot format**.
CPU engine contexts, signal sinks, external shared-memory mirrors, host
stream/socket endpoints, and file encoding are not included. Pending IRQ mailbox
edges can be saved separately through `IrqMailbox::save_edges` / `load_edges`;
they are not part of `MachineSnapshot` because a sink still cannot be registered
on the machine. The API rejects a machine containing an ordinary (non-snapshot)
device or any signal sink rather than silently saving an incomplete checkpoint.

## Model contract

Implement [`Snapshot`](../../framework/core/types/src/lib.rs) for the model,
then register it as a typed snapshot device:

```rust
machine
    .register_snapshot_device(node, model)
    .expect("unique node");
let checkpoint = machine.save_snapshot().expect("all runtime state is supported");
// ... run or modify the machine ...
machine.load_snapshot(&checkpoint).expect("same machine topology");
```

`Snapshot::validate` is side-effect free and runs before any restore. `Snapshot::load`
must validate again and leave the model unchanged on failure. `Snapshot::fingerprint`
hashes `(version, emulated state)` with `Fnv64` and must not include host ports.
A model's State must be `Clone + Send`; it must contain all emulated state but no
host handles. Host ports stay attached to the live model across restore. A device
with a host queue must snapshot the queue's emulated bytes, not copy or replace
the host endpoint.

Catalog functional models (PL011, PL022, PL061, PL190, SP804, SCS, STM32 GPIO /
RCC / USART, DW APB I2C, Arm926, LM75A, TCA9548A, BME280, MHU) implement this
contract. FIFO contents and timer `EventId`s are included. Byte streams, bus
initiators, GPIO ports, clock-input bindings, and downstream targets are not.
Restoring a GPIO `odr` does not re-drive the net.

## Validation and atomicity

Before mutation, `load_snapshot` checks the checkpoint schema; the exact address
views, node identities, RAM/ROM sizes, signal declarations, wiring, and clock
structure; queue capacity and event IDs; backing block identities and lengths;
every model's type and State version; and `state_hash`. The hash is recomputed
from the queue, RAM bytes, each device fingerprint, and clock dynamics. A mismatch
is `InvalidState`. It is an FNV checksum, not a cryptographic digest. The current
device States are held as rollback points while the model States are applied.
After all preflight checks pass, clock dynamics, RAM, and the event queue are
committed from staged replacements.

The graph fingerprint is a deterministic FNV-1a 64-bit compatibility checksum,
not a cryptographic digest. The checkpoint also holds the exact typed topology,
which is compared on restore so a checksum collision cannot make an in-memory
checkpoint compatible with a different graph. A future persistent format must
choose a versioned, cryptographic digest and specify canonical encoding.

The RAM/ROM checkpoint currently copies every backing byte. Dirty-page tracking,
copy-on-write, size limits, compression, and bounded parsing of untrusted files
are not implemented.

## Known limitations

- Only runtime-owned state is covered. `MachineSnapshot` is not accepted as a
  substitute for CPU register/engine state, a full multicore checkpoint, or
  external shared RAM. `IrqMailbox` edges checkpoint independently; aligning them
  with device output levels is still a driver coordinator.
- A machine with any `SignalSink` is rejected. Mailbox edges alone do not make a
  sink safe to register, because the sink's identity is still host wiring.
- Every registered device must implement `Snapshot` and use
  `register_snapshot_device`; mixed complete/partial snapshots are rejected.
- The checkpoint is an in-process Rust value. It cannot be serialized, migrated
  across process/version boundaries, or safely loaded from an untrusted source.
- Device `load` implementations must be atomic on error. Validation and rollback
  cover contract-compliant models; a model that mutates and then returns `Err`
  violates the trait contract.

Unicorn Engine's Rust 2.1.5 binding exposes `context_init`, `context_save`, and
`context_restore` for backend CPU state ([API](https://docs.rs/unicorn-engine/2.1.5/unicorn_engine/struct.Unicorn.html));
its opaque `Context` is deliberately non-`Send` ([auto traits](https://docs.rs/unicorn-engine/2.1.5/unicorn_engine/struct.Context.html#synthetic-implementations)).
Integrating it into a cross-machine snapshot needs an explicit engine-owned
checkpoint boundary; the context must not be treated as a portable byte blob.
The Unicorn backend remains the CPU implementation—the native CPU engine remains
a later phase.

## Example and verification

```sh
cargo run --locked --package mosaic-core-runtime --example snapshot
```

The example captures RAM, a device State, and a pending timer; mutates all three;
restores; then replays the timer. Its stdout is checked by CI against
`tests/machine-snapshot.stdout`. The unit test also restores the same checkpoint
into a second identical Machine and rejects a different memory topology without
mutating the destination.
