#![forbid(unsafe_code)]
//! `block` — guest image formats (QEMU's `block/`): ISO9660 today, raw and
//! qcow2 when machines grow disk backends.

pub mod iso9660;
