//! Display devices: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::display::vga::{VgaMemory, VgaRegisters, VgaVbe};

/// IBM VGA + VESA Bochs DISPI: the register file, VRAM and VBE the machine
/// wires (ports, aperture, LFB). The model itself holds no addresses.
fn spawn_vga(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let model = VgaModel::default();
    machine.register_device(ctx.node, Box::new(model))?;
    Ok(())
}

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

use mosaic_exec::Attr;

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:ibm/vga",
    summary: "IBM VGA register model + VESA Bochs DISPI (VBE) linear framebuffer",
    outputs: &[],
    spawn: spawn_vga,
}];
