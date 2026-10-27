//! Flash devices: catalog entries (ADR 0025).

use crate::catalog::ModelEntry;

/// CFI flash spawns through the board: the backing image (ROM contents) is
/// board-unique data, not wiring — the factory would need the bytes before
/// the manifest carried them, so boards construct it directly.
pub const MODELS: &[ModelEntry] = &[];
