#![feature(core_intrinsics)]
#![feature(allocator_api)]
#![feature(test)]
#![feature(const_option)]
#![feature(const_fn_trait_bound)]
#![feature(const_trait_impl)]
#![feature(const_impl_trait)]
#[macro_use]
extern crate log;
extern crate alloc;
extern crate test;

#[macro_use]
extern crate static_assertions;
// pub mod deque;
pub mod linked_map;
pub mod list;
pub mod lru_cache;
pub mod map;
pub mod ring_buffer;
pub mod spin;
pub mod ttl_cache;

pub mod rand;

#[macro_use]
mod par_list_test_macros;

pub const fn align_padding(len: usize, align: usize) -> usize {
    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}
