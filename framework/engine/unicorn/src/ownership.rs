//! The sole unsafe declaration in this adapter. See ADR 0007 before changing it.

use unicorn_engine::Unicorn;

#[derive(Default)]
pub(super) struct Counters {
    pub(super) instructions: u64,
}

pub(super) struct OwnedUnicorn(pub(super) Unicorn<'static, Counters>);

// SAFETY: Unicorn 2.1.5 uses Rc<UnsafeCell<_>> and is conservatively !Send. This
// wrapper owns the only escaping strong reference. No API exposes, clones or
// lends the raw Unicorn/Context/handle, and user hooks cannot be installed.
// Our sole hook is capture-free and only touches integer Counters. Temporary
// handles created inside an upstream callback are dropped before emu_start
// returns. Every operation completes before &mut self can be moved; Unicorn's
// native engine may be used sequentially on different threads. The type stays
// !Sync. Pinning the dependency makes upstream ownership changes a review point.
#[allow(unsafe_code)]
unsafe impl Send for OwnedUnicorn {}
