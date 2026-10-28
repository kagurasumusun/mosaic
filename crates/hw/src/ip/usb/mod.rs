//! USB host controller IP blocks (QEMU `hw/usb/`); USB devices
//! (keyboards, mice, storage) live in `crate::part::*`.
pub mod am335x_usb;
pub mod catalog;
pub mod dwc2;
pub mod dwc3;
pub mod ehci;
pub mod lpc11xx_usb;
pub mod lpc17xx_usb;
pub mod lpc43xx_usb;
pub mod mtk_xhci;
pub mod ohci;
pub mod pic32_usb;
pub mod rk_dwc2;
pub mod rp2040_usb;
pub mod sunxi_musb;
pub mod uhci;
pub mod usb_f1;
pub mod usb_f4;
pub mod usb_h7;
pub mod xhci;
