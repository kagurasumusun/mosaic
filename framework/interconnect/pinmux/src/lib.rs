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
}

struct MuxInner {
    /// `selection[pin] = Some(function)` or `None` while disconnected.
    selection: Vec<Option<usize>>,
    functions: Vec<Option<Box<dyn GpioPort>>>,
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
        inner.functions[function] = Some(port);
        Ok(())
    }

    /// Route `pin` to `function`, or disconnect with [`None`].
    pub fn select(
        &mut self,
        pin: usize,
        function: Option<usize>,
    ) -> Result<(), PinMuxError> {
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
        Ok(
    )

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

/// [`GpioPort`] of one muxed pin. Drives while disconnected are no-ops.
pub struct MuxedPin {
    inner: Arc<Mutex<MuxInner>>,
    pin: usize,
}

impl GpioPort for MuxedPin {
    fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
        let mut inner = lock(&self.inner);
        let Some(function) = inner.selection[self.pin] else {
            return Ok(());
        };
        let Some(port) = inner.functions[function].as_mut() else {
            return Err(GpioError::Disconnected);
        };
        port.drive(ctx, level)
    }

    fn configure(&mut self, config: PinConfig) -> Result<(), GpioError> {
        let mut inner = lock(&self.inner);
        let Some(function) = inner.selection[self.pin] else {
            return Ok(());
        };
        let Some(port) = inner.functions[function].as_mut() else {
            return Err(GpioError::Disconnected);
        };
        port.configure(config)
    }

    fn current_level(&self) -> PinLevel {
        let inner = lock(&self.inner);
        match inner.selection[self.pin] {
            Some(function) => match inner.functions[function].as_ref() {
                Some(port) => port.current_level(),
                None => PinLevel::Low,
            },
            None => PinLevel::Low,
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
    fn geometry_errors_are_bounds_checked() {
        let mut mux = PinMux::new(1, 1);
        assert_eq!(
            mux.pin_port(4).err(),
            Some(PinMuxError::PinOutOfRange)
        );
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
        fn configure(&mut self, _config: PinConfig) -> Result<(), GpioError> {
            Ok(())
        }
        fn current_level(&self) -> PinLevel {
            lock(&self.probe).current_level()
        }
    }
}
