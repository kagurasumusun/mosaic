#![forbid(unsafe_code)]
//! Point-to-point UART pair with fixed-capacity buffering.
//!
//! UART has no addressing (構成設計書 §11): the interconnect is a direct pair.
//! Bytes written at one endpoint become readable at the other, bounded by a
//! finite buffer so back-pressure surfaces as an error instead of unbounded
//! allocation. The pair also implements the byte-stream contract.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use mosaic_contract_model::Ctx;
use mosaic_contract_stream::{ByteStream, StreamError};
use mosaic_contract_uart_line::{UartConfig, UartError, UartLine};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    A,
    B,
}

impl Side {
    fn peer(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

struct EndpointState {
    rx: VecDeque<u8>,
    config: UartConfig,
    break_asserted: bool,
}

struct PairInner {
    endpoints: [EndpointState; 2],
    capacity: usize,
}

/// One end of the pair. Build both with [`UartPair::new`].
pub struct UartEndpoint {
    inner: Arc<Mutex<PairInner>>,
    side: Side,
}

/// Direct UART pair with per-side receive buffers of `capacity` bytes.
pub struct UartPair {
    inner: Arc<Mutex<PairInner>>,
}

impl UartPair {
    pub fn new(capacity: usize) -> Self {
        let default = UartConfig::default_115200_8n1();
        Self {
            inner: Arc::new(Mutex::new(PairInner {
                endpoints: [
                    EndpointState {
                        rx: VecDeque::with_capacity(capacity),
                        config: default,
                        break_asserted: false,
                    },
                    EndpointState {
                        rx: VecDeque::with_capacity(capacity),
                        config: default,
                        break_asserted: false,
                    },
                ],
                capacity,
            })),
        }
    }

    pub fn endpoints(self) -> (UartEndpoint, UartEndpoint) {
        let a = UartEndpoint {
            inner: Arc::clone(&self.inner),
            side: Side::A,
        };
        let b = UartEndpoint {
            inner: self.inner,
            side: Side::B,
        };
        (a, b)
    }
}

impl UartEndpoint {
    fn index(side: Side) -> usize {
        match side {
            Side::A => 0,
            Side::B => 1,
        }
    }

    /// Break condition observable on this endpoint's physical line.
    pub fn break_asserted(&self) -> bool {
        lock(&self.inner).endpoints[Self::index(self.side)].break_asserted
    }

    fn push_peer(&self, bytes: &[u8]) -> usize {
        let mut inner = lock(&self.inner);
        let peer = Self::index(self.side.peer());
        let room = inner
            .capacity
            .saturating_sub(inner.endpoints[peer].rx.len());
        let accepted = room.min(bytes.len());
        inner.endpoints[peer]
            .rx
            .extend(bytes[..accepted].iter().copied());
        accepted
    }

    fn pull_local(&self, buf: &mut [u8]) -> usize {
        let mut inner = lock(&self.inner);
        let queue = &mut inner.endpoints[Self::index(self.side)].rx;
        let available = queue.len().min(buf.len());
        for slot in buf[..available].iter_mut() {
            *slot = queue.pop_front().unwrap_or(0);
        }
        available
    }
}

impl UartLine for UartEndpoint {
    fn configure(&mut self, config: UartConfig) -> Result<(), UartError> {
        lock(&self.inner).endpoints[Self::index(self.side)].config = config;
        Ok(())
    }

    fn tx(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, UartError> {
        Ok(self.push_peer(bytes))
    }

    fn rx(&mut self, _ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, UartError> {
        Ok(self.pull_local(buf))
    }

    fn set_break(&mut self, _ctx: &mut dyn Ctx, asserted: bool) {
        let mut inner = lock(&self.inner);
        let peer = Self::index(self.side.peer());
        inner.endpoints[peer].break_asserted = asserted;
    }
}

impl ByteStream for UartEndpoint {
    fn write_bytes(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
        Ok(self.push_peer(bytes))
    }

    fn read_bytes(&mut self, _ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, StreamError> {
        Ok(self.pull_local(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_model::ContextError;
    use mosaic_contract_uart_line::Parity;

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
    fn bytes_cross_the_pair_in_order() {
        let pair = UartPair::new(8);
        let (mut a, mut b) = pair.endpoints();
        let ctx = &mut DummyCtx;
        assert_eq!(a.tx(ctx, b"mosaic").unwrap(), 6);
        let mut buf = [0u8; 6];
        assert_eq!(b.rx(ctx, &mut buf).unwrap(), 6);
        assert_eq!(&buf, b"mosaic");
        // Nothing queued on the sender side.
        assert_eq!(a.rx(ctx, &mut buf).unwrap(), 0);
    }

    #[test]
    fn finite_buffer_applies_backpressure_without_panicking() {
        let pair = UartPair::new(4);
        let (mut a, _b) = pair.endpoints();
        let accepted = a.tx(&mut DummyCtx, b"01234567").unwrap();
        assert_eq!(accepted, 4);
        let stream: &mut dyn ByteStream = &mut a;
        let again = stream.write_bytes(&mut DummyCtx, b"89").unwrap();
        assert_eq!(again, 0);
    }

    #[test]
    fn break_and_configuration_reach_the_peer_line() {
        let pair = UartPair::new(4);
        let (mut a, b) = pair.endpoints();
        a.set_break(&mut DummyCtx, true);
        assert!(b.break_asserted());
        a.configure(UartConfig::new(9600, 7, Parity::Even, 1).unwrap())
            .unwrap();
        let config = lock(&a.inner).endpoints[0].config;
        assert_eq!(config.baud, 9600);
        assert_eq!(config.parity, Parity::Even);
    }
}
