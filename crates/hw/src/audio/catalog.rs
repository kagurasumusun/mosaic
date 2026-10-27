//! Catalog entries for `audio` devices (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_exec::SignalId;
use mosaic_runtime::Machine;

use crate::audio::ac97::Ac97;
use crate::audio::ad1980::Ad1980;
use crate::audio::alc1220::Alc1220;
use crate::audio::alc889::Alc889;
use crate::audio::alc892::Alc892;
use crate::audio::cs4281::Cs4281;
use crate::audio::es1370::Es1370;
use crate::audio::es1371::Es1371;
use crate::audio::hda::Hda;
use crate::audio::live::Live;
use crate::audio::opl3::Opl3;
use crate::audio::pl041::Pl041;
use crate::audio::sb128::Sb128;
use crate::audio::sb16::Sb16;
use crate::audio::sbawe32::Sbawe32;
use crate::catalog::ModelEntry;

/// Default spawn for hda: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_hda(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Hda::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ac97: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ac97(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ac97::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pl041: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pl041(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pl041::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for cs4281: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_cs4281(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Cs4281::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sb16: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sb16(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Sb16::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sbawe32: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sbawe32(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Sbawe32::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sb128: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sb128(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Sb128::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for live: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_live(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Live::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for opl3: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_opl3(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Opl3::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for es1370: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_es1370(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Es1370::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for es1371: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_es1371(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Es1371::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ad1980: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ad1980(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ad1980::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for alc889: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_alc889(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Alc889::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for alc892: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_alc892(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Alc892::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for alc1220: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_alc1220(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Alc1220::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:intel/hda",
        summary: "Intel High Definition Audio (HDA) controller",
        outputs: &[],
        spawn: spawn_hda,
    },
    ModelEntry {
        id: "ip:intel/ac97",
        summary: "Intel AC\'97 audio controller",
        outputs: &[],
        spawn: spawn_ac97,
    },
    ModelEntry {
        id: "ip:arm/pl041",
        summary: "ARM PrimeCell AACI (PL041) audio",
        outputs: &[],
        spawn: spawn_pl041,
    },
    ModelEntry {
        id: "ip:cirrus/cs4281",
        summary: "Cirrus CS4281 audio controller (PCI)",
        outputs: &[],
        spawn: spawn_cs4281,
    },
    ModelEntry {
        id: "ip:creative/sb16",
        summary: "Creative Sound Blaster 16",
        outputs: &[],
        spawn: spawn_sb16,
    },
    ModelEntry {
        id: "ip:creative/sbawe32",
        summary: "Creative Sound Blaster AWE32",
        outputs: &[],
        spawn: spawn_sbawe32,
    },
    ModelEntry {
        id: "ip:creative/sb128",
        summary: "Creative Sound Blaster PCI128",
        outputs: &[],
        spawn: spawn_sb128,
    },
    ModelEntry {
        id: "ip:creative/live",
        summary: "Creative Sound Blaster Live! (EMU10K1)",
        outputs: &[],
        spawn: spawn_live,
    },
    ModelEntry {
        id: "ip:yamaha/opl3",
        summary: "Yamaha OPL3 FM synthesizer",
        outputs: &[],
        spawn: spawn_opl3,
    },
    ModelEntry {
        id: "ip:ess/es1370",
        summary: "ESS ES1370 audio controller",
        outputs: &[],
        spawn: spawn_es1370,
    },
    ModelEntry {
        id: "ip:ess/es1371",
        summary: "ESS ES1371 audio controller",
        outputs: &[],
        spawn: spawn_es1371,
    },
    ModelEntry {
        id: "ip:adi/ad1980",
        summary: "ADI AD1980 AC\'97 codec",
        outputs: &[],
        spawn: spawn_ad1980,
    },
    ModelEntry {
        id: "ip:realtek/alc889",
        summary: "Realtek ALC889 HD audio codec",
        outputs: &[],
        spawn: spawn_alc889,
    },
    ModelEntry {
        id: "ip:realtek/alc892",
        summary: "Realtek ALC892 HD audio codec",
        outputs: &[],
        spawn: spawn_alc892,
    },
    ModelEntry {
        id: "ip:realtek/alc1220",
        summary: "Realtek ALC1220 HD audio codec",
        outputs: &[],
        spawn: spawn_alc1220,
    },
];
