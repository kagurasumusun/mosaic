//! Host-backed shared RAM for engine instances (design §12.3 rule 1 and the
//! §13.1 DMI path).
//!
//! # Why the pages are synchronised instead of mapped directly
//!
//! Unicorn 2.1.x exposes `uc_mem_map_ptr`, which maps a host buffer into an
//! engine address space. The guest may **read** such a region, but a guest
//! **store faults** with `UC_ERR_WRITE_UNMAPPED`: the mapping is read-only
//! towards the guest (verified experimentally on Unicorn 2.1.4, ADR 0010).
//! A shared RAM that cores can write therefore cannot be a plain host
//! pointer map in this backend.
//!
//! The runtime-backed alternative keeps the single source of truth in the
//! machine's backing block and synchronises the page at deterministic
//! boundaries:
//!
//! * before a core slice: the authoritative block is copied into the
//!   engine's writable page (`EngineMap::sync_shared_in`);
//! * after a core slice: the engine's page is copied back
//!   (`EngineMap::sync_shared_out`).
//!
//! With the fixed-order round-robin scheduler (design §12.4) the copy points
//! are fixed, so the observable behaviour stays deterministic; a write by one
//! core becomes visible to the next core in the round, which is exactly what
//! the doorbell handshake protocol needs. A backend that supports writable
//! host pointers can implement the same contract with a real shared mapping,
//! without changing the composition root.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::EngineError;

/// One shared page (or window) of RAM, owned by the composition root and
/// mirrored into every engine that maps it.
///
/// The buffer is created at a fixed size and never resized, so the engine
/// mapping stays stable. [`SharedRam`] holds the authoritative bytes; the
/// engine keeps its own writable copy of the same size.
pub struct SharedRam {
    bytes: Mutex<Vec<u8>>,
    /// Set once the loader image has been copied in. Later cores binding the
    /// same buffer must not overwrite guest writes (design §12.3 rule 1).
    initialized: std::sync::atomic::AtomicBool,
}

impl SharedRam {
    /// A zeroed buffer of `size` bytes. Zero is a composition error, not a
    /// guest fault. The size is a byte count in the address-space unit, so
    /// callers pass the same `u64` they use for mapping sizes.
    pub fn new(size: u64) -> Result<Arc<Self>, EngineError> {
        let size = usize::try_from(size).map_err(|_| EngineError::InvalidMapping)?;
        if size == 0 {
            return Err(EngineError::InvalidMapping);
        }
        Ok(Arc::new(Self {
            bytes: Mutex::new(vec![0u8; size]),
            initialized: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    /// Buffer size in bytes.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the loader image has already been copied into the buffer.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Mark the buffer as carrying the loader image. Returns false when
    /// another core already initialized it, so the caller can skip the copy.
    pub fn mark_initialized(&self) -> bool {
        !self
            .initialized
            .swap(true, std::sync::atomic::Ordering::AcqRel)
    }

    /// Host-side copy into the buffer. Out-of-bounds spans are errors; guest
    /// input never panics.
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<(), EngineError> {
        let mut bytes = self.lock();
        let end = offset
            .checked_add(data.len())
            .ok_or(EngineError::InvalidMapping)?;
        if end > bytes.len() {
            return Err(EngineError::InvalidMapping);
        }
        bytes[offset..end].copy_from_slice(data);
        Ok(())
    }

    /// Host-side copy out of the buffer.
    pub fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), EngineError> {
        let bytes = self.lock();
        let end = offset
            .checked_add(out.len())
            .ok_or(EngineError::InvalidMapping)?;
        if end > bytes.len() {
            return Err(EngineError::InvalidMapping);
        }
        out.copy_from_slice(&bytes[offset..end]);
        Ok(())
    }

    /// A poisoned host mutex is a bug in the composition, not guest input;
    /// the guarded value stays usable, so recovery keeps determinism.
    fn lock(&self) -> MutexGuard<'_, Vec<u8>> {
        self.bytes.lock().unwrap_or_else(|error| error.into_inner())
    }
}
