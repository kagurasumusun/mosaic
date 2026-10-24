#![forbid(unsafe_code)]
//! Pin multiplexer routing GPIO pins to function-specific ports.
//!
//! SoC pinmux hardware selects, per physical pin, which function bank drives
//! it. The multiplexer is pure interconnect fabric: it holds no register map
//! (the owning IP or SoC glue owns those) and only forwards pin levels to the
//! currently selected downstream [`mosaic_contract_gpio::GpioPort`].

use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_contract_gpio::{GpioError, GpioPort, PinConfig, PinLevel};
use mosaic_contract_model::Ctx;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Construction errors for invalid multiplexer geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PinMuxError {
    PinOutOfRange,
    FunctionOutOfRange,
    FunctionAlreadyAttached,
    FunctionBusy,
    Downstream(GpioError),
}

struct MuxInner {
    /// `selection[pin] = Some(function)` or `None` while disconnected.
    selection: Vec<Option<usize>>,
    functions: Vec<Option<Box<dyn GpioPort>>>,
    /// A function port is temporarily leased while user code runs, so callbacks
    /// never execute while the mux lock is held.
    busy: Vec<bool>,
}

/// Fixed-geometry pin multiplexer: `pins` physical pins, `functions` banks.
pub struct PinMux {
    inner: Arc<Mutex<MuxInner>>,
    pins: usize,
}

impl PinMux {
    /// Every pin starts disconnected and every function bank is unattached.
    pub fn new(pins: usize, functions: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MuxInner {
                selection: vec![None; pins],
                functions: (0..functions).map(|_| None).collect(),
                busy: vec![false; functions],
            })),
            pins,
        }
    }

    /// Attach the downstream port implementing one function bank.
    pub fn attach_function(
        &mut self,
        function: usize,
        port: Box<dyn GpioPort>,
    ) -> Result<(), PinMuxError> {
        let mut inner = lock(&self.inner);
        if function >= inner.functions.len() {
            return Err(PinMuxError::FunctionOutOfRange);
        }
        if inner.busy[function] || inner.functions[function].is_some() {
            return Err(PinMuxError::FunctionAlreadyAttached);
        }
        inner.functions[function] = Some(port);
        Ok(())
    }

    /// Route `pin` to `function`, or disconnect with [`None`].
    ///
    /// This context-free form is for initial composition before guest execution.
    /// Runtime pin switching should use [`Self::select_with_ctx`] so the last
    /// pin using the previous function can release its electrical drive.
    pub fn select(&mut self, pin: usize, function: Option<usize>) -> Result<(), PinMuxError> {
        let mut inner = lock(&self.inner);
        if pin >= inner.selection.len() {
            return Err(PinMuxError::PinOutOfRange);
        }
        if let Some(index) = function {
            if index >= inner.functions.len() {
                return Err(PinMuxError::FunctionOutOfRange);
            }
        }
        inner.selection[pin] = function;
        Ok(())
    }

    /// Change a route during execution, releasing the old function's port when
    /// no other physical pin still selects it. The callback executes outside
    /// the mux lock; on failure the previous selection is retained.
    pub fn select_with_ctx(
        &mut self,
        ctx: &mut dyn Ctx,
        pin: usize,
        function: Option<usize>,
    ) -> Result<(), PinMuxError> {
        let plan = {
            let mut inner = lock(&self.inner);
            if pin >= inner.selection.len() {
                return Err(PinMuxError::PinOutOfRange);
            }
            if let Some(index) = function {
                if index >= inner.functions.len() {
                    return Err(PinMuxError::FunctionOutOfRange);
                }
                if inner.busy[index] {
                    return Err(PinMuxError::FunctionBusy);
                }
            }
            let previous = inner.selection[pin];
            if previous == function {
                return Ok(());
            }
            let Some(previous_function) = previous else {
                inner.selection[pin] = function;
                return Ok(());
            };
            let still_selected = inner
                .selection
                .iter()
                .enumerate()
                .any(|(other_pin, selected)| {
                    other_pin != pin && *selected == Some(previous_function)
                });
            if still_selected || inner.functions[previous_function].is_none() {
                inner.selection[pin] = function;
                return Ok(());
            }
            if inner.busy[previous_function] {
                return Err(PinMuxError::FunctionBusy);
            }
            let port = inner.functions[previous_function]
                .take()
                .ok_or(PinMuxError::FunctionBusy)?;
            inner.busy[previous_function] = true;
            // Temporarily disconnect this pin while downstream code runs.
            inner.selection[pin] = None;
            (previous_function, previous, port)
        };

        let (previous_function, previous_selection, mut port) = plan;
        let release = port.release(ctx);
        let mut inner = lock(&self.inner);
        inner.functions[previous_function] = Some(port);
        inner.busy[previous_function] = false;
        if let Err(error) = release {
            inner.selection[pin] = previous_selection;
            return Err(PinMuxError::Downstream(error));
        }
        inner.selection[pin] = function;
        Ok(())
    }

    /// Port a model drives; the multiplexer forwards to the selected function.
    pub fn pin_port(&self, pin: usize) -> Result<MuxedPin, PinMuxError> {
        if pin >= self.pins {
            return Err(PinMuxError::PinOutOfRange);
        }
        Ok(MuxedPin {
            inner: Arc::clone(&self.inner),
            pin,
        })
    }
}

/// Temporarily owns a function port while its callback runs outside the mux
/// lock. Drop restores it even if downstream code unwinds.
struct PortLease {
    inner: Arc<Mutex<MuxInner>>,
    function: usize,
    port: Option<Box<dyn GpioPort>>,
}

impl PortLease {
    fn port_mut(&mut self) -> &mut dyn GpioPort {
        self.port
            .as_deref_mut()
            .expect("port lease owns its function port until Drop")
    }
}

impl Drop for PortLease {
    fn drop(&mut self) {
        if let Some(port) = self.port.take() {
            let mut inner = lock(&self.inner);
            inner.functions[self.function] = Some(port);
            inner.busy[self.function] = false;
        }
    }
}

/// [`GpioPort`] of one muxed pin. Drives while disconnected are no-ops.
pub struct MuxedPin {
    inner: Arc<Mutex<MuxInner>>,
    pin: usize,
}

impl MuxedPin {
    fn lease_selected(&self) -> Result<Option<PortLease>, GpioError> {
        let mut inner = lock(&self.inner);
        let Some(function) = inner.selection[self.pin] else {
            return Ok(None);
        };
        if inner.busy[function] {
            return Err(GpioError::Reentrant);
        }
        let Some(port) = inner.functions[function].take() else {
            return Err(GpioError::Disconnected);
        };
        inner.busy[function] = true;
        Ok(Some(PortLease {
            inner: Arc::clone(&self.inner),
            function,
            port: Some(port),
        }))
    }
}

impl GpioPort for MuxedPin {
    fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
        let Some(mut lease) = self.lease_selected()? else {
            return Ok(());
        };
        lease.port_mut().drive(ctx, level)
    }

    fn release(&mut self, ctx: &mut dyn Ctx) -> Result<(), GpioError> {
        let Some(mut lease) = self.lease_selected()? else {
            return Ok(());
        };
        lease.port_mut().release(ctx)
    }

    fn configure(&mut self, config: PinConfig) -> Result<(), GpioError> {
        let Some(mut lease) = self.lease_selected()? else {
            return Ok(());
        };
        lease.port_mut().configure(config)
    }

    fn current_level(&self) -> PinLevel {
        match self.lease_selected() {
            Ok(Some(mut lease)) => lease.port_mut().current_level(),
            Ok(None) | Err(_) => PinLevel::Low,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_model::ContextError;

    struct LevelProbe {
        driven: Vec<PinLevel>,
        level: PinLevel,
    }

    impl LevelProbe {
        fn drive(&mut self, level: PinLevel) {
            self.driven.push(level);
            self.level = level;
        }

        fn current_level(&self) -> PinLevel {
            self.level
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
    fn drives_follow_the_selected_function() {
        let mut mux = PinMux::new(2, 2);
        let probe = Arc::new(Mutex::new(LevelProbe {
            driven: Vec::new(),
            level: PinLevel::Low,
        }));
        mux.attach_function(
            1,
            Box::new(ProbePort {
                probe: Arc::clone(&probe),
            }),
        )
        .unwrap();
        let mut pin = mux.pin_port(0).unwrap();

        // Disconnected: the drive is swallowed.
        pin.drive(&mut DummyCtx, PinLevel::High).unwrap();
        assert!(lock(&probe).driven.is_empty());

        // Select function 1: the same pin now reaches the probe.
        mux.select(0, Some(1)).unwrap();
        pin.drive(&mut DummyCtx, PinLevel::High).unwrap();
        pin.drive(&mut DummyCtx, PinLevel::Low).unwrap();
        assert_eq!(
            lock(&probe).driven.as_slice(),
            &[PinLevel::High, PinLevel::Low]
        );

        // Disconnect again and the pin goes quiet.
        mux.select(0, None).unwrap();
        pin.drive(&mut DummyCtx, PinLevel::High).unwrap();
        assert_eq!(lock(&probe).driven.len(), 2);
    }

    #[test]
    fn context_aware_switch_releases_the_last_selected_function() {
        let mut mux = PinMux::new(2, 2);
        let probe = Arc::new(Mutex::new(LevelProbe {
            driven: Vec::new(),
            level: PinLevel::High,
        }));
        mux.attach_function(
            1,
            Box::new(ProbePort {
                probe: Arc::clone(&probe),
            }),
        )
        .unwrap();
        mux.select(0, Some(1)).unwrap();
        assert_eq!(lock(&probe).level, PinLevel::High);
        mux.select_with_ctx(&mut DummyCtx, 0, None).unwrap();
        assert_eq!(lock(&probe).level, PinLevel::Low);
        // The route is now disconnected and further drives cannot reach it.
        let mut pin = mux.pin_port(0).unwrap();
        pin.drive(&mut DummyCtx, PinLevel::High).unwrap();
        assert!(lock(&probe).driven.is_empty());
    }

    #[test]
    fn geometry_errors_are_bounds_checked() {
        let mut mux = PinMux::new(1, 1);
        assert_eq!(mux.pin_port(4).err(), Some(PinMuxError::PinOutOfRange));
        assert_eq!(
            mux.select(0, Some(3)).err(),
            Some(PinMuxError::FunctionOutOfRange)
        );
    }

    /// Adapter so the test probe can live behind the port trait while the
    /// test retains an inspection handle.
    struct ProbePort {
        probe: Arc<Mutex<LevelProbe>>,
    }

    impl GpioPort for ProbePort {
        fn drive(&mut self, _ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
            lock(&self.probe).drive(level);
            Ok(())
        }
        fn release(&mut self, _ctx: &mut dyn Ctx) -> Result<(), GpioError> {
            lock(&self.probe).level = PinLevel::Low;
            Ok(())
        }
        fn configure(&mut self, _config: PinConfig) -> Result<(), GpioError> {
            Ok(())
        }
        fn current_level(&self) -> PinLevel {
            lock(&self.probe).current_level()
        }
    }
}
