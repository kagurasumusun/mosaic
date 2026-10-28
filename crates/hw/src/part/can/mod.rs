//! External CAN controller parts (MCP251x): SPI chips that bridge onto
//! a CAN bus. On-SoC CAN IP will live in `crate::ip::can`.
pub mod catalog;
pub mod mcp2515;
pub mod mcp2517fd;
pub mod mcp2518fd;
