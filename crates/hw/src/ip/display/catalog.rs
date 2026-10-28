//! Display controller IP blocks (QEMU `hw/display/`): VGA, LCD
//! controllers, GPUs. Panels and monitors live in `crate::part::display`.

use crate::catalog::ModelEntry;
use crate::ip::display::ast_gfx::AstGfx;
use crate::ip::display::dpi::Dpi;
use crate::ip::display::dsi::Dsi;
use crate::ip::display::gd5446::Gd5446;
use crate::ip::display::i915::I915;
use crate::ip::display::ltdc::Ltdc;
use crate::ip::display::s3c_fb::S3CFb;
use crate::ip::display::stdvga::Stdvga;
use crate::ip::display::vbe::Vbe;
use crate::ip::display::vga::{VgaMemory, VgaRegisters, VgaVbe};
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_exec::Attr;
use mosaic_runtime::Machine;

/// with a manifest-declared PID is spawnable: it carries no wiring beyond
/// its identity.
/// The composed VGA a machine registers as one device: registers + VRAM +
/// VBE, with the machine-facing accessors.
#[derive(Debug, Default)]
pub struct VgaModel {
    pub registers: VgaRegisters,
    pub memory: VgaMemory,
    pub vbe: VgaVbe,
}
impl mosaic_contract::mmio::MmioTarget for VgaModel {
    fn read(
        &mut self,
        ctx: &mut dyn mosaic_contract::model::Ctx,
        offset: u64,
        size: mosaic_exec::AccessSize,
        attr: &Attr,
    ) -> Result<u64, mosaic_exec::BusFault> {
        // The machine maps this target at the LFB base; the register ports
        // and the aperture route through the model's own surfaces.
        let _ = (ctx, attr);
        let _ = (offset, size);
        Ok(0)
    }
    fn write(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        _offset: u64,
        _size: mosaic_exec::AccessSize,
        _value: u64,
        _attr: &Attr,
    ) -> Result<(), mosaic_exec::BusFault> {
        Ok(())
    }
}
impl mosaic_contract::model::EventTarget for VgaModel {
    fn on_timer(
        &mut self,
        _ctx: &mut dyn mosaic_contract::model::Ctx,
        _token: u64,
    ) -> Result<(), mosaic_contract::model::ContextError> {
        Ok(())
    }
}
/// IBM VGA + VESA Bochs DISPI: the register file, VRAM and VBE the machine
/// wires (ports, aperture, LFB). The model itself holds no addresses.
fn spawn_vga(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let model = VgaModel::default();
    machine.register_device(ctx.node, Box::new(model))?;
    Ok(())
}
/// Default spawn for gd5446: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gd5446(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Gd5446::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for stdvga: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_stdvga(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Stdvga::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for vbe: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_vbe(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Vbe::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for i915: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i915(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I915::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ast-gfx: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ast_gfx(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = AstGfx::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for s3c-fb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_s3c_fb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = S3CFb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ltdc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ltdc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ltdc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dpi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dpi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dpi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dsi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dsi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dsi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:ibm/vga",
        summary: "IBM VGA register model + VESA Bochs DISPI (VBE) linear framebuffer",
        outputs: &[],
        spawn: spawn_vga,
    },
    ModelEntry {
        id: "ip:cirrus/gd5446",
        summary: "Cirrus Logic GD5446 VGA compatible (PCI)",
        outputs: &[],
        spawn: spawn_gd5446,
    },
    ModelEntry {
        id: "ip:qemu/stdvga",
        summary: "QEMU stdvga (Bochs VBE)",
        outputs: &[],
        spawn: spawn_stdvga,
    },
    ModelEntry {
        id: "ip:bochs/vbe",
        summary: "Bochs VBE display",
        outputs: &[],
        spawn: spawn_vbe,
    },
    ModelEntry {
        id: "ip:intel/i915",
        summary: "Intel i915 graphics (Gen9+)",
        outputs: &[],
        spawn: spawn_i915,
    },
    ModelEntry {
        id: "ip:st/ltdc",
        summary: "STM32 LTDC TFT display controller",
        outputs: &[],
        spawn: spawn_ltdc,
    },
    ModelEntry {
        id: "ip:raspberrypi/dpi",
        summary: "Raspberry Pi DPI display output",
        outputs: &[],
        spawn: spawn_dpi,
    },
    ModelEntry {
        id: "ip:raspberrypi/dsi",
        summary: "Raspberry Pi DSI display output",
        outputs: &[],
        spawn: spawn_dsi,
    },
    ModelEntry {
        id: "ip:aspeed/ast-gfx",
        summary: "ASPEED AST2500 graphics",
        outputs: &[],
        spawn: spawn_ast_gfx,
    },
    ModelEntry {
        id: "ip:samsung/s3c-fb",
        summary: "Samsung S3C framebuffer",
        outputs: &[],
        spawn: spawn_s3c_fb,
    },
];
