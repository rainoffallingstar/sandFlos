use std::sync::atomic::{AtomicBool, Ordering};
pub static FLAG: AtomicBool = AtomicBool::new(false);
pub fn requested() -> bool {
    FLAG.load(Ordering::Relaxed)
}
pub fn install() {
    #[cfg(unix)]
    {
        extern "C" fn handle(_: i32) {
            FLAG.store(true, Ordering::Relaxed);
        }
        extern "C" {
            fn signal(number: i32, handler: extern "C" fn(i32)) -> usize;
        }
        // The handler only writes a lock-free atomic and remains valid forever.
        unsafe {
            signal(2, handle);
        }
    }
    #[cfg(windows)]
    {
        extern "system" fn handle(event: u32) -> i32 {
            if event <= 1 {
                FLAG.store(true, Ordering::Relaxed);
                1
            } else {
                0
            }
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleCtrlHandler(handler: extern "system" fn(u32) -> i32, add: i32) -> i32;
        }
        unsafe {
            SetConsoleCtrlHandler(handle, 1);
        }
    }
}
