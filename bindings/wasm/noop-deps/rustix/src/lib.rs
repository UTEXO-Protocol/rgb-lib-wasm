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
