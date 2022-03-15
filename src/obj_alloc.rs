// A supposed to be fast and lock-free object allocator without size class

use std::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::{self, MaybeUninit},
    sync::{
        atomic::{AtomicU8, AtomicUsize},
        Arc,
    },
};

use crate::{ring_buffer::ACQUIRED, thread_local::ThreadLocal};
use crossbeam_epoch::{Atomic, Guard, Owned, Shared};
use std::sync::atomic::Ordering::Relaxed;

use crate::{
    ring_buffer::RingBuffer,
    stack::{LinkedRingBufferStack, RingBufferNode},
};

pub struct Allocator<T, const B: usize> {
    shared: Arc<SharedAlloc<T, B>>,
    thread: ThreadLocal<TLAlloc<T, B>>,
}

pub struct TLAlloc<T, const B: usize> {
    inner: UnsafeCell<TLAllocInner<T, B>>,
}

pub struct TLAllocInner<T, const B: usize> {
    buffer: usize,
    buffer_limit: usize,
    free_list: TLBufferedStack<usize, B>,
    shared: Arc<SharedAlloc<T, B>>,
    guard_count: usize,
    defer_free: Vec<usize>,
    _marker: PhantomData<T>,
}

pub struct SharedAlloc<T, const B: usize> {
    free_obj: LinkedRingBufferStack<usize, B>,
    free_buffer: LinkedRingBufferStack<(usize, usize), B>,
    all_buffers: LinkedRingBufferStack<usize, B>,
    _marker: PhantomData<T>,
}

impl<T, const B: usize> Allocator<T, B> {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(SharedAlloc::new()),
            thread: ThreadLocal::new(),
        }
    }

    pub fn alloc(&self) -> *mut T {
        let tl_alloc = self.tl_alloc();
        unsafe {
            let alloc_ref = &mut *tl_alloc.get();
            alloc_ref.alloc() as *mut T
        }
    }

    pub fn free(&self, addr: *mut T) {
        let tl_alloc = self.tl_alloc();

        unsafe {
            let alloc_ref = &mut *tl_alloc.get();
            alloc_ref.free(addr as usize)
        }
    }

    pub fn pin(&self) -> AllocGuard<T, B> {
        let tl_alloc = self.tl_alloc();
        unsafe {
            let alloc_ref = &mut *tl_alloc.get();
            alloc_ref.guard_count += 1;
            AllocGuard {
                alloc: tl_alloc.get(),
            }
        }
    }

    #[inline(always)]
    fn tl_alloc(&self) -> &TLAlloc<T, B> {
        self.thread
            .get_or(|| TLAlloc::new(0, 0, self.shared.clone()))
    }
}

impl<T, const B: usize> SharedAlloc<T, B> {
    const OBJ_SIZE: usize = mem::size_of::<T>();
    const BUMP_SIZE: usize = 4096 * Self::OBJ_SIZE;

    fn new() -> Self {
        Self {
            free_obj: LinkedRingBufferStack::new(),
            free_buffer: LinkedRingBufferStack::new(),
            all_buffers: LinkedRingBufferStack::new(),
            _marker: PhantomData,
        }
    }

    fn alloc_buffer(&self) -> (usize, usize) {
        if let Some(pair) = self.free_buffer.pop() {
            return pair;
        }
        let ptr = unsafe { libc::malloc(Self::BUMP_SIZE) } as usize;
        self.all_buffers.push(ptr);
        (ptr, ptr + Self::BUMP_SIZE)
    }

    fn free_objs<'a>(&self, guard: &'a Guard) -> Option<Shared<'a, RingBufferNode<usize, B>>> {
        self.free_obj.pop_buffer(guard)
    }
}

impl<T, const B: usize> TLAllocInner<T, B> {
    const OBJ_SIZE: usize = mem::size_of::<T>() as usize;

    pub fn new(buffer: usize, limit: usize, shared: Arc<SharedAlloc<T, B>>) -> Self {
        Self {
            free_list: TLBufferedStack::new(),
            buffer,
            buffer_limit: limit,
            shared,
            guard_count: 0,
            defer_free: Vec::with_capacity(64),
            _marker: PhantomData,
        }
    }

    pub fn alloc(&mut self) -> usize {
        if let Some(addr) = self.free_list.pop() {
            return addr;
        }
        if self.buffer + Self::OBJ_SIZE > self.buffer_limit {
            // Allocate new buffer
            let guard = crossbeam_epoch::pin();
            if let Some(mut new_free_buffer) = self.shared.free_objs(&guard) {
                let mut free_buffer = unsafe { new_free_buffer.deref_mut() };
                if let Some(ptr) = free_buffer.buffer.pop_back() {
                    let tl_node = Box::new(TLBufferNode {
                        elements: free_buffer.buffer.elements.clone(),
                        pos: free_buffer.buffer.tail.load(Relaxed),
                        next: 0 as *mut TLBufferNode<usize, B>
                    });
                    debug_assert_eq!(self.free_list.num_buffer, 0);
                    free_buffer.next = Atomic::null();
                    self.free_list.head = Box::into_raw(tl_node);
                    self.free_list.num_buffer += 1;
                    unsafe {
                        guard.defer_destroy(new_free_buffer);
                    }
                    return ptr;
                }
            }
            let (new_buffer, new_limit) = self.shared.alloc_buffer();
            self.buffer = new_buffer;
            self.buffer_limit = new_limit;
        }
        let obj_addr = self.buffer;
        self.buffer += Self::OBJ_SIZE;
        debug_assert!(self.buffer <= self.buffer_limit);
        return obj_addr;
    }

    pub fn free(&mut self, addr: usize) {
        if let Some(overflow_buffer) = self.free_list.push(addr) {
            let ring_buffer_node =
                unsafe { Box::from_raw(overflow_buffer).into_ring_buffer_node() };
            let guard = crossbeam_epoch::pin();
            self.shared
                .free_obj
                .attach_buffer(Owned::new(ring_buffer_node).into_shared(&guard), &guard);
        }
    }

    pub fn return_resources(&mut self) {
        // Return buffer space
        if self.buffer != self.buffer_limit {
            self.shared
                .free_buffer
                .push((self.buffer, self.buffer_limit))
        }

        // return free list
        let local_free = &mut self.free_list;
        let head = local_free.head;
        let guard = crossbeam_epoch::pin();
        if !head.is_null() {
            unsafe {
                let head = Box::from_raw(head);
                let mut next_ptr = head.next;
                self.shared.free_obj.attach_buffer(
                    Owned::new(head.into_ring_buffer_node()).into_shared(&guard),
                    &guard,
                );
                while !next_ptr.is_null() {
                    let next = Box::from_raw(next_ptr);
                    let next_next = next.next;
                    if next.pos > 0 {
                        self.shared.free_obj.attach_buffer(
                            Owned::new(next.into_ring_buffer_node()).into_shared(&guard),
                            &guard,
                        );
                    }
                    assert_ne!(next_ptr, next_next);
                    next_ptr = next_next;
                }
            }
        }
    }
}

unsafe impl<T, const B: usize> Send for TLAllocInner<T, B> {}

impl<T, const B: usize> Drop for TLAllocInner<T, B> {
    fn drop(&mut self) {
        self.return_resources();
    }
}

struct TLBufferNode<T, const B: usize> {
    elements: [Cell<T>; B],
    pos: usize,
    next: *mut Self,
}

struct TLBufferedStack<T, const B: usize> {
    head: *mut TLBufferNode<T, B>,
    num_buffer: usize,
}

impl<T: Clone + Default, const B: usize> TLBufferedStack<T, B> {
    const MAX_BUFFERS: usize = 32;
    pub fn new() -> Self {
        Self {
            head: 0 as *mut TLBufferNode<T, B>,
            num_buffer: 0,
        }
    }

    pub fn push(&mut self, val: T) -> Option<*mut TLBufferNode<T, B>> {
        unsafe {
            let mut res = None;
            if self.head.is_null() {
                self.head = Box::into_raw(Box::new(TLBufferNode {
                    elements: unsafe { MaybeUninit::uninit().assume_init() },
                    pos: 0,
                    next: 0 as *mut TLBufferNode<T, B>,
                }));
            }
            if let Err(val) = (&mut *self.head).push(val) {
                // Current buffer is full, need a new one
                if self.num_buffer >= Self::MAX_BUFFERS {
                    let overflow_buffer_node = self.head;
                    let next_head = (&*self.head).next;
                    self.head = next_head;
                    res = Some(overflow_buffer_node);
                    self.num_buffer -= 1;
                }
                debug_assert!(!self.head.is_null());
                let mut new_buffer = Box::new(TLBufferNode {
                    elements: unsafe { MaybeUninit::uninit().assume_init() },
                    pos: 0,
                    next: self.head,
                });
                let _ = new_buffer.push(val);
                let new_ptr = Box::into_raw(new_buffer);
                debug_assert_ne!(self.head, new_ptr);
                self.head = new_ptr;
                self.num_buffer += 1;
            }
            return res;
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.head.is_null() {
            return None;
        }
        unsafe {
            loop {
                let head_pop = (&mut *self.head).pop();
                if head_pop.is_some() {
                    return head_pop;
                }
                // Need to pop from next buffer
                let next_buffer = (&*self.head).next;
                if next_buffer.is_null() {
                    return None;
                }
                debug_assert_ne!(self.head, next_buffer);
                let old_head = mem::replace(&mut self.head, next_buffer);
                self.num_buffer -= 1;
                Box::from_raw(old_head); // May need to optimize this
            }
        }
    }
}

impl<T: Default, const B: usize> TLBufferNode<T, B> {
    fn push(&mut self, val: T) -> Result<(), T> {
        if self.pos >= self.elements.len() {
            return Err(val);
        } else {
            self.elements[self.pos].set(val);
            self.pos += 1;
            return Ok(());
        }
    }

    fn pop(&mut self) -> Option<T> {
        if self.pos == 0 {
            return None;
        } else {
            self.pos -= 1;
            let val = mem::take(self.elements[self.pos].get_mut());
            return Some(val);
        }
    }

    fn into_ring_buffer_node(self) -> RingBufferNode<T, B> {
        let mut flags: [AtomicU8; B] = unsafe { MaybeUninit::uninit().assume_init() };
        for f in &mut flags[0..self.pos] {
            *f = AtomicU8::new(ACQUIRED)
        }
        RingBufferNode {
            buffer: RingBuffer {
                head: AtomicUsize::new(0),
                tail: AtomicUsize::new(self.pos),
                elements: self.elements,
                flags,
            },
            next: Atomic::null(),
        }
    }
}

pub struct AllocGuard<T, const B: usize> {
    alloc: *mut TLAllocInner<T, B>,
}

impl<'a, T, const B: usize> Drop for AllocGuard<T, B> {
    fn drop(&mut self) {
        let alloc = unsafe { &mut *self.alloc };
        alloc.guard_count -= 1;
        if alloc.guard_count == 0 {
            while let Some(ptr) = alloc.defer_free.pop() {
                alloc.free(ptr);
            }
        }
    }
}

impl<'a, T, const B: usize> AllocGuard<T, B> {
    pub fn defer_free(&self, ptr: *mut T) {
        let alloc = unsafe { &mut *self.alloc };
        alloc.defer_free.push(ptr as usize);
    }

    pub fn alloc(&self) -> *mut T {
        let alloc = unsafe { &mut *self.alloc };
        alloc.alloc() as *mut T
    }

    pub fn free(&self, addr: usize) {
        let alloc = unsafe { &mut *self.alloc };
        alloc.free(addr)
    }
}

impl<T, const B: usize> Drop for Allocator<T, B> {
    fn drop(&mut self) {
        unsafe {
            let guard = crossbeam_epoch::pin();
            while let Some(b) = self.shared.all_buffers.pop_buffer(&guard) {
                let b = b.deref();
                while let Some(alloc_bufer) = b.buffer.pop_back_unsafe() {
                    libc::free(alloc_bufer as *mut libc::c_void);
                }
            }
        }
    }
}

impl<T, const B: usize> TLAlloc<T, B> {
    pub fn new(buffer: usize, limit: usize, shared: Arc<SharedAlloc<T, B>>) -> Self {
        Self {
            inner: UnsafeCell::new(TLAllocInner::new(buffer, limit, shared)),
        }
    }

    fn get(&self) -> *mut TLAllocInner<T, B> {
        self.inner.get()
    }
}
