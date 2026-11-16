pub mod allocator;

#[global_allocator]
static ALLOC: allocator::GlobalArena = allocator::GlobalArena::new();

fn main() {
    println!("Hello, world!");

    let mut vector = vec![1, 2, 3, 4, 5];
    vector.push(67);
}
