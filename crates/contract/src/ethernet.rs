//! Ethernet port vocabulary.
//!
//! A NIC model speaks this port; the link fabric (`mosaic-bus::net`) exchanges
//! frames between the ports of one collision domain (a hub/switch segment).
//! This is the vocabulary only — QEMU's `NetClientState` role, without the
//! host backend plumbing, which lives with each NIC model in `hw`.

/// A MAC address as a NIC publishes it.
pub type Mac = [u8; 6];

/// Why a frame was not delivered to a port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EthError {
    /// The port's receive ring is full; the frame is dropped (a NIC counts
    /// this in its RX drop counter, never silently).
    RxFull,
    /// The frame is shorter than a legal Ethernet frame.
    FrameTooShort,
    /// The frame exceeds the port's MTU.
    FrameTooLong,
}

/// One frame on the wire: destination, source, then the payload
/// (length/type field included). No FCS: a simulated link does not corrupt.
pub type EthFrame = [u8];

/// The port a NIC model implements to sit on a link.
pub trait EthernetPort: Send {
    /// This port's MAC address (the NIC's station address).
    fn mac(&self) -> Mac;

    /// Whether the port accepts frames not addressed to it (promiscuous
    /// mode, as a NIC's packet-filter mode register configures).
    fn promiscuous(&self) -> bool;

    /// Whether the port's multicast hash accepts `address`.
    fn accepts_multicast(&self, address: Mac) -> bool;

    /// Deliver one frame into the NIC's receive path. Called by the link
    /// fabric only for frames this port accepts.
    fn receive(&mut self, frame: &EthFrame) -> Result<(), EthError>;
}

/// Filters a link applies before delivery — kept as a free function so both
/// the fabric and a NIC's own loopback test agree on who receives what.
pub fn accepts(port: &dyn EthernetPort, frame: &EthFrame) -> bool {
    if frame.len() < 6 {
        return false;
    }
    let destination = [frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]];
    if port.promiscuous() {
        return true;
    }
    if destination == port.mac() {
        return true;
    }
    if destination == [0xFF; 6] {
        return true;
    }
    if destination[0] & 1 != 0 {
        return port.accepts_multicast(destination);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Port {
        mac: Mac,
        promiscuous: bool,
        received: Vec<Vec<u8>>,
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

        fn receive(&mut self, frame: &EthFrame) -> Result<(), EthError> {
            self.received.push(frame.to_vec());
            Ok(())
        }
    }

    fn frame(destination: Mac) -> Vec<u8> {
        let mut frame = destination.to_vec();
        frame.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x02]);
        frame.extend_from_slice(&[0x08, 0x00]);
        frame.extend_from_slice(&[0xDE, 0xAD]);
        frame
    }

    #[test]
    fn a_port_receives_its_unicast_and_broadcast() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let port = Port {
            mac,
            promiscuous: false,
            received: Vec::new(),
        };
        assert!(accepts(&port, &frame(mac)));
        assert!(accepts(&port, &frame([0xFF; 6])));
        assert!(!accepts(
            &port,
            &frame([0x52, 0x54, 0x00, 0x99, 0x99, 0x99])
        ));
        assert!(accepts(
            &Port {
                mac,
                promiscuous: true,
                received: Vec::new(),
            },
            &frame([0x52, 0x54, 0x00, 0x99, 0x99, 0x99]),
        ));
    }
}
