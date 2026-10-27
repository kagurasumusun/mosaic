//! USB port vocabulary (transaction level).
//!
//! USB is a tiered star: a host controller drives a root hub, hubs and
//! devices attach to ports, and every transfer is a sequence of packets
//! (token, data, handshake). This contract keeps the vocabulary a device
//! model needs — the setup packet, the standard requests, endpoint types,
//! and the packet-level outcomes a host controller observes. The fabric that
//! routes transfers between a controller and its attached devices lives in
//! `mosaic-bus::usb`.

/// USB transfer speed of a port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UsbSpeed {
    /// 1.5 Mbit/s (low speed, keyboards and mice).
    Low,
    /// 12 Mbit/s (full speed).
    Full,
    /// 480 Mbit/s (high speed).
    High,
    /// 5 Gbit/s (SuperSpeed, USB 3.x).
    Super,
}

impl UsbSpeed {
    /// Bit times per (bulk) byte frame at this speed, for controllers that
    /// budget frame bandwidth.
    pub const fn bits_per_second(self) -> u64 {
        match self {
            Self::Low => 1_500_000,
            Self::Full => 12_000_000,
            Self::High => 480_000_000,
            Self::Super => 5_000_000_000,
        }
    }
}

/// Token packet identifiers a host controller issues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UsbPid {
    /// SETUP token (control transfers start here).
    Setup,
    /// IN token (host reads).
    In,
    /// OUT token (host writes).
    Out,
    /// PRE/SOF as controllers emit them each 1 ms frame.
    Sof { frame: u16 },
}

/// Handshake outcomes a device answers with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UsbStatus {
    /// Data accepted / data returned.
    Ack,
    /// Endpoint has no data ready; retry later.
    Nak,
    /// Endpoint halted; needs a CLEAR_FEATURE(HALT).
    Stall,
    /// No response (error, bit stuff violation, ...).
    NoResponse,
}

/// Endpoint transfer type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UsbTransfer {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

impl UsbTransfer {
    /// The endpoint descriptor's `bmAttributes` transfer-type bits.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Control => 0,
            Self::Isochronous => 1,
            Self::Bulk => 2,
            Self::Interrupt => 3,
        }
    }
}

/// The 8-byte setup packet that begins every control transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetupPacket {
    /// Bit 7: direction (0 host-to-device, 1 device-to-host); bits 5-6:
    /// request type (0 standard, 1 class, 2 vendor); bits 0-4: recipient.
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

impl SetupPacket {
    pub const fn to_host(self) -> bool {
        self.bm_request_type & 0x80 != 0
    }

    pub const fn recipient(self) -> u8 {
        self.bm_request_type & 0x1F
    }

    pub const fn request_type(self) -> u8 {
        (self.bm_request_type >> 5) & 0x03
    }

    pub fn to_bytes(self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0] = self.bm_request_type;
        bytes[1] = self.b_request;
        bytes[2..4].copy_from_slice(&self.w_value.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.w_index.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.w_length.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }
        Some(Self {
            bm_request_type: bytes[0],
            b_request: bytes[1],
            w_value: u16::from_le_bytes([bytes[2], bytes[3]]),
            w_index: u16::from_le_bytes([bytes[4], bytes[5]]),
            w_length: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }
}

/// Standard device requests (`bmRequestType` type = 0, recipient = 0).
pub mod request {
    pub const GET_STATUS: u8 = 0x00;
    pub const CLEAR_FEATURE: u8 = 0x01;
    pub const SET_FEATURE: u8 = 0x03;
    pub const SET_ADDRESS: u8 = 0x05;
    pub const GET_DESCRIPTOR: u8 = 0x06;
    pub const SET_DESCRIPTOR: u8 = 0x07;
    pub const GET_CONFIGURATION: u8 = 0x08;
    pub const SET_CONFIGURATION: u8 = 0x09;
    pub const GET_INTERFACE: u8 = 0x0A;
    pub const SET_INTERFACE: u8 = 0x0B;
    pub const SYNCH_FRAME: u8 = 0x0C;

    /// `SET_FEATURE`/`CLEAR_FEATURE` selector for endpoint halt.
    pub const FEATURE_ENDPOINT_HALT: u16 = 0x00;
    /// `SET_FEATURE` selector for device remote wakeup.
    pub const FEATURE_REMOTE_WAKEUP: u16 = 0x01;
}

/// Descriptor type byte (high byte of `wValue` in GET_DESCRIPTOR).
pub mod descriptor {
    pub const DEVICE: u8 = 0x01;
    pub const CONFIGURATION: u8 = 0x02;
    pub const STRING: u8 = 0x03;
    pub const INTERFACE: u8 = 0x04;
    pub const ENDPOINT: u8 = 0x05;
    pub const DEVICE_QUALIFIER: u8 = 0x06;
    pub const OTHER_SPEED_CONFIGURATION: u8 = 0x07;
    pub const INTERFACE_POWER: u8 = 0x08;
    pub const HID: u8 = 0x21;
    pub const REPORT: u8 = 0x22;
    pub const PHYSICAL: u8 = 0x23;
}

/// Standard class codes a device descriptor's `bDeviceClass` holds.
pub mod class_code {
    pub const PER_INTERFACE: u8 = 0x00;
    pub const AUDIO: u8 = 0x01;
    pub const COMM: u8 = 0x02;
    pub const HID: u8 = 0x03;
    pub const PRINTER: u8 = 0x07;
    pub const MASS_STORAGE: u8 = 0x08;
    pub const HUB: u8 = 0x09;
    pub const CDC_DATA: u8 = 0x0A;
    pub const SMART_CARD: u8 = 0x0B;
    pub const VIDEO: u8 = 0x0E;
    pub const WIRELESS: u8 = 0xE0;
    pub const MISC: u8 = 0xEF;
    pub const VENDOR: u8 = 0xFF;
}

/// What a device did with a control transfer phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ControlResult {
    /// Transfer complete; the data phase moved `len` bytes.
    Complete { len: usize },
    /// More data expected in a later packet.
    Continue { len: usize },
    /// The request is not implemented by this device.
    Stall,
}

/// One attached USB device (implemented by device models in `hw`).
///
/// The device owns its descriptor tables and its endpoint state; the host
/// controller drives it through the fabric with the phases below. Addressing
/// (SET_ADDRESS) is the fabric's bookkeeping, so a device never stores its
/// own bus address.
pub trait UsbDevice: Send {
    /// The operating speed this device attaches at.
    fn speed(&self) -> UsbSpeed;

    /// Device descriptor (18 bytes) the fabric caches at attach time.
    fn device_descriptor(&self) -> [u8; 18];

    /// The active configuration descriptor (with all interface and endpoint
    /// descriptors concatenated).
    fn configuration_descriptor(&self) -> Vec<u8>;

    /// A string descriptor by index (0 = language list). None = no such
    /// string.
    fn string_descriptor(&self, index: u8) -> Option<Vec<u8>>;

    /// The SETUP stage of a control transfer. `payload_in` holds the whole
    /// data phase when the host wrote; the return value says what the device
    /// answers to the IN data phase.
    fn control(
        &mut self,
        setup: SetupPacket,
        payload_in: &[u8],
    ) -> Result<ControlResult, UsbStatus>;

    /// An IN data phase on a non-control endpoint: return up to `max` bytes
    /// (None = NAK, Some(empty) = zero-length packet).
    fn data_in(&mut self, endpoint: u8, max: usize) -> Result<Option<Vec<u8>>, UsbStatus>;

    /// An OUT data phase on a non-control endpoint.
    fn data_out(&mut self, endpoint: u8, data: &[u8]) -> Result<UsbStatus, UsbStatus>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_packet_round_trips() {
        let setup = SetupPacket {
            bm_request_type: 0x80,
            b_request: request::GET_DESCRIPTOR,
            w_value: u16::from(descriptor::DEVICE) << 8,
            w_index: 0,
            w_length: 18,
        };
        let parsed = SetupPacket::from_bytes(&setup.to_bytes()).unwrap();
        assert_eq!(parsed, setup);
        assert!(parsed.to_host());
        assert_eq!(parsed.request_type(), 0);
        assert_eq!(parsed.recipient(), 0);
    }

    #[test]
    fn transfer_bits_match_the_spec_table() {
        assert_eq!(UsbTransfer::Control.bits(), 0);
        assert_eq!(UsbTransfer::Interrupt.bits(), 3);
    }
}
