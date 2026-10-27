//! Device-local offsets only; wiring and address translation belong to runtime.

use crate::model::Ctx;
use mosaic_exec::{AccessSize, Attr, BusFault};

pub trait MmioTarget: Send {
    /// Debug reads (Attr::kind == Debug) must not consume data or clear status.
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault>;

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault>;
}
