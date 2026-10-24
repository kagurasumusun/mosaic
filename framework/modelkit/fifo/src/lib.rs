#![no_std]
#![forbid(unsafe_code)]
//! Bounded heap-free byte FIFO for device models such as UART TX/RX paths.
//!
//! The queue is a fixed-capacity ring backed by an array, so a full FIFO is a
//! reported error and never a panic or an allocation. Capacity zero is valid:
//! every push is rejected, matching a device with no buffer configured.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FifoError {
    Full,
}

#[derive(Clone, Debug)]
pub struct ByteFifo<const N: usize> {
    buffer: [u8; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Default for ByteFifo<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ByteFifo<N> {
    pub const fn new() -> Self {
        Self {
            buffer: [0; N],
            head: 0,
            len: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == N
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Append a byte. A full FIFO rejects the write without changing state.
    pub fn push(&mut self, byte: u8) -> Result<(), FifoError> {
        if self.is_full() {
            return Err(FifoError::Full);
        }
        let slot = (self.head + self.len) % N;
        self.buffer[slot] = byte;
        self.len += 1;
        Ok(())
    }

    /// Remove the oldest byte, or `None` when empty.
    pub fn pop(&mut self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let byte = self.buffer[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(byte)
    }

    /// Copy queued bytes in dequeue order. Returns how many bytes were written.
    pub fn copy_out(&self, out: &mut [u8]) -> usize {
        let count = self.len.min(out.len());
        for (index, slot) in out.iter_mut().enumerate().take(count) {
            let source = (self.head + index) % N.max(1);
            *slot = self.buffer[source];
        }
        count
    }

    /// Replace the queue with `bytes` in order. A longer slice is rejected and
    /// leaves the previous contents unchanged.
    pub fn replace_with(&mut self, bytes: &[u8]) -> Result<(), FifoError> {
        if bytes.len() > N {
            return Err(FifoError::Full);
        }
        self.clear();
        for byte in bytes {
            self.push(*byte)
                .expect("length was checked against capacity");
        }
        Ok(())
    }

    /// Inspect the oldest byte without removing it.
    pub fn peek(&self) -> Option<u8> {
        if self.is_empty() {
            None
        } else {
            Some(self.buffer[self.head])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order_across_wrap_around() {
        let mut fifo = ByteFifo::<3>::new();
        assert!(fifo.is_empty());
        fifo.push(1).unwrap();
        fifo.push(2).unwrap();
        assert_eq!(fifo.pop(), Some(1));
        fifo.push(3).unwrap();
        fifo.push(4).unwrap();
        assert!(fifo.is_full());
        assert_eq!(fifo.peek(), Some(2));
        assert_eq!(fifo.pop(), Some(2));
        assert_eq!(fifo.pop(), Some(3));
        assert_eq!(fifo.pop(), Some(4));
        assert_eq!(fifo.pop(), None);
    }

    #[test]
    fn a_full_fifo_rejects_without_dropping_state() {
        let mut fifo = ByteFifo::<2>::new();
        fifo.push(7).unwrap();
        fifo.push(8).unwrap();
        assert_eq!(fifo.push(9), Err(FifoError::Full));
        assert_eq!(fifo.len(), 2);
        fifo.clear();
        assert!(fifo.is_empty());
        assert_eq!(fifo.pop(), None);
        fifo.push(10).unwrap();
        assert_eq!(fifo.pop(), Some(10));
    }

    #[test]
    fn zero_capacity_never_accepts_a_byte() {
        let mut fifo = ByteFifo::<0>::new();
        assert!(fifo.is_full());
        assert_eq!(fifo.push(1), Err(FifoError::Full));
        assert_eq!(fifo.pop(), None);
    }
}
