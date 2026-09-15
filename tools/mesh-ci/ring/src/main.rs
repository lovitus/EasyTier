//! Tests a dependency contract, not a replacement for real-Core benchmarks.
use async_ringbuf::{traits::*, AsyncHeapRb};
use futures::{executor::block_on, FutureExt, SinkExt, StreamExt};
use std::{cell::Cell, rc::Rc, time::Instant};

fn contract() {
    let (mut p, mut c) = AsyncHeapRb::<u64>::new(128).split();
    let ready = p.send(7).now_or_never();
    let queued = c.base().occupied_len();
    assert_eq!(c.try_pop(), Some(7));
    let flush_after_pop = p.flush().now_or_never();
    assert_eq!(flush_after_pop, Some(Ok(())));
    println!("ETCI_RING {{\"kind\":\"send_without_consumer_poll\",\"send_ready\":{},\"queued_before_pop\":{},\"flush_ready_after_pop\":true}}", ready.is_some(), queued);
}

fn run(batch: usize) {
    const N: u64 = 100_000;
    let (mut p, mut c) = AsyncHeapRb::<u64>::new(128).split();
    let batches = Rc::new(Cell::new(0u64));
    let max_batch = Rc::new(Cell::new(0usize));
    let b = batches.clone();
    let m = max_batch.clone();
    let start = Instant::now();
    block_on(async {
        futures::join!(
            async move {
                for i in 0..N {
                    if batch == 1 {
                        p.send(i).await.unwrap();
                    } else {
                        p.feed(i).await.unwrap();
                        if (i + 1) % batch as u64 == 0 { p.flush().await.unwrap(); }
                    }
                }
                p.flush().await.unwrap();
            },
            async move {
                let mut expected = 0;
                while let Some(v) = c.next().await {
                    assert_eq!(v, expected); expected += 1;
                    let mut n = 1usize;
                    while n < 64 {
                        let Some(v) = c.try_pop() else { break };
                        assert_eq!(v, expected); expected += 1; n += 1;
                    }
                    b.set(b.get() + 1); m.set(m.get().max(n));
                }
                assert_eq!(expected, N);
            }
        );
    });
    println!("ETCI_RING {{\"kind\":\"synthetic_handoff\",\"offered_batch\":{},\"items\":{},\"observed_batches\":{},\"max_observed_batch\":{},\"elapsed_ns\":{}}}", batch, N, batches.get(), max_batch.get(), start.elapsed().as_nanos());
}
fn main() {
    contract();
    for _ in 0..3 { run(1); run(32); }
}
