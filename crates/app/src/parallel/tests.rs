use std::panic;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::{map_in_order, map_with, threads};

// No address space holds a stack this large, so the OS refuses every helper thread.
fn refused() -> thread::Builder {
    thread::Builder::new().stack_size(1 << 60)
}

#[test]
fn refused_helpers_leave_every_item_to_the_caller_in_input_order() {
    let items: Vec<u32> = (0..32).collect();
    let caller = thread::current().id();

    let found = map_with(&items, 8, |item| (*item, thread::current().id()), refused);

    let expected: Vec<_> = items.iter().map(|item| (*item, caller)).collect();
    assert_eq!(found, expected);
}

#[test]
fn the_pool_size_is_from_one_to_eight() {
    assert!((1..=8).contains(&threads()), "{}", threads());
}

#[test]
fn every_thread_count_gives_the_results_in_input_order() {
    let items: Vec<u64> = (0..100).collect();
    let serial: Vec<u64> = items.iter().map(|item| item * item + 1).collect();
    for count in 0..=12 {
        let found = map_in_order(&items, count, |item| item * item + 1);
        assert_eq!(found, serial, "threads: {count}");
    }
}

#[test]
fn each_item_is_mapped_once() {
    let items: Vec<usize> = (0..257).collect();
    let calls = AtomicUsize::new(0);

    let found = map_in_order(&items, 8, |item| {
        calls.fetch_add(1, Ordering::Relaxed);
        *item
    });

    assert_eq!(found, items);
    assert_eq!(calls.load(Ordering::Relaxed), items.len());
}

#[test]
fn every_item_runs_on_a_thread_with_the_caller_name() {
    let items: Vec<u32> = (0..64).collect();
    let caller = thread::Builder::new().name("erosion-lint".to_owned());

    let names = caller
        .spawn(move || map_in_order(&items, 8, |_| thread::current().name().map(str::to_owned)))
        .expect("the caller starts")
        .join()
        .expect("the caller ends");

    assert!(
        names
            .iter()
            .all(|name| name.as_deref() == Some("erosion-lint"))
    );
}

#[test]
fn an_empty_slice_maps_to_an_empty_list() {
    let found: Vec<u8> = map_in_order(&[] as &[u8], 8, |item| *item);
    assert!(found.is_empty());
}

#[test]
fn a_panic_in_a_worker_reaches_the_caller_with_its_payload() {
    let items: Vec<u32> = (0..16).collect();

    let caught = panic::catch_unwind(|| {
        map_in_order(&items, 4, |item| {
            assert_ne!(*item, 11, "item eleven");
            *item
        })
    });

    let payload = caught.expect_err("the panic reaches the caller");
    let message = payload
        .downcast_ref::<String>()
        .expect("a formatted message");
    assert!(message.contains("item eleven"), "{message}");
}
