//! No-op replacement for errno crate in WASM environment
//! This provides minimal stubs to allow compilation without actual errno support

#![cfg_attr(not(feature = "std"), no_std)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Errno(pub i32);

impl Errno {
    pub fn as_i32(self) -> i32 {
        self.0
    }
    
    pub fn from_i32(errno: i32) -> Self {
        Errno(errno)
    }
}

pub fn get_errno() -> Errno {
    Errno(0)
}

/// Stub for rustix 1.1 / libc_errno: return current errno (no-op, always 0)
pub fn errno() -> Errno {
    Errno(0)
}

pub fn set_errno(_errno: Errno) {
    // No-op in WASM
}

// Re-export common errno values
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
