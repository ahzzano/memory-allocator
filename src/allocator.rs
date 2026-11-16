use std::{
    alloc::GlobalAlloc,
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Copy, Clone)]
pub enum BlockState {
    Free,
    Used,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Block {
    size: usize,
    state: BlockState,
    pub next: *mut Block,
}

impl Block {
    fn new(size: usize) -> Self {
        Self {
            size,
            state: BlockState::Free,
            next: ptr::null_mut(),
        }
    }

    unsafe fn contents(this: *mut Block) -> *mut u8 {
        unsafe { (this as *mut u8).add(size_of::<Block>()) }
    }
}

const HEAP_SIZE: usize = 1 << 20;
struct Heap(UnsafeCell<[u8; HEAP_SIZE]>);

pub struct GlobalArena {
    heap: Heap,
    arena: UnsafeCell<Option<Arena>>,
    lock: AtomicBool,
}

impl Default for GlobalArena {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalArena {
    pub const fn new() -> Self {
        let heap = Heap(UnsafeCell::new([0; HEAP_SIZE]));
        GlobalArena {
            heap,
            arena: UnsafeCell::new(None),
            lock: AtomicBool::new(false),
        }
    }

    pub unsafe fn with<R>(&self, f: impl FnOnce(&Arena) -> R) -> R {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        let slot = unsafe { &mut *self.arena.get() };
        let arena =
            slot.get_or_insert_with(|| unsafe { Arena::new(self.heap.0.get().cast(), HEAP_SIZE) });
        let r = f(arena);
        self.lock.store(false, Ordering::Release);
        r
    }
}

unsafe impl Sync for GlobalArena {}
unsafe impl GlobalAlloc for GlobalArena {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        unsafe { self.with(|a| a.alloc(layout)) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { self.with(|a| a.dealloc(ptr, layout)) }
    }
}

pub struct Arena {
    root: *mut Block,
}

impl Arena {
    pub unsafe fn new(base: *mut u8, size: usize) -> Self {
        assert!(size > size_of::<Block>());
        let root = base.cast::<Block>();
        unsafe { root.write(Block::new(size - size_of::<Block>())) };
        Arena { root }
    }

    unsafe fn coalesce(&self) {
        let mut current = self.root;
        unsafe {
            while !current.is_null() {
                let next = (*current).next;
                if !next.is_null()
                    && matches!((*current).state, BlockState::Free)
                    && matches!((*next).state, BlockState::Free)
                {
                    (*current).size += size_of::<Block>() + (*next).size;
                    (*current).next = (*next).next;
                } else {
                    current = next;
                }
            }
        }
    }
}

unsafe impl Sync for Arena {}
unsafe impl GlobalAlloc for Arena {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.align() > align_of::<Block>() {
            return ptr::null_mut();
        }
        unsafe {
            let data_size = layout.size().next_multiple_of(align_of::<Block>());
            let mut current = self.root;

            while !current.is_null() {
                let block_size = (*current).size;
                let block = *current;

                if matches!(block.state, BlockState::Free) && data_size < block_size {
                    let new_block = Block::contents(current).add(data_size) as *mut Block;
                    new_block.write(Block {
                        size: block_size - data_size - size_of::<Block>(),
                        state: BlockState::Free,
                        next: (*current).next,
                    });

                    (*current).size = data_size;
                    (*current).next = new_block;
                    (*current).state = BlockState::Used;

                    return Block::contents(current);
                }

                current = (*current).next;
            }
            ptr::null_mut()
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: std::alloc::Layout) {
        unsafe {
            let block = ptr.sub(size_of::<Block>()) as *mut Block;
            (*block).state = BlockState::Free;
            self.coalesce();
        }
    }
}

#[cfg(test)]
mod tests {
    #[repr(align(16))]
    struct Buf([u8; 1024]);

    use super::*;
    #[test]
    fn alloc_returns_aligned_bounds_in_ptr() {
        let mut buf = Buf([0; 1024]);
        let arena = unsafe { Arena::new(buf.0.as_mut_ptr(), 1024) };

        let layout = std::alloc::Layout::from_size_align(64, 8).unwrap();
        let p = unsafe { arena.alloc(layout) };

        assert!(!p.is_null());
        assert_eq!(p as usize % 8, 0);
        assert!(p as usize >= buf.0.as_ptr() as usize);
        assert!((p as usize) + 64 <= buf.0.as_ptr() as usize + 1024);
    }

    #[test]
    fn dealloc_makes_block_reusable() {
        let mut buf = Buf([0; 1024]);
        let arena = unsafe { Arena::new(buf.0.as_mut_ptr(), 1024) };
        let layout = std::alloc::Layout::from_size_align(64, 8).unwrap();

        let a = unsafe { arena.alloc(layout) };
        assert!(!a.is_null());
        unsafe { arena.dealloc(a, layout) };

        let b = unsafe { arena.alloc(layout) };
        assert_eq!(a, b);
    }

    #[test]
    fn dealloc_coalesces_neighbours() {
        let mut buf = Buf([0; 1024]);
        let arena = unsafe { Arena::new(buf.0.as_mut_ptr(), 1024) };
        let layout = std::alloc::Layout::from_size_align(64, 8).unwrap();

        let a = unsafe { arena.alloc(layout) };
        let b = unsafe { arena.alloc(layout) };
        unsafe { arena.dealloc(a, layout) };
        unsafe { arena.dealloc(b, layout) };

        // only fits if A, B and the trailing free block merged back together
        let big = std::alloc::Layout::from_size_align(900, 8).unwrap();
        assert!(!unsafe { arena.alloc(big) }.is_null());
    }
}
