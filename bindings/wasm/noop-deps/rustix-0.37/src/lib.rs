//! No-op replacement for rustix crate in WASM environment
//! This provides minimal stubs to allow compilation without actual system calls

#![cfg_attr(not(feature = "std"), no_std)]

// Minimal re-exports to satisfy dependencies
pub mod fs {
    pub mod cwd {
        pub fn getcwd(_buffer: &mut [u8]) -> Result<usize, ()> {
            Ok(0)
        }
    }

    /// Stub for async-io: file status flags
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct OFlags(pub u32);
    impl OFlags {
        pub const NONBLOCK: OFlags = OFlags(0x800);
    }
    impl core::ops::BitOr for OFlags {
        type Output = OFlags;
        fn bitor(self, other: OFlags) -> OFlags {
            OFlags(self.0 | other.0)
        }
    }
    impl core::ops::BitOrAssign for OFlags {
        fn bitor_assign(&mut self, other: OFlags) {
            self.0 |= other.0;
        }
    }

    /// Stub for async-io: get file status flags (no-op). Takes BorrowedFd by value to match callers.
    #[cfg(feature = "std")]
    pub fn fcntl_getfl(_fd: crate::fd::BorrowedFd<'_>) -> Result<OFlags, std::io::Error> {
        Ok(OFlags::default())
    }
    #[cfg(not(feature = "std"))]
    pub fn fcntl_getfl(_fd: crate::fd::BorrowedFd<'_>) -> Result<OFlags, ()> {
        Ok(OFlags::default())
    }
    /// Stub for async-io: set file status flags (no-op). Takes BorrowedFd by value to match callers.
    #[cfg(feature = "std")]
    pub fn fcntl_setfl(_fd: crate::fd::BorrowedFd<'_>, _flags: OFlags) -> Result<(), std::io::Error> {
        Ok(())
    }
    #[cfg(not(feature = "std"))]
    pub fn fcntl_setfl(_fd: crate::fd::BorrowedFd<'_>, _flags: OFlags) -> Result<(), ()> {
        Ok(())
    }
}

/// Stub for async-io: borrowed file descriptor. Copy so fcntl_getfl(fd) doesn't move fd.
pub mod fd {
    use super::RawFd;
    #[derive(Debug, Clone, Copy)]
    pub struct BorrowedFd<'fd> {
        raw: RawFd,
        _phantom: core::marker::PhantomData<&'fd ()>,
    }
    impl BorrowedFd<'_> {
        pub unsafe fn borrow_raw(raw: RawFd) -> Self {
            Self {
                raw,
                _phantom: core::marker::PhantomData,
            }
        }
    }
}

/// Stub for async-io: errno
pub mod io {
    #[derive(Clone, Copy, Debug)]
    pub struct Errno(pub i32);
    impl Errno {
        pub const INPROGRESS: Errno = Errno(115);
        /// Returns the raw errno value (async-io compares with Some(this) vs err.raw_os_error()).
        pub fn raw_os_error(self) -> i32 {
            self.0
        }
    }
}

pub mod time {
    pub mod types {
        pub struct ClockId;
        pub struct Timespec {
            pub tv_sec: i64,
            pub tv_nsec: i64,
        }
    }
    
    pub fn clock_gettime(_clock: types::ClockId) -> Result<types::Timespec, ()> {
        Ok(types::Timespec { tv_sec: 0, tv_nsec: 0 })
    }
}

// Minimal types that might be needed
pub type RawFd = i32;
pub type Pid = i32;
pub type Uid = u32;
pub type Gid = u32;

// Re-export common constants that might be needed
pub mod c {
    pub type c_int = i32;
    pub type c_uint = u32;
    pub type c_long = i64;
    pub type c_ulong = u64;
    
    // Clock constants
    pub const CLOCK_REALTIME: i32 = 0;
    pub const CLOCK_MONOTONIC: i32 = 1;
    pub const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
    pub const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    
    // Signal constants
    pub const CLD_STOPPED: i32 = 1;
    pub const CLD_TRAPPED: i32 = 2;
    pub const CLD_EXITED: i32 = 3;
    pub const CLD_KILLED: i32 = 4;
    pub const CLD_DUMPED: i32 = 5;
    pub const CLD_CONTINUED: i32 = 6;
    
    // Utime constants
    pub const UTIME_NOW: i64 = -1;
    pub const UTIME_OMIT: i64 = -2;
    
    // Minimal types
    pub struct siginfo_t {
        _private: (),
    }
}

pub mod backend {
    pub mod c {
        pub use super::super::c::*;
    }
}
