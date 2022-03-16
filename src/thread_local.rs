// Lock-free struct local thread local

use crate::map::{Map, PassthroughHasher};
use crate::{map::LiteHashMap, stack::LinkedRingBufferStack};
use std::alloc::System;
use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::*;
use std::{mem, ptr};

static GLOBAL_COUNTER: AtomicU64 = AtomicU64::new(0);
static FREE_LIST: LinkedRingBufferStack<u64, 64> = LinkedRingBufferStack::const_new();

thread_local! {
  static THREAD_META: ThreadMeta = ThreadMeta::new();
}

struct ThreadMeta {
    hash: u64,
}

const FAST_THREADS: usize = 512;

pub struct ThreadLocal<T> {
    fast_map: [Cell<usize>; FAST_THREADS],
    reserve_map: LiteHashMap<u64, usize, System, PassthroughHasher>,
    _marker: PhantomData<T>,
}

impl<T> ThreadLocal<T> {
    const OBJ_SIZE: usize = mem::size_of::<T>();

    pub fn new() -> Self {
        Self {
            fast_map: unsafe { mem::transmute([0usize; FAST_THREADS]) },
            reserve_map: LiteHashMap::with_capacity(num_cpus::get().next_power_of_two()),
            _marker: PhantomData,
        }
    }

    pub fn get_or<F: Fn() -> T>(&self, new: F) -> Option<&T> {
        unsafe {
            ThreadMeta::get_hash().map(|hash| {
                let idx = hash as usize;
                let obj_ptr = if idx < self.fast_map.len() {
                    let cell = &self.fast_map[idx];
                    if cell.get() == 0 {
                        let ptr = libc::malloc(Self::OBJ_SIZE) as *mut T;
                        ptr::write(ptr, new());
                        cell.set(ptr as usize);
                    }
                    cell.get()
                } else {
                    self.reserve_map.get_or_insert(&hash, move || {
                        let ptr = libc::malloc(Self::OBJ_SIZE) as *mut T;
                        ptr::write(ptr, new());
                        ptr as usize
                    })
                };
                &*(obj_ptr as *const T)
            })
        }
    }
}

impl ThreadMeta {
    fn new() -> Self {
        let hash = FREE_LIST
            .pop()
            .unwrap_or_else(|| GLOBAL_COUNTER.fetch_add(1, AcqRel));
        ThreadMeta { hash }
    }

    pub fn get_hash() -> Option<u64> {
        THREAD_META.try_with(|m| m.hash).ok()
    }
}

impl Drop for ThreadMeta {
    fn drop(&mut self) {
        FREE_LIST.push(self.hash);
    }
}

impl<T> Drop for ThreadLocal<T> {
    fn drop(&mut self) {
        for (_, v) in self.reserve_map.entries() {
            unsafe {
                libc::free(v as *mut libc::c_void);
            }
        }
        for cell in self.fast_map.iter() {
            let addr = cell.get();
            if addr == 0 {
                continue;
            }
            unsafe {
                libc::free(addr as *mut libc::c_void);
            }
            cell.set(0);
        }
    }
}

unsafe impl<T> Sync for ThreadLocal<T> {}
unsafe impl<T> Send for ThreadLocal<T> {}
