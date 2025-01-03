# LendPool

LendPool is a simple library for allowing safe and concurrent access to a group of objects across threads.
It achieves this with a [`Loan<T>`](https://docs.rs/lendpool/latest/struct.Notify.html) guard. 

# Example Usage
```rust
use lendpool::LendPool;

fn main() {
    // create a new pool
    let pool = LendPool::new();

    // add items to the pool
    pool.add("Resource 1".to_string());
    pool.add("Resource 2".to_string());

    // attempt to loan an item (non-blocking)
    if let Some(loan) = pool.loan() {
        println!("Borrowed item: {}", *loan);

        // you can transform the borrowed item
        loan.with_mut(|val| val.push_str(" - Modified"));

        // access the modified value
        println!("Modified item: {}", *loan);
    }; // loan is dropped here, returning the item to the pool

    // loan another item
    if let Some(mut loan) = pool.loan() {
        // permanently take the item from the pool
        let item = loan.take();
        println!("Took item: {}", item);
    };

    // check the pool's status
    println!("Available items: {}", pool.available());
    println!("Items on loan: {}", pool.on_loan());
    println!("Total items: {}", pool.total());
}
```

# Feature Flags
- sync: allows blocking until a resource is available
- async: allows waiting on the pool until a resource is available


# Dependencies
LendPool is based on [`crossbeam_queue::SegQueue`](https://docs.rs/crossbeam/latest/crossbeam/queue/struct.SegQueue.html).
The `async` feature uses [`tokio::sync::Notify`](https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html) to handle waiting.


