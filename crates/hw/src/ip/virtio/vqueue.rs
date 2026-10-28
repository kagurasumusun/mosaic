//! The split virtqueue — the ring machinery every virtio device shares
//! (virtio 1.0/1.1 "split" rings, the only layout virtio-mmio v2 defines
//! for its 64-bit queue registers).
//!
//! A queue is three guest-memory tables the device never copies out:
//!
//! ```text
//! descriptor table  16 bytes/entry: addr u64 | len u32 | flags u16 | next u16
//! avail ring        flags u16 | idx u16 | ring[num] u16 | used_event u16
//! used ring         flags u16 | idx u16 | used[num] { id u32, len u32 } | avail_event u16
//! ```
//!
//! The driver owns the avail side (posting free descriptors), the device
//! owns the used side (posting completions). Both sides advance their
//! `idx` in one direction and only ever read the other side's — the
//! single-producer/single-consumer contract that makes the ring
//! lock-free on real hardware and borrow-free here.
//!
//! Chained descriptors: `F_NEXT` walks `next`; a chain ends where the
//! flag is clear. The device walks chains, never assumes a fixed
//! request shape, and only ever writes into descriptors the driver
//! marked `F_WRITE` (device-writable) — a driver bug that asks the
//! device to write a read-only descriptor surfaces as an error
//! completion, not as silent memory corruption.
//!
//! All access goes through the [`GuestMemory`] port (the machine glue's
//! adapter over the machine's RAM), at the guest-physical addresses the
//! driver programmed into the transport. The ring structures are
//! little-endian on the wire regardless of the guest's endianness, so
//! every field conversion is explicit here.

/// Descriptor flag: the driver may chain this entry through `next`.
pub const F_NEXT: u16 = 1;
/// Descriptor flag: the device may write into this descriptor's buffer.
pub const F_WRITE: u16 = 2;
/// Descriptor flag: indirect table (descriptor table in the buffer).
pub const F_INDIRECT: u16 = 4;

/// One descriptor-table walk error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    /// A descriptor address+len fell outside guest RAM.
    Unmapped { address: u64 },
    /// A chain referenced an out-of-range descriptor index.
    BadIndex { index: u32 },
    /// A chain was longer than the queue (a cycle).
    ChainTooLong,
    /// The driver posted a buffer the device may not write.
    NotWritable { index: u32 },
    /// The buffer is not large enough for what the device must place.
    TooSmall { needed: u32, have: u32 },
}

impl From<mosaic_contract::guest_mem::GuestMemError> for QueueError {
    fn from(value: mosaic_contract::guest_mem::GuestMemError) -> Self {
        match value {
            mosaic_contract::guest_mem::GuestMemError::Unmapped { address } => {
                Self::Unmapped { address }
            }
            _ => Self::Unmapped { address: 0 },
        }
    }
}

/// The guest-memory port a virtio queue walks (contract::guest_mem's
/// trait, re-exported so the virtio modules name it once).
pub use mosaic_contract::guest_mem::GuestMemory;

/// One queue's register image as the transport programs it (virtio-mmio
/// v2 `QUEUE_DESC/AVAIL/USED` 64-bit addresses plus size and ready).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueConfig {
    pub size: u32,
    pub desc_addr: u64,
    pub avail_addr: u64,
    pub used_addr: u64,
    pub ready: bool,
}

/// The device-side cursor over one configured queue: where the device has
/// consumed to (`seen_avail`) and where it has published to (`seen_used`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueCursor {
    pub seen_avail: u16,
    pub seen_used: u16,
}

/// A device-side view of one split virtqueue.
pub struct SplitVirtqueue<'a> {
    config: &'a QueueConfig,
    cursor: &'a mut QueueCursor,
}

impl<'a> SplitVirtqueue<'a> {
    pub fn new(config: &'a QueueConfig, cursor: &'a mut QueueCursor) -> Self {
        Self { config, cursor }
    }

    #[allow(dead_code)] // ring-size masking the used/avail helpers share
    fn queue_mask(&self) -> u32 {
        self.config.size.saturating_sub(1)
    }

    fn read_u16(&self, mem: &dyn GuestMemory, address: u64) -> Result<u16, QueueError> {
        let mut bytes = [0u8; 2];
        mem.read_bytes(address, &mut bytes)
            .map_err(QueueError::from)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn write_u16(
        &self,
        mem: &mut dyn GuestMemory,
        address: u64,
        value: u16,
    ) -> Result<(), QueueError> {
        mem.write_bytes(address, &value.to_le_bytes())
            .map_err(QueueError::from)
    }

    #[allow(dead_code)] // used by the descriptor walk's future consumers
    fn read_u32(&self, mem: &dyn GuestMemory, address: u64) -> Result<u32, QueueError> {
        let mut bytes = [0u8; 4];
        mem.read_bytes(address, &mut bytes)
            .map_err(QueueError::from)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn write_u32(
        &self,
        mem: &mut dyn GuestMemory,
        address: u64,
        value: u32,
    ) -> Result<(), QueueError> {
        mem.write_bytes(address, &value.to_le_bytes())
            .map_err(QueueError::from)
    }

    /// The driver's current avail index (how many requests it has posted
    /// since queue creation).
    pub fn avail_index(&self, mem: &dyn GuestMemory) -> Result<u16, QueueError> {
        self.read_u16(mem, self.config.avail_addr + 2)
    }

    /// How many requests are waiting: `avail.idx - seen_avail` (mod 2^16).
    pub fn pending(&self, mem: &dyn GuestMemory) -> Result<u16, QueueError> {
        let avail = self.avail_index(mem)?;
        Ok(avail.wrapping_sub(self.cursor.seen_avail))
    }

    /// Take the next request's head descriptor index, advancing the
    /// seen-avail cursor. Returns `None` when the queue is drained.
    pub fn next_head(&mut self, mem: &mut dyn GuestMemory) -> Result<Option<u32>, QueueError> {
        let avail = self.avail_index(mem)?;
        if avail == self.cursor.seen_avail {
            return Ok(None);
        }
        let slot = (self.cursor.seen_avail % self.config.size as u16) as u64;
        let ring_addr = self.config.avail_addr + 4 + slot * 2;
        let head = u32::from(self.read_u16(mem, ring_addr)?);
        self.cursor.seen_avail = self.cursor.seen_avail.wrapping_add(1);
        Ok(Some(head))
    }

    /// Walk the chain starting at `head`, calling `buffer` once per
    /// descriptor with (address, length, device_writable, index). The
    /// walk enforces the queue-size bound, so a cyclic chain reports
    /// `ChainTooLong` instead of looping.
    pub fn walk<F>(&self, mem: &dyn GuestMemory, head: u32, mut buffer: F) -> Result<(), QueueError>
    where
        F: FnMut(u64, u32, bool, u32) -> Result<(), QueueError>,
    {
        let mut index = head;
        for _ in 0..self.config.size {
            if index >= self.config.size {
                return Err(QueueError::BadIndex { index });
            }
            let base = self.config.desc_addr + (index as u64) * 16;
            let mut fields = [0u8; 16];
            mem.read_bytes(base, &mut fields)?;
            let addr = u64::from_le_bytes(fields[0..8].try_into().unwrap());
            let len = u32::from_le_bytes(fields[8..12].try_into().unwrap());
            let flags = u16::from_le_bytes(fields[12..14].try_into().unwrap());
            let next = u16::from_le_bytes(fields[14..16].try_into().unwrap());
            buffer(addr, len, flags & F_WRITE != 0, index)?;
            if flags & F_NEXT == 0 {
                return Ok(());
            }
            index = u32::from(next);
        }
        Err(QueueError::ChainTooLong)
    }

    /// Read one chain's bytes into `out`, honoring nothing but layout —
    /// the caller has already validated writability. Returns the bytes
    /// consumed (the chain's total length).
    pub fn read_chain(
        &self,
        mem: &mut dyn GuestMemory,
        head: u32,
        out: &mut Vec<u8>,
    ) -> Result<u32, QueueError> {
        // Collect first (the walk borrows immutably), then act: the
        // two-phase shape every memory-writing path in this module uses.
        let mut segments: Vec<(u64, u32)> = Vec::new();
        self.walk(mem, head, |addr, len, _writable, _index| {
            segments.push((addr, len));
            Ok(())
        })?;
        let start = out.len();
        for (addr, len) in segments {
            let old = out.len();
            out.resize(old + len as usize, 0);
            mem.read_bytes(addr, &mut out[old..])
                .map_err(QueueError::from)?;
        }
        Ok((out.len() - start) as u32)
    }

    /// Write `data` into the chain's device-writable descriptors, in
    /// order. Stops at `data.len()` bytes; fails if the writable space
    /// is smaller than the data.
    pub fn write_chain(
        &self,
        mem: &mut dyn GuestMemory,
        head: u32,
        data: &[u8],
    ) -> Result<u32, QueueError> {
        let mut segments: Vec<(u64, u32, bool)> = Vec::new();
        self.walk(mem, head, |addr, len, writable, index| {
            if !writable {
                return Err(QueueError::NotWritable { index });
            }
            segments.push((addr, len, writable));
            Ok(())
        })?;
        let mut offset = 0usize;
        for (addr, len, _) in segments {
            let take = (len as usize).min(data.len() - offset);
            mem.write_bytes(addr, &data[offset..offset + take])
                .map_err(QueueError::from)?;
            offset += take;
        }
        if offset < data.len() {
            return Err(QueueError::TooSmall {
                needed: data.len() as u32,
                have: offset as u32,
            });
        }
        Ok(offset as u32)
    }

    /// Total byte length of a chain (writable and read-only together).
    pub fn chain_len(&self, mem: &dyn GuestMemory, head: u32) -> Result<u32, QueueError> {
        let mut total = 0u32;
        self.walk(mem, head, |_addr, len, _writable, _index| {
            total = total.saturating_add(len);
            Ok(())
        })?;
        Ok(total)
    }

    /// Post one completion: the used-ring entry for `head` with `written`
    /// bytes, then the used-index bump. The guest polls that index.
    pub fn complete(
        &mut self,
        mem: &mut dyn GuestMemory,
        head: u32,
        written: u32,
    ) -> Result<(), QueueError> {
        let slot = (self.cursor.seen_used % self.config.size as u16) as u64;
        let entry = self.config.used_addr + 4 + slot * 8;
        self.write_u32(mem, entry, head)?;
        self.write_u32(mem, entry + 4, written)?;
        let next = self.cursor.seen_used.wrapping_add(1);
        self.write_u16(mem, self.config.used_addr + 2, next)?;
        self.cursor.seen_used = next;
        Ok(())
    }

    /// Whether the driver enabled the used-ring no-interrupt flag —
    /// the transport consults this before raising its IRQ.
    pub fn no_interrupt(&self, mem: &dyn GuestMemory) -> Result<bool, QueueError> {
        Ok(self.read_u16(mem, self.config.used_addr)? & 1 != 0)
    }
}

/// Shared test support: a flat guest RAM mapped at 0x1000 (cfg(test) so
/// sibling modules in this crate reuse one fixture, never the runtime).
#[cfg(test)]
pub mod tests_support {
    use super::GuestMemory;

    #[derive(Clone)]
    pub struct TestRam {
        pub bytes: Vec<u8>,
    }

    impl TestRam {
        pub fn new(size: usize) -> Self {
            Self {
                bytes: vec![0; size],
            }
        }
    }

    impl GuestMemory for TestRam {
        fn read_bytes(
            &self,
            address: u64,
            out: &mut [u8],
        ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
            let start = (address - 0x1000) as usize;
            let end = start + out.len();
            let slice = self
                .bytes
                .get(start..end)
                .ok_or(mosaic_contract::guest_mem::GuestMemError::Unmapped { address })?;
            out.copy_from_slice(slice);
            Ok(())
        }
        fn write_bytes(
            &mut self,
            address: u64,
            data: &[u8],
        ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
            let start = (address - 0x1000) as usize;
            let end = start + data.len();
            let slice = self
                .bytes
                .get_mut(start..end)
                .ok_or(mosaic_contract::guest_mem::GuestMemError::Unmapped { address })?;
            slice.copy_from_slice(data);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::TestRam;
    use super::*;

    /// A flat 64 KiB guest RAM for the tests, mapped at 0x1000 (the
    /// shared support fixture).
    type Ram = TestRam;

    /// Build a queue at fixed addresses: desc 0x1000, avail 0x2000,
    /// used 0x3000, with `size` entries.
    fn queue(size: u32) -> (QueueConfig, QueueCursor) {
        (
            QueueConfig {
                size,
                desc_addr: 0x1000,
                avail_addr: 0x2000,
                used_addr: 0x3000,
                ready: true,
            },
            QueueCursor::default(),
        )
    }

    fn push_desc(mem: &mut Ram, slot: u32, addr: u64, len: u32, flags: u16, next: u16) {
        let base = 0x1000 + (slot as u64) * 16;
        mem.bytes[(base - 0x1000) as usize..][..16].copy_from_slice(&{
            let mut entry = [0u8; 16];
            entry[0..8].copy_from_slice(&addr.to_le_bytes());
            entry[8..12].copy_from_slice(&len.to_le_bytes());
            entry[12..14].copy_from_slice(&flags.to_le_bytes());
            entry[14..16].copy_from_slice(&next.to_le_bytes());
            entry
        });
    }

    fn push_avail(mem: &mut Ram, ring_slot: u16, head: u16, idx: u16) {
        let ring = 0x2000 + 4 + (ring_slot as u64) * 2;
        mem.bytes[(ring - 0x1000) as usize..][..2].copy_from_slice(&head.to_le_bytes());
        mem.bytes[(0x2000 + 2 - 0x1000) as usize..][..2].copy_from_slice(&idx.to_le_bytes());
    }

    fn used_entry(mem: &Ram, slot: u16) -> (u32, u32) {
        let entry = 0x3000 + 4 + (slot as u64) * 8;
        let id = u32::from_le_bytes(
            mem.bytes[(entry - 0x1000) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        let len = u32::from_le_bytes(
            mem.bytes[(entry + 4 - 0x1000) as usize..][..4]
                .try_into()
                .unwrap(),
        );
        (id, len)
    }

    fn used_index(mem: &Ram) -> u16 {
        u16::from_le_bytes(
            mem.bytes[(0x3000 + 2 - 0x1000) as usize..][..2]
                .try_into()
                .unwrap(),
        )
    }

    #[test]
    fn single_descriptor_round_trip() {
        let (config, mut cursor) = queue(8);
        let mut mem = Ram::new(0x1_0000);
        // One write-only descriptor at 0x4000, length 8.
        push_desc(&mut mem, 0, 0x4000, 8, F_WRITE, 0);
        push_avail(&mut mem, 0, 0, 1);
        {
            let mut vq = SplitVirtqueue::new(&config, &mut cursor);
            assert_eq!(vq.pending(&mem).unwrap(), 1);
            let head = vq.next_head(&mut mem).unwrap().unwrap();
            assert_eq!(head, 0);
            assert_eq!(vq.chain_len(&mem, head).unwrap(), 8);
        }
        {
            let mut vq = SplitVirtqueue::new(&config, &mut cursor);
            vq.write_chain(&mut mem, 0, &[7; 8]).unwrap();
            vq.complete(&mut mem, 0, 8).unwrap();
        }
        assert_eq!(used_entry(&mem, 0), (0, 8));
        assert_eq!(used_index(&mem), 1);
    }

    #[test]
    fn chained_read_across_descriptors() {
        let (config, mut cursor) = queue(8);
        let mut mem = Ram::new(0x1_0000);
        // Chain 0 → 1, buffers at 0x4000 (4 bytes) and 0x5000 (6 bytes).
        push_desc(&mut mem, 0, 0x4000, 4, F_NEXT, 1);
        push_desc(&mut mem, 1, 0x5000, 6, 0, 0);
        mem.bytes[(0x4000 - 0x1000) as usize..][..4].copy_from_slice(&[1, 2, 3, 4]);
        mem.bytes[(0x5000 - 0x1000) as usize..][..6].copy_from_slice(&[5, 6, 7, 8, 9, 10]);
        push_avail(&mut mem, 0, 0, 1);
        let mut vq = SplitVirtqueue::new(&config, &mut cursor);
        let head = vq.next_head(&mut mem).unwrap().unwrap();
        let mut data = Vec::new();
        let len = vq.read_chain(&mut mem, head, &mut data).unwrap();
        assert_eq!(len, 10);
        assert_eq!(data, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn write_to_readonly_descriptor_is_rejected() {
        let (config, mut cursor) = queue(8);
        let mut mem = Ram::new(0x1_0000);
        push_desc(&mut mem, 0, 0x4000, 8, 0, 0);
        push_avail(&mut mem, 0, 0, 1);
        let mut vq = SplitVirtqueue::new(&config, &mut cursor);
        let head = vq.next_head(&mut mem).unwrap().unwrap();
        assert_eq!(
            vq.write_chain(&mut mem, head, &[0; 4]),
            Err(QueueError::NotWritable { index: 0 })
        );
    }

    #[test]
    fn cyclic_chain_reports_chain_too_long() {
        let (config, mut cursor) = queue(4);
        let mut mem = Ram::new(0x1_0000);
        push_desc(&mut mem, 0, 0x4000, 4, F_NEXT, 1);
        push_desc(&mut mem, 1, 0x4004, 4, F_NEXT, 0);
        // Redirect descriptor 0's next to itself: 0 -> 0 -> ...
        push_desc(&mut mem, 0, 0x4000, 4, F_NEXT, 0);
        let vq = SplitVirtqueue::new(&config, &mut cursor);
        assert_eq!(vq.chain_len(&mem, 0), Err(QueueError::ChainTooLong));
    }

    #[test]
    fn wrapped_used_ring_keeps_indices_correct() {
        let (config, mut cursor) = queue(4);
        let mut mem = Ram::new(0x1_0000);
        cursor.seen_used = 3;
        // Complete three entries: slot 3, then wrap to 0 and 1.
        for i in 0..3 {
            let mut vq = SplitVirtqueue::new(&config, &mut cursor);
            vq.complete(&mut mem, i, 16).unwrap();
        }
        assert_eq!(used_index(&mem), 6);
        assert_eq!(used_entry(&mem, 3).0, 0);
        assert_eq!(used_entry(&mem, 0).1, 16); // wrapped slot carries entry 1
    }

    #[test]
    fn pending_counts_across_index_wrap() {
        let (config, mut cursor) = queue(4);
        let mut mem = Ram::new(0x1_0000);
        cursor.seen_avail = 0xFFFF;
        push_avail(&mut mem, 3, 7, 0x0000);
        let vq = SplitVirtqueue::new(&config, &mut cursor);
        assert_eq!(vq.pending(&mem).unwrap(), 1);
        assert_eq!(vq.avail_index(&mem).unwrap(), 0);
    }
}
