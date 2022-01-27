use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{
    AtomicU8,
    Ordering::{AcqRel, Relaxed, Release},
};

pub struct SpinLock<T> {
    mark: AtomicU8,
    obj: UnsafeCell<T>,
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub fn new(obj: T) -> Self {
        Self {
            mark: AtomicU8::new(0),
            obj: UnsafeCell::new(obj),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<T> {
        let backoff = crossbeam_utils::Backoff::new();
        while self.mark.compare_exchange(0, 1, AcqRel, Relaxed).is_err() {
            backoff.spin();
        }
        SpinLockGuard { lock: self }
    }
    pub fn try_lock(&self) -> Option<SpinLockGuard<T>> {
        if self.mark.compare_exchange(0, 1, AcqRel, Relaxed).is_ok() {
            Some(SpinLockGuard { lock: self })
        } else {
            None
        }
    }
}

impl<'a, T> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &'a Self::Target {
        unsafe { &*self.lock.obj.get() }
    }
}

impl<'a, T> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &'a mut T {
        unsafe { &mut *self.lock.obj.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.mark.store(0, Release);
    }
}

unsafe impl<T> Sync for SpinLock<T> {}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn lot_load_of_lock() {
        let lock = Arc::new(SpinLock::new(0));
        let num_threads = 32;
        let thread_turns = 2048;
        let mut threads = vec![];
        for _ in 0..num_threads {
            let lock = lock.clone();
            threads.push(thread::spawn(move || {
                for _ in 0..thread_turns {
                    *lock.lock() += 1;
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*lock.lock(), num_threads * thread_turns);
    }
}
