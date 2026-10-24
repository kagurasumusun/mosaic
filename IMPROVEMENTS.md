# Mosaic code hardening review

This document summarizes a static hardening pass over the working tree. It is not a claim that the repository is fully correct or that tests have passed: this environment has no Rust toolchain, so the added and existing Rust tests could not be compiled or executed.

## Correctness fixes

- `Machine::run_until` checks its event budget before removing the next due event. Budget exhaustion now leaves the next event queued and the virtual timeline resumable.
- `EventQueue::reschedule` replaces an owned pending event without a spare queue slot and preserves the old event on time/counter validation failures. `Ctx::reschedule` provides a backwards-compatible default and the runtime context uses the atomic replacement.
- SP804 timer reprogramming commits the new state only after scheduling succeeds; it preserves the frozen count on live control updates, applies 16/32-bit counter-width masking to the running count, and ignores unknown event tokens instead of aliasing timer 2.
- Snapshot rollback now includes the device whose `load()` mutates state and then returns an error.
- MMIO device ownership uses a drop guard, so unwinding from a device callback restores the device to the arena.
- Machine-wide node-ID collisions and duplicate source/signal wiring are rejected rather than silently creating ambiguous registries/topology.
- A CPU engine's `CapabilityMissing` stop is no longer treated as a successful quantum. Single-core and multi-core drivers flush pending IRQ edges before another quantum and publish shared-memory writes from completed/partially completed slices before returning later errors.
- IRQ mailbox delivery reports a failing edge even after earlier edges in that flush succeeded; accepted edges are removed while the rejected edge and successors remain queued. Concurrent drain consumers are serialized without holding the producer queue lock during backend callbacks.

## Signal, peripheral, and interconnect hardening

- Added `set_optional_signal`, which tolerates an unwired optional output but propagates queue exhaustion, time overflow, and other runtime errors.
- Updated PL011, PL022, PL061, PL190, SP804, STM32F1 GPIO/RCC/USART, DW_apb_i2c, and LM75A output paths so a rejected signal transition is not cached as successfully delivered and can be retried.
- Re-evaluate interrupt outputs after read-to-clear/read-side effects in the affected peripheral models.
- Reject 64-bit accesses to 32-bit register blocks before reads/writes can trigger FIFO or interrupt side effects; reject oversized writes where the model only supports 32-bit values.
- PL022 separately tracks the chip-select state accepted by the SPI interconnect, so a rejected assertion can be retried by repeating CR1.
- STM32F1 GPIO retries pending pin notifications after runtime queue failures and emits the ODR level when configuration changes a pin into a GPIO output.
- RCC tracks successfully queued clock-gate output levels separately from its ENR registers, attempts independent outputs even after one fails, and retries on later writes.
- Added `GpioPort::release` to represent high impedance separately from actively driving logic-high. PL061 releases a pin when it becomes input/alternate-function, and the bit-bang adapter forwards release.
- GPIO net resolution now considers both board-level and per-pin internal pulls; output direction and open-drain-high use high impedance. Sink callbacks run outside the net mutex, duplicate sinks are rejected, and notifications drain in FIFO order through re-entrant callbacks.
- PinMux rejects duplicate function attachment and leases downstream ports while invoking callbacks outside its mutex, avoiding deadlocks and preserving port ownership across unwinding. Added `select_with_ctx` for runtime route changes: when the old function is no longer selected by another pin, it releases that port before committing the new route, and retains the old route if release fails.

## Regression coverage added or revised

The changes add/update tests for event-budget exhaustion without event loss; queue-capacity-neutral timer replacement; failed re-schedule preserving the old event; snapshot rollback after a mutating `load()` returns an error; MMIO callback panic preserving the registered device; duplicate node/wiring validation; IRQ mailbox partial failure; capability-missing driver stops; pin internal pulls; re-entrant GPIO sink delivery; rejected SPI chip-select retry; unsupported timer-token isolation; SP804 live counter preservation and 16-bit counter-width behavior; GPIO output-notification retry after queue exhaustion; context-aware PinMux switching; typed manifest rejection cases; and declarative graph resolution.

## Changed Rust files

- `framework/describe/src/{lib.rs,schema.rs,graph.rs,endpoint.rs,tests.rs}`
- `framework/core/types/src/lib.rs` (`VDelta`)
- `framework/assemble/src/{driver.rs,multicore.rs}` (`VDelta` use)
- `xtask/src/{catalog.rs,lib.rs,main.rs}`
- `framework/core/runtime/src/{events.rs,context.rs,machine.rs}`
- `framework/contract/model/src/lib.rs`
- `framework/contract/gpio/src/lib.rs`
- `framework/assemble/src/{driver.rs,multicore.rs,irq.rs}`
- `framework/interconnect/{gpio-net,pinmux,i2c-bus}/src/lib.rs`
- `catalog/ip/arm/{pl011,pl022,pl061,pl190,sp804}/src/lib.rs`
- `catalog/ip/st/{gpio-f1,rcc-f1,usart-f1}/src/lib.rs`
- `catalog/ip/synopsys/dw-apb-i2c/src/lib.rs`
- `catalog/parts/nxp/lm75a/src/lib.rs`


## Design and structure improvement

- `mosaic-describe` now provides format-independent typed `SocManifest` and
  `BoardManifest` models with semantic validation, without adding dependencies
  to the framework layer. The schema and graph builder were split into
  `schema.rs`, `endpoint.rs`, and `graph.rs` to keep format parsing, semantic
  validation, endpoint paths, and graph integrity as separate responsibilities.
- Added a TOML adapter in `xtask/src/catalog.rs`. It rejects unknown fields in
  fixed schema sections instead of silently accepting misspelled keys, while
  preserving open-ended model-specific `clk` and `conn` values as `ConfigValue`.
- Added `cargo xtask lint-catalog` and integrated it into `cargo xtask check`
  / `doctor`. It checks catalog ID uniqueness/namespaces, SoC and board
  structural invariants, SoC CPU/IP references, board SoC/part references,
  duplicate I2C addresses, duplicate SPI chip-selects, explicit MMIO region
  targets and block base-address consistency, and ambiguous equal-priority
  region overlaps. It also verifies canonical ID suffixes match catalog paths and
  builds a normalized `MachineGraph` containing CPU, peripheral, address-space,
  part, memory, clock, IRQ, and board/bus connectivity. Higher-priority overlays remain legal.
- Added `VDelta`, a duration type distinct from absolute `VTime`, and routed
  clocked driver and SP804 deadline calculations through checked `VTime + VDelta`
  operations to reduce accidental absolute-time/delta mixing and centralize
  overflow handling.
- Kept file-system walking and TOML parsing in the tooling layer so
  `framework/describe` does not violate the project's external-dependency
  boundary. Added regression tests for checked-in manifests, misspelled keys,
  duplicate IDs/names/addresses, invalid address ranges and region overlaps.
- Added a deterministic `MachineGraph::build` step after manifest validation;
  it normalizes graph node IDs and edges and rejects missing bus/port targets.
  The catalog registry and `MachineGraph -> assemble -> Machine` path remain
  unimplemented, and the distinction is documented.

## Validation status

- Static delimiter/lexical-state scan passed for all changed Rust files; source trailing-whitespace and `git diff --check` also passed. This is not a Rust parser, formatter, or type check.
- Python preflight parsed all 76 checked-in TOML files plus `Cargo.lock`, and validated the 19 catalog manifests: 19 unique IDs, canonical ID/path agreement, 12 resolvable `use`/`soc` references, target/base consistency, and 16 catalog-backed graph edge endpoints passed in the independent preflight script. An additional MMIO-window check ensures each mapping fits the declared port size. The Rust `lint-catalog` / `MachineGraph::build` implementation itself has not been executed in this environment.
- `cargo test --locked --workspace --all-features`: **not run**; the command failed to start because `cargo` is not installed in the environment.
- `cargo fmt`, `cargo clippy`, type checking, and the new regression tests remain unverified.

Before merging, run these commands in a Rust 1.85+ development environment:

```sh
cargo fmt --all -- --check
cargo xtask check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

## Remaining issues not safely closed in this pass

- `MachineGraph` normalizes declared buses and mux ports into graph edges, and tooling checks their declared catalog port names/role/index bounds. It still does not resolve distinct aliases to the same physical bus. Thus electrical conflicts that depend on mux reachability are not fully detected. Full `contract` compatibility across both endpoints is also not yet validated.
- `Machine::dispatch_event` pops an event before invoking a model callback. If that callback mutates state and then returns an error, the event is consumed and the mutation may be partial. Automatically re-queueing it could duplicate side effects, so a proper solution needs a documented transactional callback contract or an explicit fail-stop/dead-letter policy.
- Whole-machine snapshot remains incomplete: CPU contexts, IRQ mailbox contents, shared-RAM mirrors, host I/O, persistent encoding/migration, and replay state are not covered.
- The current Unicorn backend still lacks actual external-interrupt injection; the project documents that line edges may only be recorded by the adapter. Timing also still uses fixed instruction periods rather than a resolved clock tree.
- `GpioPort::release` is a public trait API addition. All in-repository implementors were updated, but external downstream implementors must add the method.
- PinMux's context-free `select()` remains intended for initial composition only; dynamic selection should use the new `select_with_ctx()` API. If downstream release reports an error, generic rollback cannot guarantee that the underlying electrical peer did not partially change state.
- License/distribution decisions and performance benchmarking remain project-level tasks.

This is a targeted, broad hardening pass based on source inspection. It does not establish a formal proof of correctness or guarantee that no other bugs remain.
