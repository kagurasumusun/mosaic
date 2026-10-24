# Machine manifest validation

`mosaic-describe` defines format-independent, typed SoC/board schemas and
semantic validation. It has no external dependencies and does not depend on the
runtime or model implementations. `mosaic-xtask` owns TOML parsing, filesystem
traversal, and repository-wide policy, keeping the framework's dependency rules
intact.

## Commands

```sh
cargo xtask lint-catalog
cargo xtask check
```

`lint-catalog` validates `soc.toml`, `board.toml`, `cpu.toml`, `ip.toml`, and
`part.toml`; enforces global catalog ID uniqueness, namespaces, and canonical
ID/path agreement; resolves SoC CPU/IP references and board SoC/part references;
and checks duplicate I2C addresses and SPI chip-selects on the same declared
bus. Fixed metadata schemas reject unknown fields and invalid property types.

The SoC schema checks non-empty and unique local names, address-space and region
names, region kinds and sizes, 64-bit address overflow, equal-priority region
overlap, CPU-to-view references, block-to-view references, explicit MMIO
region targets, exact block/MMIO base-address agreement, and declared IRQ
endpoint roots. A block can declare multiple IRQ routes, including indexed
interrupt-controller inputs and independent outputs such as SP804 timer1/timer2
or PL190 IRQ/FIQ. Every region has an explicit `target`: a block name for MMIO
or a backing-store name for RAM/ROM. Higher-priority overlays are allowed,
consistent with runtime `FlatView` precedence. The current schema describes flat
regions; nested-container syntax and model-specific address-range verification
remain future work.

The board schema checks unique local names, non-empty connection declarations,
I2C address conflicts, and SPI chip-select conflicts. The graph builder resolves
explicit paths such as `soc.i2c0` and `board.mux0.channel3` to node/port pairs.
Address-conflict detection still compares the declared bus strings exactly, so
it cannot yet detect two different aliases that resolve to the same physical bus
or buses simultaneously connected through a mux.

CPU/IP metadata validation checks identity/spec/tags, known property keys and
types in port declarations, engine-selection metadata, and typed parameter
schemas. Part validation checks its canonical origin, bindings, provenance,
I²C/SPI settings, and register tables. These checks catch many schema typos,
and the graph edge validator checks declared port/pin names, indexed port ranges,
part bindings, mux-channel indices, and the broad source/sink role. It does not
yet prove that two sides' `contract` values are compatible, that metadata matches
the Rust implementation, or that the model matches silicon. A binding-aware
registry and full contract-compatibility checks remain future work.

## Validation layer boundary

The intended pipeline is:

```text
TOML files -> xtask TOML adapter -> mosaic-describe typed schema/validation
           -> MachineGraph -> registry resolution -> mosaic-assemble -> Machine
```

The TOML adapter, typed schema, basic semantic checks, repository-wide ID
resolution, and deterministic `MachineGraph` builder are implemented. The graph
normalizes CPU/peripheral/address-space/part/backing/clock/external nodes plus
address-space assignment, multi-route IRQ, clock, board, and bus edges. Tooling
checks graph endpoints against the declared catalog ports and part bindings.
Automatic catalog-to-Rust implementation registry resolution, full endpoint
contract compatibility, and runtime assembly from the graph are not yet claimed
as complete.
