//! Network controller IP blocks (QEMU `hw/net/`): on-SoC MACs, PCI and
//! ISA NICs. PHYs, wireless modules and modem cards live in
//! `crate::part::net`.
pub mod am79c970;
pub mod am79c973;
pub mod bcmgenet;
pub mod catalog;
pub mod cpsw;
pub mod davinci_mdio;
pub mod dm9000;
pub mod dp83932;
pub mod dwmac;
pub mod e1000;
pub mod e1000e;
pub mod fec;
pub mod gmac;
pub mod i82540;
pub mod i82551;
pub mod i82557;
pub mod i82559;
pub mod i82574;
pub mod lan9118;
pub mod lan91c111;
pub mod lan9220;
pub mod lan9250;
pub mod mv643xx;
pub mod pcnet32;
pub mod rtl8139;
pub mod rtl8169;
