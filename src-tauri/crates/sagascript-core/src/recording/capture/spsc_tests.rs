use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;

#[test]
fn items_come_out_in_order_and_a_full_queue_hands_the_value_back() {
    let (mut tx, mut rx) = channel::<u32>(4);
    assert!(rx.try_pop().is_none());
    for i in 0..4 {
        tx.try_push(i).unwrap();
    }
    assert_eq!(tx.try_push(99), Err(99), "full");
    assert_eq!(rx.len(), 4);
    for i in 0..4 {
        assert_eq!(rx.try_pop(), Some(i));
    }
    assert!(rx.try_pop().is_none());
}

#[test]
fn capacity_rounds_up_to_a_power_of_two_and_wraps_many_times() {
    let (mut tx, mut rx) = channel::<usize>(3);
    // Capacity 4. Cycle far past the capacity so the indices wrap the slots.
    let mut next_in = 0usize;
    let mut next_out = 0usize;
    for round in 0..10_000 {
        let burst = 1 + round % 4;
        for _ in 0..burst {
            tx.try_push(next_in).unwrap();
            next_in += 1;
        }
        for _ in 0..burst {
            assert_eq!(rx.try_pop(), Some(next_out));
            next_out += 1;
        }
    }
}

struct CountsDrops(Arc<AtomicUsize>);

impl Drop for CountsDrops {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn values_still_in_the_queue_are_dropped_with_it_and_popped_ones_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let (mut tx, mut rx) = channel(8);
    for _ in 0..5 {
        tx.try_push(CountsDrops(Arc::clone(&drops))).ok().unwrap();
    }
    drop(rx.try_pop());
    drop(rx.try_pop());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(tx);
    drop(rx);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        5,
        "the three left are dropped"
    );
}

#[test]
fn two_threads_exchange_a_million_items_in_order() {
    const N: u64 = 1_000_000;
    let (mut tx, mut rx) = channel::<u64>(64);
    let producer = std::thread::spawn(move || {
        let mut i = 0;
        while i < N {
            if tx.try_push(i).is_ok() {
                i += 1;
            } else {
                std::thread::yield_now();
            }
        }
    });
    let mut expected = 0;
    while expected < N {
        match rx.try_pop() {
            Some(value) => {
                assert_eq!(value, expected);
                expected += 1;
            }
            None => std::thread::yield_now(),
        }
    }
    producer.join().unwrap();
}

#[test]
fn halves_can_move_between_threads() {
    fn is_send<T: Send>() {}
    is_send::<Producer<Vec<f32>>>();
    is_send::<Consumer<Vec<f32>>>();
}
