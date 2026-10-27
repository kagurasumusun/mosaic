//! SD/eMMC host controllers and media: catalog entries (ADR 0025). The card
/// attaches through the host attachment (bus membership rule).
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::sd::pl181::Pl181;
use crate::sd::sdhci::Sdhci;

/// SDHCI: `irq` output; the card arrives as the host attachment `host_sd()`.
fn spawn_sdhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut host = Sdhci::new(irq);
    if let Some(card) = ctx.host_sd() {
        host.attach_card(card);
    }
    machine.register_device(ctx.node, Box::new(host))?;
    Ok(())
}

/// PL181: `irq`/`fiq` outputs; the card arrives as the host attachment.
fn spawn_pl181(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let fiq = ctx.output("fiq")?;
    let mut host = Pl181::new(irq, fiq);
    if let Some(card) = ctx.host_sd() {
        host.attach_card(card);
    }
    machine.register_device(ctx.node, Box::new(host))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/pl181",
        summary: "ARM PL181 MultiMedia Card Interface (PrimeCell MCI host)",
        outputs: &[],
        spawn: spawn_pl181,
    },
    ModelEntry {
        id: "ip:sda/sdhci",
        summary: "SD Host Controller Interface v3 (SDHCI register set, PIO data port)",
        outputs: &[],
        spawn: spawn_sdhci,
    },
];
