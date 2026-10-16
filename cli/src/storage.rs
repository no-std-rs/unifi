//! Process-wide, bounded working memory. Keep large buffers out of the Linux stack.
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct Storage {
    pub response: [u8; 1048576],
    pub combined: [u8; 1048576],
    pub filtered: [u8; 1048576],
    pub sanitized: [u8; 1048576],
}
struct Shared(UnsafeCell<Storage>);
// SAFETY: take() grants access once, guarded by an atomic operation. No other access exists.
unsafe impl Sync for Shared {}
static TAKEN: AtomicBool = AtomicBool::new(false);
static STORAGE: Shared = Shared(UnsafeCell::new(Storage {
    response: [0; 1048576],
    combined: [0; 1048576],
    filtered: [0; 1048576],
    sanitized: [0; 1048576],
}));

pub fn take() -> Option<&'static mut Storage> {
    if TAKEN.swap(true, Ordering::Relaxed) {
        return None;
    }
    // SAFETY: the atomic guard permits exactly one mutable reference for this process.
    Some(unsafe { &mut *STORAGE.0.get() })
}
