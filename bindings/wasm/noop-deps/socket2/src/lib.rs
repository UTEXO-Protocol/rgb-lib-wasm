//! WASM-compatible no-op replacement for socket2 crate
//! Provides minimal stubs for socket operations in WASM environment

#[cfg(not(feature = "std"))]
use core::fmt;

/// Socket wrapper (no-op for WASM)
#[derive(Debug)]
pub struct Socket;

impl Socket {
    /// When std: SocketError = std::io::Error so async-io's return Err(err) type-checks.
    #[cfg(feature = "std")]
    pub fn new(
        _domain: Domain,
        _ty: Type,
        _protocol: Option<Protocol>,
    ) -> Result<Self, SocketError> {
        Err(unsupported_socket_error())
    }
    #[cfg(not(feature = "std"))]
    pub fn new(_domain: Domain, _ty: Type, _protocol: Protocol) -> Result<Self, SocketError> {
        Err(SocketError::unsupported())
    }

    #[cfg(feature = "std")]
    pub fn bind(&self, _addr: &SockAddr) -> Result<(), SocketError> {
        Err(unsupported_socket_error())
    }
    #[cfg(not(feature = "std"))]
    pub fn bind(&self, _addr: &SockAddr) -> Result<(), SocketError> {
        Err(SocketError::unsupported())
    }

    #[cfg(feature = "std")]
    pub fn listen(&self, _backlog: i32) -> Result<(), SocketError> {
        Err(unsupported_socket_error())
    }
    #[cfg(not(feature = "std"))]
    pub fn listen(&self, _backlog: i32) -> Result<(), SocketError> {
        Err(SocketError::unsupported())
    }

    #[cfg(feature = "std")]
    pub fn connect(&self, _addr: &SockAddr) -> Result<(), SocketError> {
        Err(unsupported_socket_error())
    }
    #[cfg(not(feature = "std"))]
    pub fn connect(&self, _addr: &SockAddr) -> Result<(), SocketError> {
        Err(SocketError::unsupported())
    }

    #[cfg(feature = "std")]
    pub fn set_nonblocking(&self, _nonblocking: bool) -> Result<(), SocketError> {
        Err(unsupported_socket_error())
    }
    #[cfg(not(feature = "std"))]
    pub fn set_nonblocking(&self, _nonblocking: bool) -> Result<(), SocketError> {
        Err(SocketError::unsupported())
    }
}

/// Socket domain
#[derive(Debug, Clone, Copy)]
pub enum Domain {
    IPv4,
    IPv6,
    Unix,
}

impl Domain {
    /// Unix domain constant (async-io uses Domain::UNIX).
    pub const UNIX: Domain = Domain::Unix;
}

#[cfg(feature = "std")]
impl Domain {
    /// Stub: derive domain from address (async-io).
    pub fn for_address(_addr: std::net::SocketAddr) -> Self {
        Domain::IPv4
    }
}

/// Socket type
#[derive(Debug, Clone, Copy)]
pub enum Type {
    Stream,
    Datagram,
    SeqPacket,
    Raw,
}

impl Type {
    /// Stream type constant (async-io uses Type::STREAM).
    pub const STREAM: Type = Type::Stream;
    /// Stub: return self for nonblocking (async-io).
    pub const fn nonblocking(self) -> Self {
        self
    }
}

/// Protocol
#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    TCP,
    UDP,
    ICMP,
}

/// Socket address (minimal stub)
#[derive(Debug, Clone, Copy)]
pub struct SockAddr;

impl SockAddr {
    pub fn as_socket(&self) -> Option<SocketAddr> {
        None
    }
}

#[cfg(feature = "std")]
impl SockAddr {
    /// Stub: create unix sockaddr (async-io).
    pub fn unix<P: AsRef<std::path::Path>>(_path: P) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unix sockets not supported",
        ))
    }
}

#[cfg(feature = "std")]
impl From<std::net::SocketAddr> for SockAddr {
    fn from(_: std::net::SocketAddr) -> Self {
        SockAddr
    }
}

/// Socket address (minimal stub)
#[derive(Debug, Clone, Copy)]
pub struct SocketAddr;

// When std: use io types so async-io's "return Err(err)" gets std::io::Error
#[cfg(feature = "std")]
pub type SocketError = std::io::Error;
#[cfg(feature = "std")]
pub type ErrorKind = std::io::ErrorKind;
#[cfg(feature = "std")]
pub fn unsupported_socket_error() -> SocketError {
    std::io::Error::new(std::io::ErrorKind::Unsupported, "sockets not supported")
}

/// Socket error (no_std)
#[cfg(not(feature = "std"))]
#[derive(Debug)]
pub struct SocketError {
    kind: ErrorKind,
}

#[cfg(not(feature = "std"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Unsupported,
    WouldBlock,
    Interrupted,
    Other,
}

#[cfg(not(feature = "std"))]
impl SocketError {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        match self.kind {
            ErrorKind::WouldBlock => Some(115),
            ErrorKind::Interrupted => Some(4),
            _ => None,
        }
    }

    pub const fn new(kind: ErrorKind) -> Self {
        SocketError { kind }
    }

    pub const fn unsupported() -> Self {
        SocketError {
            kind: ErrorKind::Unsupported,
        }
    }
}

#[cfg(not(feature = "std"))]
impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Unsupported => write!(f, "Sockets not supported in WASM"),
            ErrorKind::WouldBlock => write!(f, "Operation would block"),
            ErrorKind::Interrupted => write!(f, "Operation interrupted"),
            ErrorKind::Other => write!(f, "Other socket error"),
        }
    }
}

#[cfg(not(feature = "std"))]
impl core::error::Error for SocketError {}

// Separate types to avoid conflicts with blanket impl From<T> for T
#[derive(Debug)]
pub struct TcpStream(Socket);

#[derive(Debug)]
pub struct TcpListener(Socket);

#[derive(Debug)]
pub struct UdpSocket(Socket);

// Implement From traits for compatibility
impl From<TcpStream> for Socket {
    fn from(t: TcpStream) -> Self {
        t.0
    }
}

impl From<TcpListener> for Socket {
    fn from(t: TcpListener) -> Self {
        t.0
    }
}

impl From<UdpSocket> for Socket {
    fn from(u: UdpSocket) -> Self {
        u.0
    }
}

impl From<Socket> for TcpStream {
    fn from(s: Socket) -> Self {
        TcpStream(s)
    }
}

impl From<Socket> for TcpListener {
    fn from(s: Socket) -> Self {
        TcpListener(s)
    }
}

impl From<Socket> for UdpSocket {
    fn from(s: Socket) -> Self {
        UdpSocket(s)
    }
}

// Stub impls so async-io type-checks when it does TcpStream::from(socket) / UnixStream::from(socket).
// In our noop, Socket::new always returns Err so this path is never taken at runtime.
#[cfg(all(unix, feature = "std"))]
impl From<Socket> for std::net::TcpStream {
    fn from(_: Socket) -> Self {
        use std::os::unix::io::{FromRawFd, OwnedFd};
        unsafe { std::net::TcpStream::from(OwnedFd::from_raw_fd(-1)) }
    }
}

#[cfg(all(unix, feature = "std"))]
impl From<Socket> for std::os::unix::net::UnixStream {
    fn from(_: Socket) -> Self {
        use std::os::unix::io::{FromRawFd, OwnedFd};
        unsafe { std::os::unix::net::UnixStream::from(OwnedFd::from_raw_fd(-1)) }
    }
}
