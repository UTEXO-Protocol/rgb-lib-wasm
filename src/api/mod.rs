#[cfg(feature = "esplora")]
pub(crate) mod evm;
#[cfg(feature = "esplora")]
pub(crate) mod proxy;

#[cfg(feature = "esplora")]
pub(crate) mod reject_list;

#[cfg(feature = "esplora")]
use super::*;
/// Maximum number of entries of a reject list, kept by an EVM contract or served at a URL. The
/// issuer chooses the source, so a longer list is refused instead of read without bound.
#[cfg(feature = "esplora")]
const MAX_REJECT_LIST_ENTRIES: u64 = 100_000;
