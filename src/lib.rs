//! A concurrent pool for lending and reusing items.
//!
//! This crate provides a [`LendPool`] type for managing reusable items in a concurrent or
//! asynchronous context. Items can be checked out ("loaned") as [`Loan`] guards and are
//! automatically returned to the pool when the guard is dropped.
//!
//! # Crate Features
//!
//! - **sync**  
//!   Enables the synchronous blocking method [`LendPool::loan_sync`], which will block the
//!   current thread until an item becomes available.
//!
//! - **async**  
//!   Enables the asynchronous method [`LendPool::loan_async`], which will `await` a notification
//!   until an item becomes available.
//!
//! # Examples
//!
//! ```rust
//! use lendpool::LendPool;
//!
//! // Create a new pool and add items
//! let pool = LendPool::new();
//! pool.add(1);
//! pool.add(2);
//!
//! // Loan an item (non-blocking)
//! if let Some(loan) = pool.loan() {
//!     println!("Got item: {}", *loan);
//! }; // The `Loan` is dropped here, returning items to the pool
//! ```

use crossbeam_queue::SegQueue;
use std::fmt::{Debug, Formatter};
use std::ops::{Deref, DerefMut};
use std::{fmt, mem};

use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "sync")]
use std::sync::{Condvar, Mutex};

#[cfg(feature = "async")]
use tokio::sync::Notify;

/// A thread-safe or async-aware pool of reusable items.
///
/// Items can be loaned out via [`loan`], [`loan_sync`], or [`loan_async`] (depending on which
/// features are enabled). Once a [`Loan`] is dropped, the corresponding item is returned to the
/// pool for future use.
///
/// # Feature Flags
///
/// - **sync** – Enables [`loan_sync`][Self::loan_sync] for blocking usage.
/// - **async** – Enables [`loan_async`][Self::loan_async] for asynchronous usage.
///
/// # Usage
///
/// 1. Create a `LendPool` using [`LendPool::new`].
/// 2. Add items to the pool using [`add`][Self::add].
/// 3. Loan items out using one of the `loan` methods.
/// 4. Use the [`Loan`] guard to access or mutate the underlying item.
/// 5. Drop or explicitly [`take`][Loan::take] the item. Dropping returns the item to the pool, while
///    `take` permanently removes it.
///
/// # Examples
///
/// ```rust
/// use lendpool::LendPool;
///
/// let pool = LendPool::new();
///
/// // Add items to the pool
/// pool.add("Hello".to_string());
/// pool.add("World".to_string());
///
/// // Loan an item non-blocking
/// if let Some(loan) = pool.loan() {
///     println!("Loaned item: {}", *loan);
/// };
/// ```
pub struct LendPool<T> {
    queue: SegQueue<T>,
    available: AtomicUsize,
    on_loan: AtomicUsize,

    #[cfg(feature = "sync")]
    _mutex: Mutex<()>,
    #[cfg(feature = "sync")]
    _condvar: Condvar,

    #[cfg(feature = "async")]
    _notify: Notify,
}

impl<T> Default for LendPool<T> {
    /// Creates a new, empty `LendPool`.
    ///
    /// This is equivalent to calling [`new`][Self::new].
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LendPool<T> {
    /// Creates a new, empty `LendPool`.
    ///
    /// The pool will initially contain zero items. Add items via [`add`][Self::add].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool: LendPool<i32> = LendPool::new();
    /// assert_eq!(pool.available(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            queue: SegQueue::new(),
            available: AtomicUsize::new(0),
            on_loan: AtomicUsize::new(0),

            #[cfg(feature = "sync")]
            _mutex: Mutex::new(()),
            #[cfg(feature = "sync")]
            _condvar: Condvar::new(),

            #[cfg(feature = "async")]
            _notify: Notify::new(),
        }
    }

    /// Adds a new item to this pool.
    ///
    /// # Behavior
    ///
    /// - Increments the count of [`available()`][Self::available].
    /// - Notifies any blocking or awaiting callers if necessary.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// {
    ///     let pool = LendPool::new();
    ///     pool.add(42);
    ///     assert_eq!(pool.available(), 1);
    /// };
    /// ```
    pub fn add(&self, item: T) {
        self.available.fetch_add(1, Ordering::SeqCst);
        self.queue.push(item);

        #[cfg(feature = "async")]
        {
            self._notify.notify_one();
        }

        #[cfg(feature = "sync")]
        {
            let _lock = self._mutex.lock().unwrap();
            self._condvar.notify_one();
        }
    }

    /// Attempts to loan out an item from the pool immediately (non-blocking).
    ///
    /// If an item is available, returns `Some(Loan)`. Otherwise, returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    ///
    /// let pool = LendPool::new();
    /// pool.add(10);
    ///
    /// if let Some(loan) = pool.loan() {
    ///     println!("Got item: {}", *loan);
    /// } else {
    ///     println!("No items available!");
    /// };
    /// ```
    pub fn loan(&self) -> Option<Loan<T>> {
        self.queue
            .pop()
            .map(|item| Loan {
                item: Some(item),
                lp: self,
            })
            .inspect(|_| {
                self.on_loan.fetch_add(1, Ordering::SeqCst);
            })
            .inspect(|_| {
                self.available.fetch_sub(1, Ordering::SeqCst);
            })
    }

    /// Returns the number of items currently in the pool and ready to be loaned.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// {
    ///     let pool = LendPool::new();
    ///     pool.add('a');
    ///     pool.add('b');
    ///     assert_eq!(pool.available(), 2);
    /// };
    /// ```
    pub fn available(&self) -> usize {
        self.available.load(Ordering::SeqCst)
    }

    /// Indicates whether there is at least one item available to be loaned out.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// {
    ///     let pool = LendPool::new();
    ///     pool.add(42);
    ///
    ///     if pool.is_available() {
    ///         println!("Pool has at least one item!");
    ///     }
    /// };
    /// ```
    pub fn is_available(&self) -> bool {
        self.available() >= 1
    }

    /// Returns the number of items currently checked out from this pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// {
    ///     let pool = LendPool::new();
    ///     pool.add(100);
    ///
    ///     {
    ///         let _loan = pool.loan().unwrap();
    ///         assert_eq!(pool.on_loan(), 1);
    ///     }
    ///
    ///     assert_eq!(pool.on_loan(), 0);
    /// };
    /// ```
    pub fn on_loan(&self) -> usize {
        self.on_loan.load(Ordering::SeqCst)
    }

    /// Indicates whether any items are currently on loan.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// {
    ///     pool.add(1);
    ///     assert!(!pool.is_loaned());
    ///
    ///     let loan = pool.loan().unwrap();
    ///     assert!(pool.is_loaned());
    /// }
    /// ```
    pub fn is_loaned(&self) -> bool {
        self.on_loan() != 0
    }

    /// Returns the total number of items managed by this pool.
    ///
    /// This is the sum of [`available()`][Self::available] + [`on_loan()`][Self::on_loan].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(10);
    /// pool.add(20);
    /// let _loan = pool.loan().unwrap();
    /// assert_eq!(pool.total(), 2);
    /// ```
    pub fn total(&self) -> usize {
        self.on_loan() + self.available()
    }
}

#[cfg(feature = "sync")]
impl<T> LendPool<T> {
    /// Loans out an item, blocking until one becomes available.
    ///
    /// This method requires the **sync** feature. It will acquire a lock, check for an available
    /// item, and if none is available, wait on a condition variable until notified.
    ///
    /// # Panics
    ///
    /// This function can panic if the mutex or condition variable is poisoned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// // Requires the "sync" feature to be enabled.
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(123);
    /// let loan = pool.loan_sync();
    /// println!("Loaned item: {}", *loan);
    /// ```
    pub fn loan_sync(&self) -> Loan<T> {
        loop {
            if let Some(loaned) = self.loan() {
                return loaned;
            }

            let _lock = self._mutex.lock().unwrap();
            let _wait = self._condvar.wait(_lock).unwrap();
        }
    }
}

#[cfg(feature = "async")]
impl<T> LendPool<T> {
    /// Loans out an item, `await`ing until one becomes available.
    ///
    /// This method requires the **async** feature. It will check for an available item, and if
    /// none is present, it will asynchronously wait on a [`tokio::sync::Notify`].
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Requires the "async" feature to be enabled.
    /// use tokio::runtime::Runtime;
    /// use lendpool::LendPool;
    ///
    /// fn main() {
    ///     let rt = Runtime::new().unwrap();
    ///     rt.block_on(async {
    ///         let pool = LendPool::new();
    ///         pool.add(10);
    ///
    ///         let loan = pool.loan_async().await;
    ///         println!("Loaned item: {}", *loan);
    ///     });
    /// }
    /// ```
    pub async fn loan_async(&self) -> Loan<T> {
        loop {
            if let Some(loan) = self.loan() {
                return loan;
            }

            self._notify.notified().await;
        }
    }
}

/// A guard representing a borrowed item from a [`LendPool`].
///
/// A [`Loan`] is returned by methods like [`LendPool::loan`] and automatically
/// returns the item to the pool upon being dropped. If you need to keep the item
/// permanently, call [`Loan::take`].
#[derive(Debug)]
pub struct Loan<'lp, T> {
    item: Option<T>,
    lp: &'lp LendPool<T>,
}

impl<T> Loan<'_, T> {
    /// Applies a function to the loaned item, transforming it in place.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(1);
    ///
    /// if let Some(mut loan) = pool.loan() {
    ///     loan.map(|val| val + 1);
    ///     assert_eq!(*loan, 2);
    /// };
    ///
    /// ```
    pub fn map(&mut self, f: impl FnOnce(T) -> T) {
        let item = self.item.take().expect("loan already consumed");
        self.item = Some(f(item));
    }

    /// Provides read-only access to the loaned item.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(100);
    ///
    /// if let Some(loan) = pool.loan() {
    ///    let x = loan.with(|val| *val + 1);
    ///    assert_eq!(x, 101);
    /// };
    /// ```
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(self.item.as_ref().expect("loan already consumed"))
    }

    /// Provides mutable access to the loaned item.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(String::from("Hello"));
    ///
    /// if let Some(mut loan) = pool.loan() {
    ///     loan.with_mut(|s| s.push_str(" World"));
    ///     assert_eq!(*loan, "Hello World");
    /// };
    /// ```
    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        f(self.item.as_mut().expect("loan already consumed"))
    }

    /// Permanently removes the item from the pool.
    ///
    /// This will decrement the on-loan count but **will not** add the item back to the pool.
    /// Useful if you need to keep the item or dispose of it without returning it to the pool.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed by a previous call to `take`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// pool.add(42);
    ///
    /// if let Some(loan) = pool.loan() {
    ///     let item = loan.take();
    ///     // `item` is now fully yours, and not returned to the pool.
    /// };
    ///
    /// assert_eq!(pool.available(), 0);
    /// assert_eq!(pool.on_loan(), 0);
    /// ```
    pub fn take(mut self) -> T {
        let item = self.item.take().expect("loan already consumed");
        self.lp.on_loan.fetch_sub(1, Ordering::SeqCst);
        item
    }

    /// Swaps items between two `Loan`s without returning them to the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    ///
    /// let pool = LendPool::new();
    /// {
    ///     pool.add('a');
    ///     pool.add('b');
    ///
    ///     let mut loan1 = pool.loan().unwrap();
    ///     let mut loan2 = pool.loan().unwrap();
    ///
    ///     loan1.swap_pool(&mut loan2);
    ///     // Now loan1 has what loan2 had, and vice versa.
    /// }
    /// ```
    pub fn swap_pool(&mut self, other: &mut Loan<'_, T>) {
        mem::swap(&mut self.item, &mut other.item)
    }
}

impl<'lp, T> Loan<'lp, T> {
    /// Moves the `Loan` into another [`LendPool`].
    pub fn move_pool(&mut self, pool: &'lp (impl PoolRef<'lp, T> + 'lp)) {
        self.lp = pool.pool_ref();
    }
}

impl<T> AsRef<T> for Loan<'_, T> {
    /// Returns a reference to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    fn as_ref(&self) -> &T {
        self.item.as_ref().expect("loan already consumed")
    }
}

impl<T> AsMut<T> for Loan<'_, T> {
    /// Returns a mutable reference to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    fn as_mut(&mut self) -> &mut T {
        self.item.as_mut().expect("loan already consumed")
    }
}

impl<T> Deref for Loan<'_, T> {
    type Target = T;

    /// Dereferences to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> DerefMut for Loan<'_, T> {
    /// Mutably dereferences to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`][Self::take].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl<T> Drop for Loan<'_, T> {
    /// Returns the loaned item to the pool on drop, unless it was [`take`][Self::take]n.
    ///
    /// This also decrements the on-loan count and enqueues the item back to the pool’s queue if
    /// present.
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            self.lp.queue.push(item);
            self.lp.on_loan.fetch_sub(1, Ordering::SeqCst);
            self.lp.available.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// A trait for types that can provide a reference to a [`LendPool`].
///
/// This is used by [`Loan::move_pool`] to move a loan from one pool reference to another
/// without returning or re-adding the item to the pool.
pub trait PoolRef<'lp, T> {
    /// Returns a reference to the underlying [`LendPool`].
    fn pool_ref(&'lp self) -> &'lp LendPool<T>;
}

impl<'lp, T> PoolRef<'lp, T> for LendPool<T> {
    fn pool_ref(&'lp self) -> &'lp LendPool<T> {
        self
    }
}

impl<'lp, T> PoolRef<'lp, T> for Loan<'lp, T> {
    fn pool_ref(&'lp self) -> &'lp LendPool<T> {
        self.lp
    }
}

impl<T> Debug for LendPool<T> {
    /// Formats the `LendPool` for debugging.
    ///
    /// Displays the number of [`available()`][Self::available] items,
    /// the count of items [`on_loan()`][Self::on_loan], and the [`total()`][Self::total].
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LendPool")
            .field("available", &self.available())
            .field("on_loan", &self.on_loan())
            .field("total", &self.total())
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Tests `LendPool::new`, `LendPool::available`, `LendPool::on_loan`, and `LendPool::total`.
    #[test]
    fn test_lendpool_new_and_counts() {
        let pool = LendPool::<i32>::new();
        assert_eq!(
            pool.available(),
            0,
            "Pool should start with zero available items"
        );
        assert_eq!(
            pool.on_loan(),
            0,
            "Pool should start with zero on-loan items"
        );
        assert_eq!(
            pool.total(),
            0,
            "Pool should have zero total items initially"
        );
    }

    /// Tests `LendPool::add` and verifies that items are truly added.
    #[test]
    fn test_lendpool_add() {
        let pool = LendPool::new();
        pool.add("test");
        assert_eq!(
            pool.available(),
            1,
            "One item should be available after adding"
        );
        assert_eq!(pool.on_loan(), 0, "No items on loan yet");
        assert_eq!(pool.total(), 1, "Total should be 1");
    }

    /// Tests `LendPool::loan` in a non-blocking scenario.
    #[test]
    fn test_lendpool_loan_non_blocking() {
        let pool = LendPool::new();
        pool.add(10);
        pool.add(20);

        // First loan
        let loan_opt = pool.loan();
        assert!(loan_opt.is_some(), "Should be able to loan an item");
        let loan = loan_opt.unwrap();
        assert!(
            *loan == 10 || *loan == 20,
            "Loaned item should be one of the added items"
        );
        assert_eq!(
            pool.available(),
            1,
            "One item left available after the first loan"
        );
        assert_eq!(pool.on_loan(), 1, "One item is on loan");
        assert_eq!(pool.total(), 2, "Total should remain 2");

        // Second loan
        let loan2 = pool.loan().expect("Another item should be available");
        assert!(
            *loan2 == 10 || *loan2 == 20,
            "Second loaned item should be the remaining one"
        );
        assert_ne!(
            *loan, *loan2,
            "Items should be different when two are added"
        );
        assert_eq!(pool.available(), 0, "No items left available");
        assert_eq!(pool.on_loan(), 2, "Two items on loan now");

        // When both loans drop, items return to the pool
        drop(loan);
        drop(loan2);
        assert_eq!(
            pool.available(),
            2,
            "Items should return after dropping loans"
        );
        assert_eq!(
            pool.on_loan(),
            0,
            "No items on loan after dropping all loans"
        );
    }

    /// Tests `LendPool::is_available` and `LendPool::is_loaned`.
    #[test]
    fn test_lendpool_is_available_and_is_loaned() {
        let pool = LendPool::new();
        assert!(!pool.is_available(), "No items are in the pool yet");
        assert!(!pool.is_loaned(), "No items are on loan yet");

        pool.add(1);
        assert!(pool.is_available(), "Pool has at least one item");
        assert!(!pool.is_loaned(), "No items loaned out yet");

        let loan = pool.loan().unwrap();
        assert!(!pool.is_available(), "No items left in the pool after loan");
        assert!(pool.is_loaned(), "Now at least one item is on loan");

        drop(loan);
        assert!(pool.is_available(), "Item should be back in pool");
        assert!(!pool.is_loaned(), "No items on loan after dropping");
    }

    /// Tests `Loan::map`, `Loan::with`, and `Loan::with_mut`.
    #[test]
    fn test_loan_transformations() {
        let pool = LendPool::new();
        pool.add(5);
        let mut loan = pool.loan().expect("Should have an item to loan");

        // Test `Loan::with` for read-only access
        let doubled = loan.with(|val| *val * 2);
        assert_eq!(doubled, 10);

        // Test `Loan::with_mut` for mutable access
        loan.with_mut(|val| *val += 10);
        assert_eq!(*loan, 15);

        // Test `Loan::map` to transform the item
        loan.map(|val| val * 2);
        assert_eq!(*loan, 30);

        // Dropping returns the item to the pool
        drop(loan);
        assert_eq!(
            pool.available(),
            1,
            "Item should return after dropping loan"
        );
        assert_eq!(pool.on_loan(), 0);
    }

    /// Tests `Loan::take` to permanently remove an item from the pool.
    #[test]
    fn test_loan_take() {
        let pool = LendPool::new();
        pool.add("Hello");
        assert_eq!(pool.total(), 1);

        {
            let loan = pool.loan().expect("Item should be available");
            let item = loan.take();
            assert_eq!(item, "Hello");

            // After calling `take`, nothing should be returned to the pool
            assert_eq!(pool.on_loan(), 0, "No items should be on loan after take");
            assert_eq!(pool.available(), 0, "Item is not returned to the pool");
            assert_eq!(pool.total(), 0, "Pool total is effectively zero now");
        }

        // Double-check the pool after the loan scope ends
        assert_eq!(pool.available(), 0, "Still no items in the pool");
        assert_eq!(pool.on_loan(), 0);
        assert_eq!(pool.total(), 0);
    }

    /// Tests `Loan::swap_pool` by swapping items from two different loans.
    #[test]
    fn test_loan_swap_pool() {
        let pool = LendPool::new();
        pool.add('x');
        pool.add('y');

        let mut loan1 = pool.loan().unwrap();
        let mut loan2 = pool.loan().unwrap();
        let val1 = *loan1;
        let val2 = *loan2;

        // Swap them
        loan1.swap_pool(&mut loan2);
        assert_eq!(*loan1, val2);
        assert_eq!(*loan2, val1);

        // Drop them in reverse order
        drop(loan1);
        drop(loan2);

        assert_eq!(pool.available(), 2, "Both items returned to the pool");
        assert_eq!(pool.on_loan(), 0);
    }

    /// Tests `AsRef`, `AsMut`, `Deref`, and `DerefMut` from `Loan`.
    #[test]
    fn test_loan_as_ref_and_deref() {
        let pool = LendPool::new();
        pool.add(String::from("Hello World"));
        let mut loan = pool.loan().expect("Should have a string to loan");

        // Test `AsRef<T>`
        assert_eq!(loan.as_ref(), "Hello World");

        // Test `AsMut<T>`
        loan.as_mut().push_str("!!!");
        assert_eq!(loan.as_ref(), "Hello World!!!");

        // Test `Deref`
        assert_eq!(&*loan, "Hello World!!!");

        // Test `DerefMut`
        loan.push_str("???");
        assert_eq!(&*loan, "Hello World!!!???");
    }

    /// Tests that dropping a `Loan` re-enqueues the item, using `Drop` impl.
    #[test]
    fn test_loan_drop() {
        let pool = LendPool::new();
        pool.add(99);

        assert_eq!(pool.available(), 1);
        assert_eq!(pool.on_loan(), 0);

        {
            let _loan = pool.loan().expect("Should be able to loan item");
            assert_eq!(pool.available(), 0);
            assert_eq!(pool.on_loan(), 1);
        } // Loan drops here

        assert_eq!(pool.available(), 1, "Item is returned to the pool on drop");
        assert_eq!(pool.on_loan(), 0);
    }

    /// Tests the `Debug` implementation for `LendPool`.
    #[test]
    fn test_lendpool_debug() {
        let pool = LendPool::new();
        pool.add(1);
        let debug_str = format!("{:?}", pool);

        assert!(
            debug_str.contains("LendPool"),
            "Debug output should contain 'LendPool'"
        );
        assert!(
            debug_str.contains("available"),
            "Debug output should mention 'available'"
        );
        assert!(
            debug_str.contains("on_loan"),
            "Debug output should mention 'on_loan'"
        );
        assert!(
            debug_str.contains("total"),
            "Debug output should mention 'total'"
        );
    }

    /// If you have the "sync" feature enabled, test the blocking method `loan_sync`.
    #[cfg(feature = "sync")]
    #[test]
    fn test_lendpool_loan_sync() {
        let pool = LendPool::new();
        pool.add(123);

        let loan = pool.loan_sync();
        assert_eq!(*loan, 123);
        assert_eq!(pool.available(), 0);
        assert_eq!(pool.on_loan(), 1);

        // Once dropped, the item is returned
        drop(loan);
        assert_eq!(pool.available(), 1);
        assert_eq!(pool.on_loan(), 0);
    }

    /// If you have the "async" feature enabled, test the async method `loan_async`.
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_lendpool_loan_async() {
        let pool = LendPool::new();
        pool.add("hello");

        let loan = pool.loan_async().await;
        assert_eq!(*loan, "hello");
        assert_eq!(pool.available(), 0);
        assert_eq!(pool.on_loan(), 1);

        // Once dropped, the item is returned
        drop(loan);
        assert_eq!(pool.available(), 1);
        assert_eq!(pool.on_loan(), 0);
    }

    #[test]
    fn test_loan_with_method() {
        let pool = LendPool::new();
        pool.add(42);

        // Loan an item
        let loan = pool.loan().expect("Should be able to loan an item");
        assert_eq!(pool.available(), 0, "Item is now on loan");
        assert_eq!(pool.on_loan(), 1);

        // Use `with` to read the item without consuming it
        let plus_one = loan.with(|val| *val + 1);
        assert_eq!(
            plus_one, 43,
            "`with` should apply the function to the borrowed item"
        );

        // Verify that the item remains on loan and has not been consumed
        assert_eq!(pool.available(), 0);
        assert_eq!(pool.on_loan(), 1);

        // Dropping the loan returns the item to the pool
        drop(loan);
        assert_eq!(
            pool.available(),
            1,
            "Item should be returned after dropping the loan"
        );
        assert_eq!(pool.on_loan(), 0);
    }
}
