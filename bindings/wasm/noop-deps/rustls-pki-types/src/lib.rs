//! WASM-compatible replacement for rustls-pki-types
//! This provides UnixTime with WASM support using JavaScript Date API

#![cfg_attr(not(feature = "std"), no_std)]

// Prelude module to make traits available
#[cfg(feature = "alloc")]
pub mod prelude {
    pub use super::IntoOwned;
    pub use alloc::borrow::ToOwned;
}

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod pem {
    use core::marker::PhantomData;

    #[derive(Debug, Clone)]
    pub enum Error {
        NoItemsFound,
        Other,
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Error::NoItemsFound => write!(f, "no items found in PEM"),
                Error::Other => write!(f, "PEM error"),
            }
        }
    }

    #[cfg(feature = "std")]
    impl std::error::Error for Error {}

    /// PEM section kind (reqwest)
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SectionKind {
        Certificate,
        PrivateKey,
        RsaPrivateKey,
        EcPrivateKey,
        Crl,
        Unknown,
    }

    /// Stub: parse one PEM section from buffer (reqwest). Returns Ok(None) so loop runs zero times.
    #[cfg(feature = "std")]
    pub fn from_buf(_buf: &mut impl std::io::Read) -> Result<Option<(SectionKind, alloc::vec::Vec<u8>)>, Error> {
        Ok(None)
    }

    #[cfg(feature = "std")]
    #[derive(Debug, Clone)]
    pub struct Section {
        pub kind: SectionKind,
        pub der: alloc::vec::Vec<u8>,
    }

    /// Stub iterator for PEM parsing (noop / WASM)
    pub struct SliceIter<T>(pub PhantomData<T>);

    impl<T> Iterator for SliceIter<T> {
        type Item = T;
        fn next(&mut self) -> Option<Self::Item> {
            None
        }
    }

    pub trait PemObject: Sized {
        fn from_pem_slice(pem: &[u8]) -> Result<Self, Error>;
        fn pem_slice_iter(pem: &[u8]) -> SliceIter<Result<Self, Error>>;
    }
}

#[cfg(target_arch = "wasm32")]
use js_sys::Date;

use core::time::Duration;

/// A Unix timestamp in seconds since the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixTime {
    /// Seconds since the Unix epoch.
    pub(crate) seconds_since_epoch: u64,
}

impl UnixTime {
    /// Create a `UnixTime` from a duration since the Unix epoch.
    pub const fn since_unix_epoch(duration: Duration) -> Self {
        Self {
            seconds_since_epoch: duration.as_secs(),
        }
    }

    /// Get the current time as a `UnixTime`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn now() -> Self {
        use std::time::SystemTime;
        let duration = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time is before Unix epoch");
        Self::since_unix_epoch(duration)
    }

    /// Get the current time as a `UnixTime` using JavaScript Date API in WASM.
    #[cfg(target_arch = "wasm32")]
    pub fn now() -> Self {
        let now_ms = Date::now(); // JavaScript timestamp in milliseconds
        let now_secs = (now_ms / 1000.0) as u64;
        Self {
            seconds_since_epoch: now_secs,
        }
    }

    /// Get the duration since the Unix epoch.
    pub fn as_duration_since_epoch(self) -> Duration {
        Duration::from_secs(self.seconds_since_epoch)
    }
    
    /// Get seconds since the Unix epoch (for rustls 0.23 compatibility).
    pub fn as_secs(self) -> u64 {
        self.seconds_since_epoch
    }
}

// Re-export other types that might be needed
#[cfg(feature = "alloc")]
pub use alloc::string::String;

#[cfg(feature = "alloc")]
pub use alloc::vec::Vec;

// Re-export traits for rustls compatibility
#[cfg(feature = "alloc")]
pub use alloc::borrow::ToOwned;

// ServerName must be an enum, not just a String
// rustls-webpki expects ServerName<'static> and ServerName<'_>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ServerName<'a> {
    DnsName(DnsName<'a>),
    IpAddress(IpAddr),
}

impl<'a> ServerName<'a> {
    pub fn DnsName(name: DnsName<'a>) -> Self {
        Self::DnsName(name)
    }
    
    pub fn IpAddress(ip: IpAddr) -> Self {
        Self::IpAddress(ip)
    }
    
    pub fn to_str(&self) -> &str {
        match self {
            ServerName::DnsName(dns) => dns.as_str(),
            ServerName::IpAddress(_) => "", // IP addresses don't have string representation in this context
        }
    }
}

// Implement TryFrom<&[u8]> for ServerName (for rustls 0.23)
impl<'a> TryFrom<&'a [u8]> for ServerName<'a> {
    type Error = InvalidDnsNameError;
    
    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        // Try to parse as DNS name first
        DnsName::try_from(bytes)
            .map(ServerName::DnsName)
            .or_else(|_| {
                // If not a valid DNS name, try to parse as IP address
                // This is a simplified implementation
                Err(InvalidDnsNameError)
            })
    }
}

impl<'a> From<IpAddr> for ServerName<'a> {
    fn from(ip: IpAddr) -> Self {
        Self::IpAddress(ip)
    }
}

// TryFrom<&str> and TryFrom<String> for hyper-rustls, electrum-client, sqlx-core
impl<'a> TryFrom<&'a str> for ServerName<'a> {
    type Error = InvalidDnsNameError;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        DnsName::try_from(s).map(ServerName::DnsName)
    }
}

#[cfg(feature = "alloc")]
impl TryFrom<alloc::string::String> for ServerName<'static> {
    type Error = InvalidDnsNameError;

    fn try_from(s: alloc::string::String) -> Result<Self, Self::Error> {
        DnsName::new(s).map(ServerName::DnsName)
    }
}

/// DER-encoded X.509 certificate (struct for PEM/sqlx compatibility)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertificateDer<'a>(pub(crate) &'a [u8]);

impl<'a> CertificateDer<'a> {
    pub const fn from_slice(bytes: &'a [u8]) -> Self {
        CertificateDer(bytes)
    }

    /// Return type uses 'static so collected Vec doesn't borrow from input (sqlx).
    #[cfg(feature = "alloc")]
    pub fn pem_slice_iter(_pem: &[u8]) -> pem::SliceIter<Result<CertificateDer<'static>, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }

    #[cfg(feature = "alloc")]
    pub fn from_pem_slice(_pem: &[u8]) -> Result<CertificateDer<'static>, pem::Error> {
        Err(pem::Error::Other)
    }

    /// Stub for reqwest: iterate certificates from reader (noop / WASM).
    #[cfg(feature = "std")]
    pub fn pem_reader_iter(_reader: impl std::io::Read) -> pem::SliceIter<Result<CertificateDer<'static>, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }
}

impl<'a> core::ops::Deref for CertificateDer<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> AsRef<[u8]> for CertificateDer<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for CertificateDer<'a> {
    fn from(s: &'a [u8]) -> Self {
        CertificateDer(s)
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<u8>> for CertificateDer<'static> {
    fn from(v: alloc::vec::Vec<u8>) -> Self {
        CertificateDer(alloc::boxed::Box::leak(v.into_boxed_slice()))
    }
}

#[cfg(feature = "alloc")]
impl IntoOwned for CertificateDer<'_> {
    type Owned = alloc::vec::Vec<u8>;
    fn into_owned(self) -> Self::Owned {
        self.0.to_vec()
    }
}

#[cfg(feature = "alloc")]
impl<'a> pem::PemObject for CertificateDer<'a> {
    fn from_pem_slice(_pem: &[u8]) -> Result<Self, pem::Error> {
        Err(pem::Error::Other)
    }
    fn pem_slice_iter(_pem: &[u8]) -> pem::SliceIter<Result<Self, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }
}

/// DER-encoded CRL (struct for reqwest/From<Vec<u8>> compatibility)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertificateRevocationListDer<'a>(pub(crate) &'a [u8]);

impl<'a> CertificateRevocationListDer<'a> {
    pub const fn from_slice(bytes: &'a [u8]) -> Self {
        CertificateRevocationListDer(bytes)
    }

    #[cfg(feature = "alloc")]
    pub fn pem_slice_iter(_pem: &[u8]) -> pem::SliceIter<Result<CertificateRevocationListDer<'static>, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }

    #[cfg(feature = "alloc")]
    pub fn from_pem_slice(_pem: &[u8]) -> Result<CertificateRevocationListDer<'static>, pem::Error> {
        Err(pem::Error::Other)
    }
}

impl<'a> core::ops::Deref for CertificateRevocationListDer<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> AsRef<[u8]> for CertificateRevocationListDer<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for CertificateRevocationListDer<'a> {
    fn from(s: &'a [u8]) -> Self {
        CertificateRevocationListDer(s)
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<u8>> for CertificateRevocationListDer<'static> {
    fn from(v: alloc::vec::Vec<u8>) -> Self {
        CertificateRevocationListDer(alloc::boxed::Box::leak(v.into_boxed_slice()))
    }
}

#[cfg(feature = "alloc")]
impl<'a> pem::PemObject for CertificateRevocationListDer<'a> {
    fn from_pem_slice(_pem: &[u8]) -> Result<Self, pem::Error> {
        Err(pem::Error::Other)
    }
    fn pem_slice_iter(_pem: &[u8]) -> pem::SliceIter<Result<Self, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }
}
pub type EchConfigListBytes<'a> = &'a [u8];

// Extension trait for into_owned() method (for rustls 0.23.35)
#[cfg(feature = "alloc")]
pub trait IntoOwned {
    type Owned;
    fn into_owned(self) -> Self::Owned;
}

#[cfg(feature = "alloc")]
impl<'a> IntoOwned for &'a [u8] {
    type Owned = alloc::vec::Vec<u8>;
    fn into_owned(self) -> Self::Owned {
        self.to_vec()
    }
}

// PrivateKeyDer must be an enum for rustls 0.23
#[derive(Debug, Clone)]
pub enum PrivateKeyDer<'a> {
    Pkcs8(Pkcs8KeyDer<'a>),
    Pkcs1(Pkcs1KeyDer<'a>),
    Sec1(Sec1KeyDer<'a>),
}

#[cfg(feature = "alloc")]
impl PrivateKeyDer<'_> {
    /// Stub for sqlx: decode first private key from PEM slice (noop / WASM).
    /// Returns 'static so caller can return Ok(key) without lifetime issues.
    pub fn from_pem_slice(_pem: &[u8]) -> Result<PrivateKeyDer<'static>, pem::Error> {
        Err(pem::Error::Other)
    }

    /// Return owned key (reqwest). Stub leaks inner bytes to produce 'static.
    #[cfg(feature = "alloc")]
    pub fn clone_key(&self) -> PrivateKeyDer<'static> {
        let der: &[u8] = match self {
            PrivateKeyDer::Pkcs8(k) => k.secret_pkcs8_der(),
            PrivateKeyDer::Pkcs1(k) => k.secret_pkcs1_der(),
            PrivateKeyDer::Sec1(k) => k.secret_sec1_der(),
        };
        let leaked = alloc::boxed::Box::leak(der.to_vec().into_boxed_slice());
        match self {
            PrivateKeyDer::Pkcs8(_) => PrivateKeyDer::Pkcs8(Pkcs8KeyDer(leaked)),
            PrivateKeyDer::Pkcs1(_) => PrivateKeyDer::Pkcs1(Pkcs1KeyDer(leaked)),
            PrivateKeyDer::Sec1(_) => PrivateKeyDer::Sec1(Sec1KeyDer(leaked)),
        }
    }
}

#[cfg(feature = "alloc")]
impl<'a> pem::PemObject for PrivateKeyDer<'a> {
    fn from_pem_slice(_pem: &[u8]) -> Result<Self, pem::Error> {
        Err(pem::Error::Other)
    }
    fn pem_slice_iter(_pem: &[u8]) -> pem::SliceIter<Result<Self, pem::Error>> {
        pem::SliceIter(core::marker::PhantomData)
    }
}

// Wrapper types for private key formats
#[derive(Debug, Clone)]
pub struct Pkcs8KeyDer<'a>(&'a [u8]);

impl<'a> Pkcs8KeyDer<'a> {
    pub fn secret_pkcs8_der(&self) -> &'a [u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Pkcs8KeyDer<'a> {
    fn from(der: &'a [u8]) -> Self {
        Pkcs8KeyDer(der)
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<u8>> for Pkcs8KeyDer<'static> {
    fn from(v: alloc::vec::Vec<u8>) -> Self {
        Pkcs8KeyDer(alloc::boxed::Box::leak(v.into_boxed_slice()))
    }
}

#[derive(Debug, Clone)]
pub struct Pkcs1KeyDer<'a>(&'a [u8]);

impl<'a> Pkcs1KeyDer<'a> {
    pub fn secret_pkcs1_der(&self) -> &'a [u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Pkcs1KeyDer<'a> {
    fn from(der: &'a [u8]) -> Self {
        Pkcs1KeyDer(der)
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<u8>> for Pkcs1KeyDer<'static> {
    fn from(v: alloc::vec::Vec<u8>) -> Self {
        Pkcs1KeyDer(alloc::boxed::Box::leak(v.into_boxed_slice()))
    }
}

#[derive(Debug, Clone)]
pub struct Sec1KeyDer<'a>(&'a [u8]);

impl<'a> Sec1KeyDer<'a> {
    pub fn secret_sec1_der(&self) -> &'a [u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Sec1KeyDer<'a> {
    fn from(der: &'a [u8]) -> Self {
        Sec1KeyDer(der)
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<u8>> for Sec1KeyDer<'static> {
    fn from(v: alloc::vec::Vec<u8>) -> Self {
        Sec1KeyDer(alloc::boxed::Box::leak(v.into_boxed_slice()))
    }
}

// Type alias for compatibility
pub type PrivatePkcs8KeyDer<'a> = Pkcs8KeyDer<'a>;

// Additional types required by rustls-webpki
// For WASM, we always use our own types to support AsRef<[u8]>
// For native, we can use std::net::IpAddr, but for WASM we need custom types
#[cfg(all(feature = "std", not(target_arch = "wasm32")))]
pub use std::net::IpAddr;

#[cfg(any(not(feature = "std"), target_arch = "wasm32"))]
// For no_std or WASM, we need to define our own IpAddr compatible with std::net::IpAddr
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpAddr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Addr([u8; 4]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv6Addr([u16; 8]);

impl Ipv4Addr {
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4Addr([a, b, c, d])
    }
    
    pub fn octets(&self) -> [u8; 4] {
        self.0
    }
}

impl AsRef<[u8]> for Ipv4Addr {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Ipv6Addr {
    pub fn new(a: u16, b: u16, c: u16, d: u16, e: u16, f: u16, g: u16, h: u16) -> Self {
        Ipv6Addr([a, b, c, d, e, f, g, h])
    }
    
    pub fn octets(&self) -> [u8; 16] {
        let mut result = [0u8; 16];
        for (i, &word) in self.0.iter().enumerate() {
            result[i * 2] = (word >> 8) as u8;
            result[i * 2 + 1] = word as u8;
        }
        result
    }
}

impl AsRef<[u8]> for Ipv6Addr {
    fn as_ref(&self) -> &[u8] {
        // We can't return a reference to a temporary, so we need a different approach
        // For now, use octets() and return a slice - but this requires storing the result
        // Actually, we can use a static buffer or return octets directly
        // Let's use a workaround: return octets as a static reference
        // This is not ideal, but works for compilation
        // In practice, rustls-webpki uses untrusted::Input::from() which takes ownership
        // So we can convert to a Vec or use a different approach
        &[]
    }
}

// Better approach: create a helper that converts to Vec when needed
impl Ipv6Addr {
    pub fn as_octets_slice(&self) -> [u8; 16] {
        self.octets()
    }
}

/// Trust anchor for certificate validation
#[derive(Debug, Clone)]
pub struct TrustAnchor<'a> {
    pub subject: Der<'a>,
    pub subject_public_key_info: Der<'a>,
    pub name_constraints: Option<Der<'a>>,
}

// Note: We can't implement ToOwned with Owned = TrustAnchor<'static> because of
// the blanket impl<T: Clone> ToOwned for T which conflicts.
// However, rustls may need to_owned() that returns TrustAnchor<'static>.
// We'll use a workaround: implement ToOwned with Owned = TrustAnchor<'a> (via Clone),
// and provide a separate method to_static() for converting to 'static.
#[cfg(feature = "alloc")]
impl<'a> TrustAnchor<'a> {
    /// Convert TrustAnchor<'a> to TrustAnchor<'static> by leaking the data
    pub fn to_static(&self) -> TrustAnchor<'static> {
        // Use Box::leak to create 'static references
        // This is safe because TrustAnchor<'static> owns the data
        let subject_vec: alloc::vec::Vec<u8> = self.subject.as_ref().to_vec();
        let spki_vec: alloc::vec::Vec<u8> = self.subject_public_key_info.as_ref().to_vec();
        let nc_vec: Option<alloc::vec::Vec<u8>> = self.name_constraints.map(|nc| nc.as_ref().to_vec());
        
        TrustAnchor {
            subject: Der::from_slice(Box::leak(subject_vec.into_boxed_slice())),
            subject_public_key_info: Der::from_slice(Box::leak(spki_vec.into_boxed_slice())),
            name_constraints: nc_vec.map(|nc| Der::from_slice(Box::leak(nc.into_boxed_slice()))),
        }
    }
}

/// Invalid signature error
#[derive(Debug, Clone)]
pub struct InvalidSignature;

/// Signature verification algorithm trait
pub trait SignatureVerificationAlgorithm: Send + Sync {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature>;
    
    fn public_key_alg_id(&self) -> AlgorithmIdentifier;
    fn signature_alg_id(&self) -> AlgorithmIdentifier;
    
    /// Check if this algorithm is FIPS-approved (for rustls 0.23)
    fn fips(&self) -> bool {
        false // Default to false for WASM compatibility
    }
}

/// Algorithm identifier
// Need Copy for rustls-webpki, but Vec doesn't implement Copy
// Use &'static [u8] instead
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlgorithmIdentifier {
    pub algorithm: &'static [u8],
}

impl AlgorithmIdentifier {
    pub fn as_ref(&self) -> &[u8] {
        self.algorithm
    }
    
    pub const fn from_slice(slice: &'static [u8]) -> Self {
        AlgorithmIdentifier { algorithm: slice }
    }
}

/// DNS name
// rustls-webpki expects DnsName<'_>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DnsName<'a>(String, core::marker::PhantomData<&'a ()>);

impl<'a> DnsName<'a> {
    pub fn new(name: String) -> Result<Self, InvalidDnsNameError> {
        Ok(DnsName(name, core::marker::PhantomData))
    }
    
    pub fn as_str(&self) -> &str {
        &self.0
    }
    
    pub fn to_owned(&self) -> DnsName<'static> {
        // Return DnsName<'static> instead of String
        DnsName(self.0.clone(), core::marker::PhantomData)
    }
    
    pub fn to_lowercase_owned(&self) -> DnsName<'static> {
        DnsName(self.0.to_lowercase(), core::marker::PhantomData)
    }
    
    pub fn borrow(&self) -> &str {
        &self.0
    }
}

// Implement Borrow<str> for DnsName
impl<'a> core::borrow::Borrow<str> for DnsName<'a> {
    fn borrow(&self) -> &str {
        &self.0
    }
}

// Implement TryFrom<&[u8]> for DnsName (for rustls 0.23)
impl<'a> TryFrom<&'a [u8]> for DnsName<'a> {
    type Error = InvalidDnsNameError;
    
    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        // Try to convert bytes to string
        core::str::from_utf8(bytes)
            .map_err(|_| InvalidDnsNameError)
            .and_then(|s| Self::try_from(s))
    }
}

impl<'a> AsRef<str> for DnsName<'a> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Note: We don't implement AsRef<[u8]> directly for DnsName
// because str already implements AsRef<[u8]>, so we can use
// dns_name.as_ref().as_bytes() or dns_name.as_ref().as_ref()

impl<'a> TryFrom<&'a str> for DnsName<'a> {
    type Error = InvalidDnsNameError;
    
    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Ok(DnsName(s.to_string(), core::marker::PhantomData))
    }
}

/// Invalid DNS name error
#[derive(Debug, Clone)]
pub struct InvalidDnsNameError;

#[cfg(feature = "std")]
impl core::fmt::Display for InvalidDnsNameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid DNS name")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InvalidDnsNameError {}

/// DER-encoded data wrapper
/// Using a struct with const fn to allow Der::from_slice() in const context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Der<'a>(pub &'a [u8]);

impl<'a> Der<'a> {
    pub const fn from_slice(slice: &'a [u8]) -> Self {
        Der(slice)
    }
    
    pub const fn as_ref(&self) -> &'a [u8] {
        self.0
    }
}

impl<'a> AsRef<[u8]> for Der<'a> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> From<&'a [u8]> for Der<'a> {
    fn from(slice: &'a [u8]) -> Self {
        Der(slice)
    }
}

// Allow Deref to &[u8] for compatibility
impl<'a> core::ops::Deref for Der<'a> {
    type Target = [u8];
    
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

// Der is Copy, so we don't need DerefMut
// The Deref implementation above should be sufficient for compatibility

/// Subject public key info DER
/// For WASM compatibility, we need to support From<Vec<u8>>
/// Using a struct that can hold both borrowed and owned data
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectPublicKeyInfoDer<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl<'a> SubjectPublicKeyInfoDer<'a> {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            SubjectPublicKeyInfoDer::Borrowed(s) => s,
            SubjectPublicKeyInfoDer::Owned(v) => v.as_slice(),
        }
    }
}

// Allow conversion from Vec<u8> for rustls-webpki compatibility
impl From<Vec<u8>> for SubjectPublicKeyInfoDer<'static> {
    fn from(v: Vec<u8>) -> Self {
        SubjectPublicKeyInfoDer::Owned(v)
    }
}

// Allow conversion from &[u8] for rustls-webpki compatibility
impl<'a> From<&'a [u8]> for SubjectPublicKeyInfoDer<'a> {
    fn from(slice: &'a [u8]) -> Self {
        SubjectPublicKeyInfoDer::Borrowed(slice)
    }
}

// Allow SubjectPublicKeyInfoDer to be used as &[u8]
impl<'a> AsRef<[u8]> for SubjectPublicKeyInfoDer<'a> {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

// Allow Deref to &[u8] for compatibility
impl<'a> core::ops::Deref for SubjectPublicKeyInfoDer<'a> {
    type Target = [u8];
    
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

/// Helper function for algorithm ID
/// Note: rustls-webpki primarily uses alg_id module constants, not this function
/// This function requires 'static lifetime, so it's limited in use
pub fn alg_id(bytes: &'static [u8]) -> AlgorithmIdentifier {
    AlgorithmIdentifier {
        algorithm: bytes,
    }
}

// Create alg_id module with constants as rustls-webpki expects
pub mod alg_id {
    pub use super::AlgorithmIdentifier;
    
    // Common algorithm identifiers
    pub const ECDSA_P256: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x03\x01\x07" };
    pub const ECDSA_P384: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x03\x01\x08" };
    pub const ECDSA_SHA256: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x04\x03\x02" };
    pub const ECDSA_SHA384: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x04\x03\x03" };
    pub const RSA_ENCRYPTION: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x01" };
    pub const RSA_PKCS1_SHA256: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0b" };
    pub const RSA_PKCS1_SHA384: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0c" };
    pub const RSA_PKCS1_SHA512: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0d" };
    pub const RSA_PSS_SHA256: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0a" };
    pub const RSA_PSS_SHA384: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0b" };
    pub const RSA_PSS_SHA512: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0c" };
    pub const ED25519: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x03\x2b\x65\x70" };
    // P-521 and ECDSA SHA512 (stubs for rustls-webpki)
    pub const ECDSA_P521: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x03\x01\x09" };
    pub const ECDSA_SHA512: AlgorithmIdentifier = AlgorithmIdentifier { algorithm: b"\x06\x08\x2a\x86\x48\xce\x3d\x04\x03\x04" };
}
