pub mod allocator;

#[global_allocator]
static alloc: allocator::GlobalArena = allocator::GlobalArena::new();
