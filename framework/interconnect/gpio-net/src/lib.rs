#![forbid(unsafe_code)]
//! Deterministic digital net resolution for GPIO pins.
//!
//! The net owns the electrical rules from 構成設計書 §13.3: pull resistors,
//! open-drain release (tristate) and collision detection. Models never resolve
//! levels themselves; they drive [`mosaic_contract_gpio::GpioPort`] adapters and
//! observe results through [`mosaic_contract_gpio::GpioSink`] callbacks.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_contract_gpio::{
    DriveMode, GpioError, GpioPort, GpioSink, PinConfig, PinDirection, PinLevel, Pull,
};
use mosaic_contract_model::Ctx;
use mosaic_contract_net::{DigitalNet, NetDrive, NetLevel, NetNode, NetPull};

/// Recover from a poisoned mutex without panicking: a poisoned guard still
/// holds the last consistent state, and every callback leaves state valid.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Clone, Copy)]
struct NodeState {
    /// Physical pull resistor attached to the net by the board composition.
    pull: NetPull,
    /// The port's internal pull configured by its GPIO model.
    pin_pull: NetPull,
    drive: NetDrive,
}

struct NetInner {
    nodes: BTreeMap<u32, NodeState>,
    // None means this sink is temporarily executing outside the net lock.
    // The key remains reserved so a concurrent attach cannot replace it.
    sinks: BTreeMap<u32, Option<Box<dyn GpioSink>>>,
    next: u64,
    resolved: NetLevel,
    pending: VecDeque<(u32, PinLevel)>,
    notifying: bool,
}

/// Concrete digital net. Output adapters created through [`GpioNet::port`]
/// share the net, keeping one single-threaded ownership group.
pub struct GpioNet {
    inner: Arc<Mutex<NetInner>>,
}

impl Default for GpioNet {
    fn default() -> Self {
        Self::new()
    }
}

impl GpioNet {
    pub fn new() -> Self {
        let inner = NetInner {
            nodes: BTreeMap::new(),
            sinks: BTreeMap::new(),
            next: 0,
            resolved: NetLevel::new(PinLevel::High, false),
            pending: VecDeque::new(),
            notifying: false,
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Begin notifying `node` whenever the resolved level changes.
    pub fn attach_sink(&mut self, node: NetNode, sink: Box<dyn GpioSink>) -> Result<(), GpioError> {
        let mut inner = lock(&self.inner);
        if !inner.nodes.contains_key(&node.0) {
            return Err(GpioError::Disconnected);
        }
        if inner.sinks.contains_key(&node.0) {
            return Err(GpioError::AlreadyConnected);
        }
        inner.sinks.insert(node.0, Some(sink));
        Ok(())
    }

    /// Output adapter bound to one previously attached node.
    pub fn port(&self, node: NetNode, config: PinConfig) -> GpioNetPort {
        GpioNetPort {
            inner: Arc::clone(&self.inner),
            node,
            config,
        }
    }

    fn resolve(inner: &NetInner) -> NetLevel {
        let mut strong_low = false;
        let mut strong_high = false;
        let mut pull_up = false;
        let mut pull_down = false;
        for state in inner.nodes.values() {
            match state.drive {
                NetDrive::StrongLow | NetDrive::OpenDrainLow => strong_low = true,
                NetDrive::StrongHigh => strong_high = true,
                // HighZ and any future drive variant contribute no level.
                _ => {}
            }
            for pull in [state.pull, state.pin_pull] {
                match pull {
                    NetPull::Up => pull_up = true,
                    NetPull::Down => pull_down = true,
                    // None and any future pull variant leave the net unpulled.
                    _ => {}
                }
            }
        }
        // Deterministic tie-breaks keep simulations reproducible: on an
        // electrical conflict low wins, and an unpulled floating net idles high.
        let collision = (strong_low && strong_high) || (pull_up && pull_down);
        let level = if strong_low {
            PinLevel::Low
        } else if strong_high {
            PinLevel::High
        } else if pull_down && !pull_up {
            PinLevel::Low
        } else {
            PinLevel::High
        };
        NetLevel::new(level, collision)
    }

    /// Apply a drive change, re-resolve and return the sinks to notify.
    /// Notifications run outside the calling lock to avoid re-entrant locks.
    fn apply(
        inner: &mut NetInner,
        node: NetNode,
        drive: NetDrive,
        pin_pull: Option<NetPull>,
    ) -> Result<Vec<(u32, PinLevel)>, GpioError> {
        let state = inner
            .nodes
            .get_mut(&node.0)
            .ok_or(GpioError::Disconnected)?;
        state.drive = drive;
        if let Some(pin_pull) = pin_pull {
            state.pin_pull = pin_pull;
        }
        let resolved = Self::resolve(inner);
        let previous = inner.resolved;
        inner.resolved = resolved;
        if previous.level == resolved.level {
            return Ok(Vec::new());
        }
        Ok(inner
            .sinks
            .keys()
            .map(|node| (*node, resolved.level))
            .collect())
    }

    fn notify(inner: &Arc<Mutex<NetInner>>, pending: Vec<(u32, PinLevel)>, ctx: &mut dyn Ctx) {
        // Queue notifications under the lock, but never call model code while
        // holding it. A callback may drive another port on this same net; its
        // newly generated edges are appended and drained after the current
        // callback returns instead of deadlocking or recursively re-entering.
        {
            let mut guard = lock(inner);
            guard.pending.extend(pending);
            if guard.notifying {
                return;
            }
            guard.notifying = true;
        }

        loop {
            let next = {
                let mut guard = lock(inner);
                let Some((node, level)) = guard.pending.pop_front() else {
                    guard.notifying = false;
                    return;
                };
                guard
                    .sinks
                    .get_mut(&node)
                    .and_then(Option::take)
                    .map(|sink| (node, level, sink))
            };

            let Some((node, level, mut sink)) = next else {
                // No sink was attached for this node. The resolved net level
                // remains available to any later port read.
                continue;
            };
            let result = catch_unwind(AssertUnwindSafe(|| sink.level_changed(ctx, level)));
            {
                let mut guard = lock(inner);
                if let Some(slot) = guard.sinks.get_mut(&node) {
                    // The map key remains present while the sink is out of
                    // the lock, and attach_sink rejects replacement of it.
                    *slot = Some(sink);
                }
                if result.is_err() {
                    // Keep notifications queued by the callback for a later
                    // call; do not leave the dispatcher permanently marked busy.
                    guard.notifying = false;
                }
            }
            if let Err(payload) = result {
                resume_unwind(payload);
            }
        }
    }
}

impl DigitalNet for GpioNet {
    fn attach(&mut self, pull: NetPull) -> NetNode {
        let mut inner = lock(&self.inner);
        let raw_node = u32::try_from(inner.next)
            .expect("GpioNet exhausted its u32 NetNode identifier space");
        let node = NetNode(raw_node);
        inner.next += 1;
        inner.nodes.insert(
            node.0,
            NodeState {
                pull,
                pin_pull: NetPull::None,
                drive: NetDrive::HighZ,
            },
        );
        inner.resolved = Self::resolve(&inner);
        node
    }

    fn drive(&mut self, ctx: &mut dyn Ctx, node: NetNode, drive: NetDrive) {
        let pending = {
            let mut inner = lock(&self.inner);
            Self::apply(&mut inner, node, drive, None).unwrap_or_default()
        };
        Self::notify(&self.inner, pending, ctx);
    }

    fn level(&self) -> NetLevel {
        lock(&self.inner).resolved
    }
}

/// [`GpioPort`] adapter for one net node. Open-drain outputs release the net
/// (high-Z) on high level instead of actively driving it.
pub struct GpioNetPort {
    inner: Arc<Mutex<NetInner>>,
    node: NetNode,
    config: PinConfig,
}

fn net_pull(pull: Pull) -> NetPull {
    match pull {
        Pull::Up => NetPull::Up,
        Pull::Down => NetPull::Down,
        Pull::None => NetPull::None,
        // Pull is non-exhaustive; unknown future values have no modeled effect.
        _ => NetPull::None,
    }
}

impl GpioPort for GpioNetPort {
    fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
        let drive = match (self.config.direction, self.config.drive, level) {
            // Input mode must release any prior output drive, even if a model
            // writes the GPIO data latch while the pin is configured as input.
            (PinDirection::Input, _, _) => NetDrive::HighZ,
            (PinDirection::Output, DriveMode::OpenDrain, PinLevel::High) => NetDrive::HighZ,
            (PinDirection::Output, _, PinLevel::Low) => NetDrive::StrongLow,
            (PinDirection::Output, _, PinLevel::High) => NetDrive::StrongHigh,
            // Public pin enums are non-exhaustive; reject future unknown values.
            _ => return Err(GpioError::Unsupported),
        };
        let pending = {
            let mut inner = lock(&self.inner);
            GpioNet::apply(
                &mut inner,
                self.node,
                drive,
                Some(net_pull(self.config.pull)),
            )?
        };
        GpioNet::notify(&self.inner, pending, ctx);
        Ok(())
    }

    fn release(&mut self, ctx: &mut dyn Ctx) -> Result<(), GpioError> {
        let pending = {
            let mut inner = lock(&self.inner);
            GpioNet::apply(
                &mut inner,
                self.node,
                NetDrive::HighZ,
                Some(net_pull(self.config.pull)),
            )?
        };
        GpioNet::notify(&self.inner, pending, ctx);
        Ok(())
    }

    fn configure(&mut self, config: PinConfig) -> Result<(), GpioError> {
        // Configuration alone has no Ctx to deliver net edges. Callers pair
        // it with drive() or release(), which atomically apply the new pull
        // state and drive mode and then notify sinks if the resolved level moved.
        self.config = config;
        Ok(())
    }

    fn current_level(&self) -> PinLevel {
        lock(&self.inner).resolved.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_model::ContextError;

    #[derive(Default)]
    struct RecordingSink {
        changes: Arc<Mutex<Vec<PinLevel>>>,
    }

    impl GpioSink for RecordingSink {
        fn level_changed(&mut self, _ctx: &mut dyn Ctx, level: PinLevel) {
            lock(&self.changes).push(level);
        }
    }

    struct DummyCtx;
    impl Ctx for DummyCtx {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _signal: mosaic_core_types::SignalId,
            _high: bool,
        ) -> Result<(), ContextError> {
            Ok(())
        }
    }

    #[test]
    fn pull_up_idles_high_and_strong_low_overrides_it() {
        let mut net = GpioNet::new();
        let node = net.attach(NetPull::Up);
        assert_eq!(net.level().level, PinLevel::High);
        net.drive(&mut DummyCtx, node, NetDrive::StrongLow);
        assert_eq!(net.level().level, PinLevel::Low);
        net.drive(&mut DummyCtx, node, NetDrive::HighZ);
        assert_eq!(net.level().level, PinLevel::High);
    }

    #[test]
    fn conflicting_strong_drivers_flag_a_deterministic_collision() {
        let mut net = GpioNet::new();
        let a = net.attach(NetPull::None);
        let b = net.attach(NetPull::None);
        net.drive(&mut DummyCtx, a, NetDrive::StrongHigh);
        net.drive(&mut DummyCtx, b, NetDrive::StrongLow);
        let level = net.level();
        assert!(level.collision);
        assert_eq!(level.level, PinLevel::Low);
    }

    #[test]
    fn pin_internal_pull_is_resolved_separately_from_board_pull() {
        let mut net = GpioNet::new();
        let node = net.attach(NetPull::None);
        let mut port = net.port(node, PinConfig::input(mosaic_contract_gpio::Pull::None));
        assert_eq!(net.level().level, PinLevel::High);

        port.configure(PinConfig::input(Pull::Down)).unwrap();
        port.release(&mut DummyCtx).unwrap();
        assert_eq!(net.level().level, PinLevel::Low);

        // The board has no pull-down, so changing the internal pull to up
        // changes the floating level without changing the external resistor.
        port.configure(PinConfig::input(Pull::Up)).unwrap();
        port.release(&mut DummyCtx).unwrap();
        assert_eq!(net.level().level, PinLevel::High);
    }

    #[test]
    fn open_drain_high_releases_the_net() {
        let mut net = GpioNet::new();
        let node = net.attach(NetPull::Up);
        let mut port = net.port(node, PinConfig::output(PinLevel::High));
        port.configure(PinConfig::new(
            mosaic_contract_gpio::PinDirection::Output,
            mosaic_contract_gpio::Pull::None,
            DriveMode::OpenDrain,
            PinLevel::High,
        ))
        .unwrap();
        port.drive(&mut DummyCtx, PinLevel::High).unwrap();
        assert_eq!(port.current_level(), PinLevel::High);
        port.drive(&mut DummyCtx, PinLevel::Low).unwrap();
        assert_eq!(port.current_level(), PinLevel::Low);
        port.drive(&mut DummyCtx, PinLevel::High).unwrap();
        assert_eq!(port.current_level(), PinLevel::High);
    }

    #[test]
    fn sinks_receive_only_resolved_edge_changes() {
        let mut net = GpioNet::new();
        let driver = net.attach(NetPull::None);
        let observer = net.attach(NetPull::Up);
        let sink = RecordingSink::default();
        let changes = Arc::clone(&sink.changes);
        net.attach_sink(observer, Box::new(sink)).unwrap();
        net.drive(&mut DummyCtx, driver, NetDrive::StrongLow);
        net.drive(&mut DummyCtx, driver, NetDrive::StrongLow);
        net.drive(&mut DummyCtx, driver, NetDrive::HighZ);
        assert_eq!(lock(&changes).as_slice(), &[PinLevel::Low, PinLevel::High]);
    }

    #[test]
    fn duplicate_sink_registration_is_rejected_without_replacing_the_original() {
        let mut net = GpioNet::new();
        let driver = net.attach(NetPull::None);
        let observer = net.attach(NetPull::Up);
        let first = RecordingSink::default();
        let first_changes = Arc::clone(&first.changes);
        net.attach_sink(observer, Box::new(first)).unwrap();
        let replacement = RecordingSink::default();
        assert_eq!(
            net.attach_sink(observer, Box::new(replacement)),
            Err(GpioError::AlreadyConnected)
        );
        let mut port = net.port(driver, PinConfig::output(PinLevel::Low));
        port.drive(&mut DummyCtx, PinLevel::Low).unwrap();
        assert_eq!(lock(&first_changes).as_slice(), &[PinLevel::Low]);
    }

    struct ReentrantSink {
        driver: GpioNetPort,
        changes: Arc<Mutex<Vec<PinLevel>>>,
    }

    impl GpioSink for ReentrantSink {
        fn level_changed(&mut self, ctx: &mut dyn Ctx, level: PinLevel) {
            lock(&self.changes).push(level);
            if level == PinLevel::Low {
                // Drive the same net while inside the notification callback.
                // This used to re-lock NetInner and deadlock.
                self.driver.drive(ctx, PinLevel::High).unwrap();
            }
        }
    }

    #[test]
    fn sink_can_reenter_the_same_net_and_notifications_are_drained_fifo() {
        let mut net = GpioNet::new();
        let driver = net.attach(NetPull::None);
        let observer = net.attach(NetPull::Up);
        let changes = Arc::new(Mutex::new(Vec::new()));
        let sink = ReentrantSink {
            driver: net.port(driver, PinConfig::output(PinLevel::High)),
            changes: Arc::clone(&changes),
        };
        net.attach_sink(observer, Box::new(sink)).unwrap();
        let mut port = net.port(driver, PinConfig::output(PinLevel::Low));
        port.drive(&mut DummyCtx, PinLevel::Low).unwrap();
        assert_eq!(lock(&changes).as_slice(), &[PinLevel::Low, PinLevel::High]);
        assert_eq!(net.level().level, PinLevel::High);
    }

    #[test]
    fn unknown_nodes_are_reported_not_panicked() {
        let net = GpioNet::new();
        let mut port = net.port(NetNode(99), PinConfig::output(PinLevel::Low));
        assert_eq!(
            port.drive(&mut DummyCtx, PinLevel::High),
            Err(GpioError::Disconnected)
        );
    }
}
