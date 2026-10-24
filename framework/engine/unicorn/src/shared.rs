//! Host-backed shared RAM mapped into engine instances (design §12.3 rule 1
//! and the §13.1 DMI path). One reviewed unsafe call; see ADR 0010.

use std::sync::{Arc, Mutex, MutexGuard};

use unicorn_engine::{unicorn_const::Prot, Unicorn};

use crate::{EngineData, EngineError};

/// Fixed-capacity host buffer that several engine instances can map at once.
///
/// This is the shared-RAM path between cores: every Unicorn instance maps the
/// same host memory through `uc_mem_map_ptr`, so one core's guest stores are
/// immediately visible to every other core mapping the buffer. Cross-instance
/// coherence was verified experimentally on Unicorn 2.1.x (ADR 0010): an A32
/// store and an RV32I load through two instances observe one buffer.
///
/// The buffer is created at a fixed size and never resized, so its host
/// pointer stays stable. Engines hold an `Arc<SharedRam>` next to the mapping,
/// which guarantees the buffer outlives every address space referencing it.
/// Host-side [`SharedRam::read`] / [`SharedRam::write`] are for the composition
/// root between run quanta; single-threaded interleaved execution is the
/// composition invariant (design §12.4: parallel mode is future work, and
/// guest access during `emu_start` never touches the host mutex).
pub struct SharedRam {
    bytes: Mutex<Vec<u8>>,
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
        }))
    }

    /// Buffer size in bytes.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Stable host pointer for engine mapping. Safe to produce; only the
    /// reviewed [`map_host`] call dereferences it inside Unicorn.
    pub(super) fn as_ptr(&self) -> *mut core::ffi::c_void {
        self.lock().as_mut_ptr().cast()
    }
}

/// Map `ram` into one engine instance's address space.
#[allow(unsafe_code)]
pub(super) fn map_host(
    uc: &mut Unicorn<'_, EngineData>,
    base: u64,
    size: u64,
    prot: Prot,
    ram: &SharedRam,
) -> Result<(), EngineError> {
    // SAFETY: `ram` is a fixed-capacity Vec that is never resized, so the
    // pointer is stable for the buffer's lifetime; the calling adapter stores
    // an `Arc<SharedRam>` beside the mapping, keeping the buffer alive at
    // least as long as the engine address space. `uc_mem_map_ptr` never takes
    // ownership: closing or unmapping the instance does not free host memory.
    // Engines run single-threaded and interleaved (design §12.4), so no two
    // guests touch the buffer simultaneously; host accessors take the mutex
    // only between run quanta.
    unsafe { uc.mem_map_ptr(base, size, prot, ram.as_ptr()) }.map_err(|_| EngineError::Backend)
}
