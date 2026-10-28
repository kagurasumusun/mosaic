//! GPIO/IO expander parts (I2C, SPI, 1-Wire): the MCP23xxx, PCA/PCF/TCA
//! families and their shared cores.
//!
//! The MCP23017 is the multi-face exemplar (ADR 0032): one model whose
//! spawn exposes its I2C target face, its 16 GPIO line faces and its `int`
//! IRQ output — a board wires any subset through `[[part.face]]`.

use mosaic_assemble::{ConfigValue, InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::part::expander::mcp23017::{Mcp23017Shared, PINS};

/// MCP23017: the `i2c` face answers at the wired address, the `gpio` faces
/// expose the 16 lines, the `int` face is the interrupt output (emitted
/// under the part's own node origin).
fn spawn_mcp23017(ctx: &InstantiateCtx, _machine: &mut Machine) -> Result<(), MaterializeError> {
    let address = ctx
        .face_params("i2c")
        .and_then(|params| params.get("address"))
        .and_then(ConfigValue::as_integer)
        .unwrap_or(0x20) as u8;
    let int = ctx.output("int").ok();
    let shared = std::sync::Arc::new(Mcp23017Shared::new(ctx.node, address, int).map_err(
        |error| MaterializeError::Bind {
            node: ctx.graph_id.to_owned(),
            detail: format!("mcp23017 strap address {address:#x}: {error:?}"),
        },
    )?);
    ctx.expose_i2c_target("i2c", Box::new(shared.i2c_face()));
    for pin in 0..PINS {
        ctx.expose_gpio_line("gpio", pin as u64, Box::new(shared.line_face(pin)));
    }
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "part:microchip/mcp23017",
    summary: "MCP23017 16-bit I2C I/O expander (i2c face + 16 gpio line faces + int)",
    outputs: &[],
    spawn: spawn_mcp23017,
}];
