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
//! }
//!
//! // Once `loan` is dropped, the item is returned to the pool automatically.
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
/// - **sync** – Enables `loan_sync` for blocking usage.
/// - **async** – Enables `loan_async` for asynchronous usage.
///
/// # Usage
///
/// 1. Create a `LendPool` using [`new`].
/// 2. Add items to the pool using [`add`].
/// 3. Loan items out using one of the `loan` methods.
/// 4. Use the [`Loan`] guard to access or mutate the underlying item.
/// 5. Drop or explicitly [`take`] the item. Dropping returns the item to the pool, while
///    `take` permanently removes it.
///
/// # Examples
///
/// ```rust
/// use lendpool::LendPool;
/// let pool = LendPool::new();
///
/// // Add items to the pool
/// pool.add("Hello".to_string());
/// pool.add("World".to_string());
///
/// // Loan an item non-blocking
/// if let Some(loan) = pool.loan() {
///     println!("Loaned item: {}", *loan);
/// }
///
/// // Item is automatically returned when loan goes out of scope
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
    /// This is equivalent to calling [`new`].
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LendPool<T> {
    /// Creates a new, empty `LendPool`.
    ///
    /// The pool will initially contain zero items. Add items via [`add`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
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
    /// - Increments the count of [`available`] items.
    /// - Notifies any blocking or awaiting callers if necessary.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(42);
    /// assert_eq!(pool.available(), 1);
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

    /// Attempts to loan out an item from the pool immediately.
    ///
    /// This method does **not** block or wait. If an item is not immediately available,
    /// it returns `None`.
    ///
    /// # Returns
    ///
    /// - [`Some(Loan)`] if the pool has an available item.
    /// - [`None`] if the pool is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(10);
    ///
    /// if let Some(loan) = pool.loan() {
    ///     println!("Got item: {}", *loan);
    /// } else {
    ///     println!("No items available!");
    /// }
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
    /// let pool = LendPool::new();
    /// pool.add('a');
    /// pool.add('b');
    /// assert_eq!(pool.available(), 2);
    /// ```
    pub fn available(&self) -> usize {
        self.available.load(Ordering::SeqCst)
    }

    /// Indicates whether there is at least one item available to be loaned out.
    ///
    /// Returns `true` if [`available`] > 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(42);
    ///
    /// if pool.is_available() {
    ///     println!("Pool has at least one item!");
    /// }
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
    /// let pool = LendPool::new();
    /// pool.add(100);
    /// {
    ///     let loan = pool.loan().unwrap();
    ///     assert_eq!(pool.on_loan(), 1);
    /// }
    /// assert_eq!(pool.on_loan(), 0);
    /// ```
    pub fn on_loan(&self) -> usize {
        self.on_loan.load(Ordering::SeqCst)
    }

    /// Indicates whether any items are currently on loan.
    ///
    /// Returns `true` if [`on_loan`] > 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(1);
    /// assert!(!pool.is_loaned());
    ///
    /// let loan = pool.loan().unwrap();
    /// assert!(pool.is_loaned());
    /// ```
    pub fn is_loaned(&self) -> bool {
        self.on_loan() != 0
    }

    /// Returns the total number of items managed by this pool.
    ///
    /// This is the sum of [`available`] + [`on_loan`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
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
    /// use lend_pool::LendPool;
    ///
    /// let rt = Runtime::new().unwrap();
    /// rt.block_on(async {
    ///     let pool = LendPool::new();
    ///     pool.add(10);
    ///
    ///     let loan = pool.loan_async().await;
    ///     println!("Loaned item: {}", *loan);
    /// });
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
/// permanently, call [`take`] on the [`Loan`].
///
/// [`take`]: Loan::take
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
    /// Panics if the item has already been consumed via [`take`].
    ///
    /// # Examples
    ///
    /// ```
    /// let pool = LendPool::new();
    /// pool.add(1);
    ///
    /// if let Some(mut loan) = pool.loan() {
    ///     loan.map(|val| val + 1);
    ///     assert_eq!(*loan, 2);
    /// }
    /// ```
    pub fn map(&mut self, f: impl FnOnce(T) -> T) {
        let item = self.item.take().expect("loan already consumed");
        self.item = Some(f(item));
    }

    /// Provides read-only access to the loaned item.
    ///
    /// The closure receives an immutable reference to the item and returns a result `R`.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(100);
    ///
    /// if let Some(loan) = pool.loan() {
    ///     let x = loan.with(|val| *val + 1);
    ///     assert_eq!(x, 101);
    /// }
    /// ```
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(self.item.as_ref().expect("loan already consumed"))
    }

    /// Provides mutable access to the loaned item.
    ///
    /// The closure receives a mutable reference to the item and returns a result `R`.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(String::from("Hello"));
    ///
    /// if let Some(mut loan) = pool.loan() {
    ///     loan.with_mut(|s| s.push_str(" World"));
    ///     assert_eq!(*loan, "Hello World");
    /// }
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
    /// let pool = LendPool::new();
    /// pool.add(42);
    ///
    /// if let Some(loan) = pool.loan() {
    ///     let item = loan.take();
    ///     // `item` is now fully yours, and not returned to the pool.
    /// }
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
    /// let pool = LendPool::new();
    /// pool.add('a');
    /// pool.add('b');
    ///
    /// let mut loan1 = pool.loan().unwrap();
    /// let mut loan2 = pool.loan().unwrap();
    ///
    /// loan1.swap_pool(&mut loan2);
    /// // Now loan1 has what loan2 had, and vice versa.
    /// ```
    pub fn swap_pool(&mut self, other: &mut Loan<'_, T>) {
        mem::swap(&mut self.item, &mut other.item)
    }
}

impl<'lp, T> Loan<'lp, T> {
    /// Moves the `Loan` into another type that references the same [`LendPool`].
    ///
    /// This can be useful if you have a wrapped type that implements [`PoolRef`], and you
    /// want this `Loan` to be associated with that reference instead.
    ///
    /// # Safety
    ///
    /// You must ensure the target reference points to the same `LendPool` to avoid undefined
    /// behavior.
    ///
    /// # Examples
    ///
    /// ```
    /// use lendpool::LendPool;
    /// let pool = LendPool::new();
    /// pool.add(10);
    ///
    /// if let Some(mut loan) = pool.loan() {
    ///     // This is a contrived example, normally you'd have a distinct type implementing `PoolRef`.
    ///     loan.move_pool(&loan);
    /// }
    /// ```
    pub fn move_pool(&mut self, pool: &'lp (impl PoolRef<'lp, T> + 'lp)) {
        self.lp = pool.pool_ref();
    }
}

impl<T> AsRef<T> for Loan<'_, T> {
    /// Returns a reference to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`].
    fn as_ref(&self) -> &T {
        self.item.as_ref().expect("loan already consumed")
    }
}

impl<T> AsMut<T> for Loan<'_, T> {
    /// Returns a mutable reference to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`].
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
    /// Panics if the item has already been consumed via [`take`].
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> DerefMut for Loan<'_, T> {
    /// Mutably dereferences to the item under loan.
    ///
    /// # Panics
    ///
    /// Panics if the item has already been consumed via [`take`].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl<T> Drop for Loan<'_, T> {
    /// Returns the loaned item to the pool on drop, unless it was [`take`]n.
    ///
    /// This also decrements the on-loan count and enqueues the item back to the pool’s queue if
    /// present.
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            self.lp.queue.push(item);
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
    /// Displays the number of `available` items, the count of items `on_loan`, and the `total`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LendPool")
            .field("available", &self.available())
            .field("on_loan", &self.on_loan())
            .field("total", &self.total())
            .finish()
    }
}
