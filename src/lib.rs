pub mod allocator;

#[global_allocator]
static ALLOC: allocator::GlobalArena = allocator::GlobalArena::new();
