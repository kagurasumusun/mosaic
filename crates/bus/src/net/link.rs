//! Ethernet link fabric: frame exchange between NIC ports of one segment.
//!
//! A link is a collision domain (a hub segment, or the loopback a
//! single-NIC simulation still needs): every frame one port transmits is
//! delivered to every *other* port that accepts it, by MAC address, in
//! transmit order. Delivery order is deterministic — that is the property
//! guest drivers test against.

use std::collections::VecDeque;

use mosaic_contract::ethernet::{accepts, EthError, EthernetPort};

/// A pending delivery: the transmitting port's index and the frame.
struct Pending {
    from: usize,
    frame: Vec<u8>,
}

/// One Ethernet segment.
pub struct EthernetLink {
    ports: Vec<Box<dyn EthernetPort>>,
    /// Deterministic, bounded delivery queue: one frame in flight per port
    /// ahead of it, so a flood of transmits never allocates unboundedly.
    pending: VecDeque<Pending>,
    capacity: usize,
    /// Frames each port dropped because its receive path was full, for the
    /// NIC models to report through their own counters.
    dropped: Vec<usize>,
}

/// Default queue capacity: enough for several simultaneous transmitters.
const DEFAULT_CAPACITY: usize = 64;

impl Default for EthernetLink {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl EthernetLink {
    pub fn new(capacity: usize) -> Self {
        Self {
            ports: Vec::new(),
            pending: VecDeque::new(),
            capacity: capacity.max(1),
            dropped: Vec::new(),
        }
    }

    /// Attach a NIC's port to the segment.
    pub fn add_port(&mut self, port: Box<dyn EthernetPort>) -> usize {
        self.ports.push(port);
        self.ports.len() - 1
    }

    pub fn port_count(&self) -> usize {
        self.ports.len()
    }

    /// Frames dropped per port since the last observation (a NIC reads this
    /// into its own RX-drop counter, so the loss is never silent).
    pub fn take_drops(&mut self) -> Vec<usize> {
        let mut drops = vec![0; self.ports.len()];
        for port in self.drain_drops() {
            drops[port] += 1;
        }
        drops
    }

    fn drain_drops(&mut self) -> impl Iterator<Item = usize> + '_ {
        std::mem::take(&mut self.dropped).into_iter()
    }

    /// Queue one frame from port `from` onto the segment. The queue is
    /// bounded; a full queue is a transmit error the NIC observes, never a
    /// silent loss.
    pub fn transmit(&mut self, from: usize, frame: &[u8]) -> Result<(), EthError> {
        if from >= self.ports.len() || frame.len() < 12 {
            return Err(EthError::FrameTooShort);
        }
        if self.pending.len() >= self.capacity {
            return Err(EthError::RxFull);
        }
        self.pending.push_back(Pending {
            from,
            frame: frame.to_vec(),
        });
        Ok(())
    }

    /// Deliver every pending frame, in transmit order. A port whose receive
    /// path is full drops the frame — recorded, not hidden.
    pub fn flush(&mut self) {
        let pending: Vec<Pending> = std::mem::take(&mut self.pending).into_iter().collect();
        for item in pending {
            for index in 0..self.ports.len() {
                if index == item.from {
                    continue;
                }
                if accepts(self.ports[index].as_ref(), &item.frame)
                    && self.ports[index].receive(&item.frame).is_err()
                {
                    self.dropped.push(index);
                }
            }
        }
    }

    /// Queue and deliver in one step, for models that run at transaction
    /// granularity.
    pub fn transmit_now(&mut self, from: usize, frame: &[u8]) -> Result<(), EthError> {
        self.transmit(from, frame)?;
        self.flush();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::ethernet::Mac;
    use std::sync::{Arc, Mutex};

    struct Port {
        mac: Mac,
        promiscuous: bool,
        received: Vec<Vec<u8>>,
        capacity: usize,
    }

    impl Port {
        fn new(mac: Mac) -> Self {
            Self {
                mac,
                promiscuous: false,
                received: Vec::new(),
                capacity: 8,
            }
        }

        fn full(mut self) -> Self {
            self.capacity = 0;
            self
        }
    }

    impl EthernetPort for Port {
        fn mac(&self) -> Mac {
            self.mac
        }

        fn promiscuous(&self) -> bool {
            self.promiscuous
        }

        fn accepts_multicast(&self, _address: Mac) -> bool {
            false
        }

        fn receive(&mut self, frame: &[u8]) -> Result<(), EthError> {
            if self.received.len() >= self.capacity {
                return Err(EthError::RxFull);
            }
            self.received.push(frame.to_vec());
            Ok(())
        }
    }

    /// A port the test can still observe after it was boxed onto the link.
    struct SharedPort {
        inner: Arc<Mutex<Port>>,
    }

    impl SharedPort {
        fn new(mac: Mac) -> (Self, Arc<Mutex<Port>>) {
            let inner = Arc::new(Mutex::new(Port::new(mac)));
            (
                Self {
                    inner: Arc::clone(&inner),
                },
                inner,
            )
        }

        fn full(mac: Mac) -> (Self, Arc<Mutex<Port>>) {
            let inner = Arc::new(Mutex::new(Port::new(mac).full()));
            (
                Self {
                    inner: Arc::clone(&inner),
                },
                inner,
            )
        }
    }

    impl EthernetPort for SharedPort {
        fn mac(&self) -> Mac {
            self.inner.lock().map(|port| port.mac()).unwrap_or([0; 6])
        }

        fn promiscuous(&self) -> bool {
            self.inner
                .lock()
                .map(|port| port.promiscuous())
                .unwrap_or(false)
        }

        fn accepts_multicast(&self, address: Mac) -> bool {
            self.inner
                .lock()
                .map(|port| port.accepts_multicast(address))
                .unwrap_or(false)
        }

        fn receive(&mut self, frame: &[u8]) -> Result<(), EthError> {
            self.inner
                .lock()
                .map_err(|_| EthError::RxFull)?
                .receive(frame)
        }
    }

    fn frame(destination: Mac, from: Mac) -> Vec<u8> {
        let mut frame = destination.to_vec();
        frame.extend_from_slice(&from);
        frame.extend_from_slice(&[0x08, 0x00, 0xAA, 0xBB]);
        frame
    }

    const A: Mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
    const B: Mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x02];

    #[test]
    fn a_frame_reaches_only_the_addressed_port() {
        let mut link = EthernetLink::default();
        let (shared_a, port_a) = SharedPort::new(A);
        let (shared_b, port_b) = SharedPort::new(B);
        let a = link.add_port(Box::new(shared_a));
        link.add_port(Box::new(shared_b));
        link.transmit_now(a, &frame(B, A)).unwrap();
        // B received it; A (the sender) never loops its own frame back.
        assert_eq!(port_b.lock().unwrap().received.len(), 1);
        assert_eq!(port_a.lock().unwrap().received.len(), 0);
        assert_eq!(link.take_drops(), vec![0, 0]);
    }

    #[test]
    fn broadcast_reaches_every_other_port() {
        let mut link = EthernetLink::default();
        let (shared_a, port_a) = SharedPort::new(A);
        let (shared_b, port_b) = SharedPort::new(B);
        let a = link.add_port(Box::new(shared_a));
        link.add_port(Box::new(shared_b));
        link.transmit_now(a, &frame([0xFF; 6], A)).unwrap();
        assert_eq!(port_b.lock().unwrap().received.len(), 1);
        assert_eq!(port_a.lock().unwrap().received.len(), 0);
    }

    #[test]
    fn a_full_receive_path_is_a_recorded_drop() {
        let mut link = EthernetLink::default();
        let (shared_a, _port_a) = SharedPort::new(A);
        let (shared_b, _port_b) = SharedPort::full(B);
        let a = link.add_port(Box::new(shared_a));
        link.add_port(Box::new(shared_b));
        link.transmit_now(a, &frame(B, A)).unwrap();
        assert_eq!(link.take_drops(), vec![0, 1]);
    }

    #[test]
    fn a_short_frame_is_rejected() {
        let mut link = EthernetLink::default();
        let (shared_a, _port_a) = SharedPort::new(A);
        let a = link.add_port(Box::new(shared_a));
        assert_eq!(
            link.transmit_now(a, &[0u8; 4]),
            Err(EthError::FrameTooShort)
        );
    }
}
