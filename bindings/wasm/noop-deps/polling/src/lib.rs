//! No-op replacement for polling crate in WASM environment
//! This provides minimal stubs to allow compilation without actual polling support

#![cfg_attr(not(feature = "std"), no_std)]

use core::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub key: usize,
    pub readable: bool,
    pub writable: bool,
}

impl Event {
    /// Create an empty event for the given key (stub for async-io).
    pub fn none(key: usize) -> Self {
        Event {
            key,
            readable: false,
            writable: false,
        }
    }
}

pub struct Poller {
    // Empty struct for WASM
}

impl Poller {
    pub fn new() -> Result<Self, crate::Error> {
        Ok(Poller {})
    }
    
    pub fn add(&self, _source: impl Source, _interest: Event) -> Result<(), crate::Error> {
        Ok(())
    }
    
    pub fn modify(&self, _source: impl Source, _interest: Event) -> Result<(), crate::Error> {
        Ok(())
    }
    
    pub fn delete(&self, _source: impl Source) -> Result<(), crate::Error> {
        Ok(())
    }
    
    pub fn wait(&self, _events: &mut [Event], _timeout: Option<Duration>) -> Result<usize, crate::Error> {
        Ok(0)
    }

    /// Wakes up the current or next wait() (stub for async-io).
    pub fn notify(&self) -> Result<(), crate::Error> {
        Ok(())
    }
}

pub trait Source {
    fn raw_fd(&self) -> i32;
}

/// Allow i32 (raw fd) to be used as Source for Poller::add/delete (async-io).
impl Source for i32 {
    fn raw_fd(&self) -> i32 {
        *self
    }
}

#[cfg(feature = "std")]
pub type Error = std::io::Error;

#[cfg(not(feature = "std"))]
pub type Error = core::fmt::Error;
