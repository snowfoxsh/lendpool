use crossbeam_queue::SegQueue;
use std::fmt::{Debug, Formatter};
use std::ops::{Deref, DerefMut};
use std::{fmt, mem};

use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "sync")]
use std::sync::{Condvar, Mutex};

#[cfg(feature = "async")]
use tokio::sync::Notify;

pub struct LoanPool<T> {
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

impl<T> Default for LoanPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LoanPool<T> {
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

    pub fn available(&self) -> usize {
        self.available.load(Ordering::SeqCst)
    }

    pub fn is_available(&self) -> bool {
        self.available() >= 1
    }

    pub fn on_loan(&self) -> usize {
        self.on_loan.load(Ordering::SeqCst)
    }

    pub fn is_loaned(&self) -> bool {
        self.on_loan() != 0
    }

    pub fn total(&self) -> usize {
        self.on_loan() + self.available()
    }
}

#[cfg(feature = "sync")]
impl<T> LoanPool<T> {
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
impl<T> LoanPool<T> {
    pub async fn loan_async(&self) -> Loan<T> {
        loop {
            if let Some(loan) = self.loan() {
                return loan;
            }

            // wait if no item is available
            self._notify.notified().await
        }
    }
}

pub struct Loan<'lp, T> {
    item: Option<T>,
    lp: &'lp LoanPool<T>,
}

impl<T> Loan<'_, T> {
    pub fn map(&mut self, f: impl FnOnce(T) -> T) {
        let item = self.item.take().expect("loan already consumed");
        self.item = Some(f(item));
    }

    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(self.item.as_ref().expect("loan already consumed"))
    }

    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        f(self.item.as_mut().expect("loan already consumed"))
    }

    pub fn take(mut self) -> T {
        let item = self.item.take().expect("loan already consumed");
        self.lp.on_loan.fetch_sub(1, Ordering::SeqCst);
        item
    }

    pub fn swap_pool(&mut self, other: &mut Loan<'_, T>) {
        mem::swap(&mut self.item, &mut other.item)
    }
}

impl<'lp, T> Loan<'lp, T> {
    pub fn move_pool(&mut self, pool: &'lp (impl PoolRef<'lp, T> + 'lp)) {
        self.lp = pool.pool_ref();
    }
}

impl<T> AsRef<T> for Loan<'_, T> {
    fn as_ref(&self) -> &T {
        self.item.as_ref().expect("loan already consumed")
    }
}

impl<T> Deref for Loan<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> AsMut<T> for Loan<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        self.item.as_mut().expect("loan already consumed")
    }
}

impl<T> DerefMut for Loan<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.item.as_mut().expect("loan already consumed")
    }
}

impl<T> Drop for Loan<'_, T> {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            self.lp.queue.push(item)
        };
    }
}

pub trait PoolRef<'lp, T> {
    fn pool_ref(&'lp self) -> &'lp LoanPool<T>;
}

impl<'lp, T> PoolRef<'lp, T> for LoanPool<T> {
    fn pool_ref(&'lp self) -> &'lp LoanPool<T> {
        self
    }
}

impl<'lp, T> PoolRef<'lp, T> for Loan<'lp, T> {
    fn pool_ref(&'lp self) -> &'lp LoanPool<T> {
        self.lp
    }
}

impl<T> Debug for LoanPool<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoanPool")
            .field("available", &self.available())
            .field("on_loan", &self.on_loan())
            .field("total", &self.total())
            .finish()
    }
}
