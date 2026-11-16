//! A bounded single-producer single-consumer queue that never allocates,
//! locks or waits after it is created.
//!
//! This is the one place in the recorder with `unsafe`. The callback side
//! needs a queue it can push to and pop from without a lock, an allocation or
//! a syscall; `std::sync::mpsc` is not documented to promise that, and a
//! `Mutex` (even `try_lock`) is a priority-inversion risk on the audio thread.
//! The structure is the textbook ring: the producer owns `tail`, the consumer
//! owns `head`, and each publishes its index with a release store that the
//! other reads with an acquire load, which orders the slot write before the
//! index that makes it visible.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Keeps `head` and `tail` on different cache lines.
#[repr(align(128))]
struct Padded(AtomicUsize);

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: Padded,
    tail: Padded,
}

// SAFETY: a slot is accessed by exactly one side at a time. The producer only
// writes slots in `[tail, head + capacity)` and the consumer only reads slots
// in `[head, tail)`; the acquire/release pair on the indices hands a slot from
// one side to the other. `T: Send` is what moving values across threads needs.
unsafe impl<T: Send> Sync for Ring<T> {}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let tail = *self.tail.0.get_mut();
        let mut head = *self.head.0.get_mut();
        while head != tail {
            // SAFETY: slots in [head, tail) hold initialised values that
            // were never popped; we have exclusive access in `drop`.
            unsafe { self.slots[head & self.mask].get_mut().assume_init_drop() };
            head = head.wrapping_add(1);
        }
    }
}

/// The pushing half.
pub(crate) struct Producer<T> {
    ring: Arc<Ring<T>>,
}

/// The popping half.
pub(crate) struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// A queue that holds at least `capacity` items (rounded up to a power of two).
pub(crate) fn channel<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = capacity.max(1).next_power_of_two();
    let slots = (0..capacity)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect();
    let ring = Arc::new(Ring {
        slots,
        mask: capacity - 1,
        head: Padded(AtomicUsize::new(0)),
        tail: Padded(AtomicUsize::new(0)),
    });
    (
        Producer {
            ring: Arc::clone(&ring),
        },
        Consumer { ring },
    )
}

impl<T> Producer<T> {
    /// Push without waiting. A full queue gives the value back.
    pub(crate) fn try_push(&mut self, value: T) -> Result<(), T> {
        let ring = &*self.ring;
        let tail = ring.tail.0.load(Ordering::Relaxed);
        let head = ring.head.0.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > ring.mask {
            return Err(value);
        }
        // SAFETY: slot `tail` is outside [head, tail), so the consumer will not
        // touch it until the release store below publishes it.
        unsafe { (*ring.slots[tail & ring.mask].get()).write(value) };
        ring.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Consumer<T> {
    /// Pop without waiting.
    pub(crate) fn try_pop(&mut self) -> Option<T> {
        let ring = &*self.ring;
        let head = ring.head.0.load(Ordering::Relaxed);
        let tail = ring.tail.0.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        // SAFETY: slot `head` is in [head, tail): initialised by the producer
        // (its release store of `tail` happens-before our acquire load) and
        // not read by anyone else. After the release store of `head` the
        // producer may overwrite it.
        let value = unsafe { (*ring.slots[head & ring.mask].get()).assume_init_read() };
        ring.head.0.store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Items waiting (a snapshot; the producer may add more).
    pub(crate) fn len(&self) -> usize {
        let ring = &*self.ring;
        ring.tail
            .0
            .load(Ordering::Acquire)
            .wrapping_sub(ring.head.0.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
#[path = "spsc_tests.rs"]
mod tests;
