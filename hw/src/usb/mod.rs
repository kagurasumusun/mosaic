//! usb (QEMU `hw/usb/` role).
pub mod catalog;
pub mod hid_keyboard;
pub mod hid_mouse;
pub mod uhci;

pub mod am335x_usb;
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
pub mod usb_f1;
pub mod usb_f4;
pub mod usb_h7;
pub mod xhci;
