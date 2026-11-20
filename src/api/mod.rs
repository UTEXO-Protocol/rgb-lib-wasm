#[cfg(feature = "esplora")]
pub(crate) mod evm;
#[cfg(feature = "esplora")]
pub(crate) mod proxy_v1;

#[cfg(feature = "esplora")]
pub(crate) mod reject_list;

#[cfg(feature = "esplora")]
use super::*;
/// Maximum number of entries of a reject list, kept by an EVM contract or served at a URL. The
/// issuer chooses the source, so a longer list is refused instead of read without bound.
#[cfg(feature = "esplora")]
const MAX_REJECT_LIST_ENTRIES: u64 = 100_000;
/// Time limit of a request to an EVM node or a reject list server. The browser has no connect
/// timeout. Tests use a short one.
#[cfg(all(feature = "esplora", not(test)))]
pub(crate) const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(all(feature = "esplora", test))]
pub(crate) const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
