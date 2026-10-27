//! The device model catalog: every model, registered by its own module.
//!
//! This is QEMU's `TypeInfo`/`MODULE_INIT` mechanism in Rust form (ADR 0025).
//! Each function directory owns a `catalog.rs` that registers its models —
//! a model's default instantiation is the model's own knowledge, never a
//! board's — and this module aggregates them into one [`ModelEntry`] table:
//!
//! * a **machine** builds its registry with [`device_registry`] and never
//!   writes a factory closure: a board is its manifest (parts, edges,
//!   regions) plus the wiring only that board knows (which UART is the
//!   console, which pin carries the LED);
//! * an **operator** lists the catalog with `mosaic --catalog`, one line per
//!   model, the way `qemu-system-x86_64 -device help` lists devices;
//! * a **lint** (`xtask lint-catalog`) cross-checks the table against the
//!   `manifests/hw/` tree both ways, so a model without a manifest or a
//!   manifest without a model is a CI failure, not a surprise.
//!
//! ## What the catalog covers, exactly
//!
//! * **Everything constructible from placement + wires + host attachments**:
//!   register-level models (UARTs, GPIOs, interrupt controllers, timers,
//!   RTCs, clock controls, display, input, DMA, SD/eMMC hosts, NICs, USB
//!   hosts, virtio): the factory reads its IRQ outputs from declared `irq`
//!   edges and its clock inputs from declared `clk` edges.
//! * **Bus-mediated leaf parts** (I2C/SPI sensors, bus switches): their bus
//!   *membership* is board-unique wiring — which address a sensor sits at,
//!   which controller owns the bus — so boards attach them through the
//!   `bus` fabrics in code, exactly as QEMU boards wire `i2c_bus` in C.
//!   The models are no less reusable for it; the *fabric* is the reuse.
//!
//! ## The factory contract
//!
//! A default factory may read (ADR 0025):
//!
//! * `ctx.output("irq-port")` — the per-node signal the graph assigned to a
//!   declared IRQ output edge;
//! * `ctx.incoming` — the `irq`/`clock` edges that arrive at this node
//!   (clock wires carry the vendor signal resolved through this entry's
//!   [`ModelEntry::outputs`] table);
//! * `ctx.parameters` — the manifest's model parameters;
//! * `ctx.host_uart()` — a host console endpoint the machine harness
//!   attached for this node.
//!
//! It may not invent placement (regions), addresses or IRQ numbers — those
//! are board-unique facts and stay in the board's manifest.

use mosaic_assemble::{DeviceRegistry, HostAttachments, InstantiateCtx, MaterializeError};
use mosaic_exec::SignalId;
use mosaic_runtime::Machine;

/// A model's default instantiation function.
pub type SpawnFn = fn(&InstantiateCtx, &mut Machine) -> Result<(), MaterializeError>;

/// One catalog entry: a model's identity, its vendor output table and its
/// default instantiation. ` &'static` throughout: a model's registration is
/// compile-time knowledge, exactly like QEMU's `TypeInfo` tables.
pub struct ModelEntry {
    /// The catalog ID (`ip:st/usart-f1`), the manifest `[identity] id`.
    pub id: &'static str,
    /// One-line description, as `--catalog` prints it.
    pub summary: &'static str,
    /// Vendor output table: this model's *model-internal* signal numbers by
    /// port name (the RCC's `gate-apb2-iopa` gates). `clk` edges resolve
    /// their source signal through this table (ADR 0025).
    pub outputs: &'static [(&'static str, SignalId)],
    /// Default instantiation from placement + wires + host attachments.
    pub spawn: SpawnFn,
}

impl ModelEntry {
    /// The vendor signal one output port drives.
    pub fn output_signal(&self, port: &str) -> Option<SignalId> {
        self.outputs
            .iter()
            .find(|(name, _)| *name == port)
            .map(|(_, signal)| *signal)
    }
}

/// The whole catalog in manifest order: one entry per `catalog.rs`.
pub fn entries() -> Vec<&'static ModelEntry> {
    let mut all: Vec<&'static ModelEntry> = Vec::new();
    macro_rules! extend {
        ($module:expr) => {
            all.extend($module.iter());
        };
    }
    extend!(crate::char::catalog::MODELS);
    extend!(crate::display::catalog::MODELS);
    extend!(crate::dma::catalog::MODELS);
    extend!(crate::gpio::catalog::MODELS);
    extend!(crate::i2c::catalog::MODELS);
    extend!(crate::i3c::catalog::MODELS);
    extend!(crate::ide::catalog::MODELS);
    extend!(crate::input::catalog::MODELS);
    extend!(crate::intc::catalog::MODELS);
    extend!(crate::mailbox::catalog::MODELS);
    extend!(crate::misc::catalog::MODELS);
    extend!(crate::net::catalog::MODELS);
    extend!(crate::nvram::catalog::MODELS);
    extend!(crate::rtc::catalog::MODELS);
    extend!(crate::sd::catalog::MODELS);
    extend!(crate::ssi::catalog::MODELS);
    extend!(crate::timer::catalog::MODELS);
    extend!(crate::usb::catalog::MODELS);
    extend!(crate::virtio::catalog::MODELS);
    all
}

/// The [`DeviceFactory`] face of one catalog entry: the spawn function plus
/// the entry's vendor output table, so `clk` edges resolve through the entry
/// (a bare fn pointer would lose the outputs).
struct EntryFactory {
    entry: &'static ModelEntry,
}

impl mosaic_assemble::DeviceFactory for EntryFactory {
    fn instantiate(
        &self,
        ctx: &InstantiateCtx,
        machine: &mut Machine,
    ) -> Result<(), MaterializeError> {
        (self.entry.spawn)(ctx, machine)
    }

    fn output_signal(&self, port: &str) -> Option<SignalId> {
        self.entry.output_signal(port)
    }
}

/// A ready-made registry of every catalog entry (QEMU's `MODULE_INIT`
/// aggregate). A machine merges board-unique factories on top only when a
/// board truly has one.
pub fn device_registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    for entry in entries() {
        let _ = registry.register(entry.id, EntryFactory { entry });
    }
    registry
}

/// The `--catalog` listing: `id  summary`, one model per line, grouped by
/// function directory the way `qemu -device help` groups by category.
pub fn catalog_listing() -> Vec<(String, &'static str, &'static str)> {
    entries()
        .into_iter()
        .map(|entry| {
            let function = entry
                .id
                .split_once('/')
                .map(|(namespace, _)| namespace)
                .unwrap_or(entry.id);
            (function.to_owned(), entry.id, entry.summary)
        })
        .collect()
}

/// Count of registered models, for tests and the front-end's summary.
pub fn model_count() -> usize {
    entries().len()
}

/// The empty host-attachment set for callers that attach none.
pub fn no_hosts() -> HostAttachments {
    HostAttachments::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every catalog ID is unique: a duplicate registration is a bug that
    /// would silently drop one model's factory.
    #[test]
    fn catalog_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries() {
            assert!(seen.insert(entry.id), "duplicate catalog id {}", entry.id);
        }
    }

    /// Every catalog ID is a two-part namespaced identifier (`ip:vendor/model`),
    /// the manifest vocabulary: a bare name is a naming bug.
    #[test]
    fn catalog_ids_are_namespaced() {
        for entry in entries() {
            let parts: Vec<&str> = entry.id.split('/').collect();
            assert!(
                parts.len() == 2 && parts[0].contains(':') && !parts[1].is_empty(),
                "catalog id {} is not ip:<vendor>/<model>",
                entry.id
            );
        }
    }

    /// The vendor output tables' port names are unique per entry and
    /// kebab-case: they are port names a manifest spells in `clk` edges.
    #[test]
    fn output_port_names_are_unique_and_kebab() {
        for entry in entries() {
            let mut seen = std::collections::BTreeSet::new();
            for (port, _) in entry.outputs {
                assert!(
                    port.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "output port {port:?} of {} is not kebab-case",
                    entry.id
                );
                assert!(seen.insert(*port), "duplicate output port {port:?}");
            }
        }
    }

    #[test]
    fn the_registry_builds_and_lists_every_model() {
        let listing = catalog_listing();
        assert_eq!(listing.len(), model_count());
        assert!(model_count() > 20);
    }
}
