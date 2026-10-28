//! Sensor parts: chips that measure physical quantities (temperature,
//! humidity, pressure, motion, light, cameras) plus the hardware monitors
//! (ADM/LM/MAX monitor families, ADR 0031 §3). Board wiring picks the bus
//! address; the fabric is the reuse.
//!
//! Two exemplars are spawn-registered (ADR 0032):
//!
//! * **BME280** — the multi-transport part: one model exposing both an
//!   `i2c` and an `spi` target face; a board wires exactly one of them
//!   (the SDO strap picks one on real hardware).
//! * **LM75A** — a sensor with a thermostat output: an `i2c` target face
//!   plus an `os` IRQ face the board may wire.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{ConfigValue, InstantiateCtx, MaterializeError};
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::part::sensor::bme280::i2c::{Bme280I2c, Bme280I2cParams};
use crate::part::sensor::bme280::spi::Bme280Spi;
use crate::part::sensor::lm75a::{Lm75a, Lm75aParams};

/// The sensor behind its I2C target face.
struct SharedI2c<T>(Arc<Mutex<T>>);

impl<T> SharedI2c<T>
where
    T: I2cTarget,
{
    fn new(shared: Arc<Mutex<T>>) -> Self {
        Self(shared)
    }
}

impl<T> I2cTarget for SharedI2c<T>
where
    T: I2cTarget,
{
    fn matches(&self, addr: &I2cAddr) -> bool {
        self.0.lock().unwrap().matches(addr)
    }
    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        self.0.lock().unwrap().target_write(ctx, addr, bytes)
    }
    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        self.0.lock().unwrap().target_read(ctx, addr, buf)
    }
}

/// The face-wired address of `port`, or `default` when unwired.
fn face_address(ctx: &InstantiateCtx, port: &str, default: u8) -> u8 {
    ctx.face_params(port)
        .and_then(|params| params.get("address"))
        .and_then(ConfigValue::as_integer)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(default)
}

/// BME280: both transport faces on one model — the board wires `i2c` or
/// `spi`, never both (the SDO strap picks one on real hardware).
fn spawn_bme280(ctx: &InstantiateCtx, _machine: &mut Machine) -> Result<(), MaterializeError> {
    let address = face_address(ctx, "i2c", 0x76);
    let shared = Arc::new(Mutex::new(Bme280I2c::new(Bme280I2cParams::new(
        I2cAddr::seven(address).map_err(|error| MaterializeError::Bind {
            node: ctx.graph_id.to_owned(),
            detail: format!("bme280 address {address:#x}: {error:?}"),
        })?,
    ))));
    ctx.expose_i2c_target("i2c", Box::new(SharedI2c::new(Arc::clone(&shared))));
    // The SPI bind is its own SpiTarget over the same core; only the face
    // the board wires ever attaches to a segment.
    ctx.expose_spi_target("spi", 0, Box::new(Bme280Spi::new()));
    Ok(())
}

/// LM75A: the `i2c` face at the wired strap address; the `os` thermostat
/// output is the IRQ face, when the board wired it.
fn spawn_lm75a(ctx: &InstantiateCtx, _machine: &mut Machine) -> Result<(), MaterializeError> {
    let address = face_address(ctx, "i2c", 0x48);
    let mut params =
        Lm75aParams::new(
            I2cAddr::seven(address).map_err(|error| MaterializeError::Bind {
                node: ctx.graph_id.to_owned(),
                detail: format!("lm75a address {address:#x}: {error:?}"),
            })?,
        );
    if let Ok(os) = ctx.output("os") {
        params = params.with_os(os);
    }
    ctx.expose_i2c_target(
        "i2c",
        Box::new(SharedI2c::new(Arc::new(Mutex::new(Lm75a::new(params))))),
    );
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "part:bosch/bme280",
        summary: "BME280 humidity/pressure/temperature sensor (i2c and spi faces, wire one)",
        outputs: &[],
        spawn: spawn_bme280,
    },
    ModelEntry {
        id: "part:nxp/lm75a",
        summary: "LM75A digital temperature sensor and thermal watchdog (i2c face + os output)",
        outputs: &[],
        spawn: spawn_lm75a,
    },
];
