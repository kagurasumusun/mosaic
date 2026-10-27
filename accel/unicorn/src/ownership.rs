//! The sole unsafe declaration in this adapter. See ADR 0007 before changing it.

use unicorn_engine::Unicorn;

use crate::EngineData;

#[derive(Default)]
pub(super) struct Counters {
    pub(super) instructions: u64,
}

pub(super) struct OwnedUnicorn(pub(super) Unicorn<'static, EngineData>);

// SAFETY: Unicorn 2.1.5 uses Rc<UnsafeCell<_>> and is conservatively !Send. This
// wrapper owns the only escaping strong reference. No API exposes, clones or
// lends the raw Unicorn/Context/handle, and user hooks cannot be installed.
// The installed MMIO closures capture only a Copy region base address; together
// with the code hook they touch only integer Counters, the bus port recorded
// in EngineData, or emu_stop(), and the bus port is itself Send by contract.
// Temporary handles created inside an upstream callback are dropped before
// emu_start returns. Every operation completes before &mut self can be moved;
// Unicorn's native engine may be used sequentially on different threads. The
// type stays !Sync. Pinning the dependency makes upstream ownership changes a
// review point.
#[allow(unsafe_code)]
unsafe impl Send for OwnedUnicorn {}
