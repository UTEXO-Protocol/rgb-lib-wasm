//! RGB wallet
//!
//! This module defines the offline methods of the [`Wallet`] structure and all its related data.

use super::*;

pub(crate) const RGB_LIB_DB_NAME: &str = "rgb_lib_db";
const BDK_DB_NAME: &str = "bdk_db";

pub(crate) const MEDIA_DIR: &str = "media_files";
const TRANSFERS_DIR: &str = "transfers";
const ASSETS_DIR: &str = "assets";

const MIN_BTC_REQUIRED: u64 = 2000;

/// Seconds a deadline must have passed on the local clock before the proxy deadline counts as
/// passed. It matches the proxy's timestamp tolerance.
#[cfg(feature = "esplora")]
pub(crate) const PROXY_CLOCK_SKEW_MARGIN_SECS: u32 = 300;

/// Minimum seconds between an outgoing operation's expiration and the earliest invoice expiration
/// among its recipients. It leaves time to detect the ACKs and broadcast, and covers clock skew.
#[cfg(feature = "esplora")]
pub(crate) const INVOICE_EXPIRATION_MARGIN_SECS: u32 = 2 * PROXY_CLOCK_SKEW_MARGIN_SECS;

/// Maximum time in the future, in seconds, that the expiration of an outgoing operation, or of an
/// invoice it pays, can have: about 10 years. `send_begin` refuses a later one with
/// [`Error::InvalidExpiration`].
pub const MAX_EXPIRATION_SECS: u32 = 10 * 365 * 24 * 60 * 60;

/// The maximum length of a confined collection, given an accessor returning it from a bundle.
const fn bundle_confinement_max<
    C: amplify::confinement::Collection,
    const MIN: usize,
    const MAX: usize,
>(
    _: fn(&TransitionBundle) -> &Confined<C, MIN, MAX>,
) -> usize {
    MAX
}

/// Maximum number of allocations of an asset a UTXO can hold and still be spent. A bundle spends
/// at most as many opouts as its input map holds.
pub(crate) const MAX_UTXO_ASSET_ALLOCATIONS: usize =
    bundle_confinement_max(|b: &TransitionBundle| &b.input_map);

/// Maximum number of allocations of an asset a UTXO can hold (see [`MAX_UTXO_ASSET_ALLOCATIONS`]).
pub(crate) fn max_utxo_asset_allocations() -> usize {
    #[cfg(test)]
    if let Some(max_allocations) = MOCK_MAX_UTXO_ASSET_ALLOCATIONS.with_borrow(|m| *m) {
        return max_allocations;
    }
    MAX_UTXO_ASSET_ALLOCATIONS
}

/// Minimum `retention_grace_secs` a recipient requires from the sender proxy to ACK a transfer.
/// A transfer whose sender proxy grants less is NACKed.
#[cfg(feature = "esplora")]
pub(crate) const MIN_SENDER_RETENTION_GRACE_SECS: u32 = 86400;

/// Seconds a recipient waits after the broadcast deadline (operation expiration plus sender proxy
/// retention grace) before failing a transfer whose TX is unconfirmed.
#[cfg(feature = "esplora")]
pub(crate) const CONFIRMATION_MARGIN_SECS: u32 = 86400;

/// Maximum proof of work difficulty (leading zero bits) the wallet computes for a consignment.
#[cfg(feature = "esplora")]
pub(crate) const MAX_POW_DIFFICULTY: u32 = 28;

pub(crate) const NUM_KNOWN_SCHEMAS: usize = 3;

pub(crate) const MAX_TRANSPORT_ENDPOINTS: usize = 3;

/// Max size, in bytes, of a contract that [`import_contract`](Wallet::import_contract) accepts.
pub(crate) const MAX_CONTRACT_SIZE: u64 = 16 * 1024 * 1024;

pub(crate) const ASSET_ID_PREFIX: &str = "rgb:";
const RGB_BIN_EXTENSION: &str = "rgb";
pub(crate) static CONSIGNMENT_FILE: LazyLock<String> =
    LazyLock::new(|| format!("consignment_out.{RGB_BIN_EXTENSION}"));

static CONSIGNMENT_RCV_FILE: LazyLock<String> =
    LazyLock::new(|| format!("rcv_compose.{RGB_BIN_EXTENSION}"));

pub(crate) const RGB_GLOBAL_BRIDGE_LOCATION: &str = "bridgeLocation";
pub(crate) const RGB_GLOBAL_BURNED_ASSET: &str = "burnedAsset";
pub(crate) const RGB_GLOBAL_BURN_REASON: &str = "burnReason";
pub(crate) const RGB_GLOBAL_BURNED_INFLATION: &str = "burnedInflation";
pub(crate) const RGB_GLOBAL_ISSUED_SUPPLY: &str = "issuedSupply";
pub(crate) const RGB_GLOBAL_LINKED_FROM_CONTRACT: &str = "linkedFromContract";
pub(crate) const RGB_GLOBAL_LINKED_TO_CONTRACT: &str = "linkedToContract";
pub(crate) const RGB_GLOBAL_REJECT_LIST: &str = "rejectList";
pub(crate) const RGB_GLOBAL_REJECT_LIST_URL: &str = "rejectListUrl";
pub(crate) const RGB_METADATA_ALLOWED_INFLATION: &str = "allowedInflation";
pub(crate) const RGB_METADATA_BEFORE_BLOCK: &str = "beforeBlock";

pub(crate) const SCHEMA_ID_NIA: &str =
    "rgb:sch:RWhwUfTMpuP2Zfx1~j4nswCANGeJrYOqDcKelaMV4zU#remote-digital-pegasus";
pub(crate) const SCHEMA_ID_IFA: &str =
    "rgb:sch:A2n9kPNWtXUqwQJyXfY8yGHKyEPZgvrNYYYPGhXawOk#graph-salute-retro";
pub(crate) const SCHEMA_ID_BFA: &str =
    "rgb:sch:3I0xZVEQnQxYVk7vDnh5uNtQyUfsmThw6dX9n_Gvb5o#mama-swing-album";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct LocalAssetData {
    pub(crate) name: String,
    pub(crate) precision: u8,
    pub(crate) ticker: Option<String>,
    pub(crate) details: Option<String>,
    pub(crate) media_idx: Option<i32>,
    pub(crate) initial_supply: u64,
    pub(crate) max_supply: Option<u64>,
    pub(crate) known_circulating_supply: Option<u64>,
    pub(crate) bridge_location: Option<RgbBridgeLocationInfo>,
    pub(crate) reject_list: Option<RgbRejectListInfo>,
    pub(crate) linked_from_contract: Option<String>,
}

#[cfg(test)]
thread_local! {
    /// Seal the genesis of the next IFA asset issued assigns a link right to, if set.
    pub(crate) static MOCK_IFA_LINK_RIGHT: std::cell::RefCell<Option<BuilderSeal<BlindSeal<RgbTxid>>>> =
        const { std::cell::RefCell::new(None) };
    pub(crate) static MOCK_MAX_UTXO_ASSET_ALLOCATIONS: std::cell::RefCell<Option<usize>> =
        const { std::cell::RefCell::new(None) };
    /// Media of the next asset issued, if set.
    pub(crate) static MOCK_ISSUE_MEDIA: std::cell::RefCell<Option<Attachment>> =
        const { std::cell::RefCell::new(None) };
    /// Clock skew margin of the invoice deadline, if set.
    pub(crate) static MOCK_CLOCK_SKEW_MARGIN: std::cell::RefCell<Option<u32>> =
        const { std::cell::RefCell::new(None) };
}

/// Margin for proxy clock skew, added to the deadline of a reusable invoice.
pub(crate) fn clock_skew_margin() -> u32 {
    #[cfg(test)]
    if let Some(margin) = MOCK_CLOCK_SKEW_MARGIN.with(|m| *m.borrow()) {
        return margin;
    }
    PROXY_CLOCK_SKEW_MARGIN_SECS
}

/// The ID of the contract a contract `link` (e.g. [`LinkableSchemaWrapper::link_from`]) points
/// to, if any.
pub(crate) fn contract_link(
    link: Result<Option<ContractId>, LinkError>,
) -> Result<Option<String>, Error> {
    link.map(|contract_id| contract_id.map(|c| c.to_string()))
        .map_err(|e| Error::Internal {
            details: format!("invalid contract link: {e}"),
        })
}

/// The bitcoin balances (in sats) for the vanilla and colored wallets.
///
/// The settled balances include the confirmed balance.
/// The future balances also include the immature balance and the untrusted and trusted pending
/// balances.
/// The spendable balances include the settled balance and also the untrusted and trusted pending
/// balances.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct BtcBalance {
    /// Funds that will never hold RGB assets
    pub vanilla: Balance,
    /// Funds that may hold RGB assets
    pub colored: Balance,
}

/// An asset media file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Media {
    /// Path of the media file
    pub file_path: String,
    /// Digest of the media file
    pub digest: String,
    /// Mime type of the media file
    pub mime: String,
}

impl Media {
    pub(crate) fn get_digest(&self) -> String {
        PathBuf::from(&self.file_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    pub(crate) fn from_attachment<P: AsRef<Path>>(attachment: &Attachment, media_dir: P) -> Self {
        let digest = hex::encode(attachment.digest);
        let file_path = media_dir
            .as_ref()
            .join(&digest)
            .to_string_lossy()
            .to_string();
        Self {
            digest,
            mime: attachment.ty.to_string(),
            file_path,
        }
    }
}

impl Media {
    pub(crate) fn from_db_media<P: AsRef<Path>>(db_media: &DbMedia, media_dir: P) -> Self {
        let digest = db_media.digest.clone();
        let file_path = media_dir
            .as_ref()
            .join(&digest)
            .to_string_lossy()
            .to_string();
        Self {
            digest,
            mime: db_media.mime.clone(),
            file_path,
        }
    }
}

/// The contract of an RGB asset, with the media it references.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ExportedContract {
    /// The contract consignment, with no bundles
    pub contract: Vec<u8>,
    /// The bytes of each media the contract references
    pub media: Vec<Vec<u8>>,
}

/// Metadata of an RGB asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Metadata {
    /// Asset schema type
    pub asset_schema: AssetSchema,
    /// Initial issued supply
    pub initial_supply: u64,
    /// Max issued supply
    pub max_supply: u64,
    /// Known circulating supply
    pub known_circulating_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Asset name
    pub name: String,
    /// Asset precision
    pub precision: u8,
    /// Asset ticker
    pub ticker: Option<String>,
    /// Asset details
    pub details: Option<String>,
    /// Where the bridge contract backing the asset is deployed, which only a BFA asset has
    pub bridge_location: Option<RgbBridgeLocationInfo>,
    /// Where the reject list the asset commits to is published, if any
    pub reject_list: Option<RgbRejectListInfo>,
    /// ID of the contract the asset is linked from (e.g. the one it replaces), if any
    pub linked_from_contract: Option<String>,
    /// ID of the contract the asset has been linked to (e.g. the one replacing it), if linked by
    /// this wallet: a link assigns nothing, so no consignment of a later operation carries it
    pub linked_to_contract: Option<String>,
}

/// A Non-Inflatable Asset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct AssetNIA {
    /// ID of the asset
    pub asset_id: String,
    /// Ticker of the asset
    pub ticker: String,
    /// Name of the asset
    pub name: String,
    /// Details of the asset
    pub details: Option<String>,
    /// Precision, also known as divisibility, of the asset
    pub precision: u8,
    /// Total issued amount
    pub issued_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Timestamp of asset import
    pub added_at: i64,
    /// Current balance of the asset
    pub balance: Balance,
    /// Asset media attachment
    pub media: Option<Media>,
}

impl AssetNIA {
    pub(crate) fn get_asset_details(
        wallet: &Wallet,
        asset: &DbAsset,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
        medias: Option<Vec<DbMedia>>,
    ) -> Result<AssetNIA, Error> {
        let media = {
            let medias = if let Some(m) = medias {
                m
            } else {
                wallet.database.iter_media()?
            };
            medias
                .iter()
                .find(|m| Some(m.idx) == asset.media_idx)
                .map(|m| Media::from_db_media(m, wallet.get_media_dir()))
        };
        let balance = wallet.database.get_asset_balance(
            asset.id.clone(),
            transfers,
            asset_transfers,
            batch_transfers,
            colorings,
            txos,
        )?;
        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        Ok(AssetNIA {
            asset_id: asset.id.clone(),
            ticker: asset.ticker.clone().unwrap(),
            name: asset.name.clone(),
            details: asset.details.clone(),
            precision: asset.precision,
            issued_supply: initial_supply,
            timestamp: asset.timestamp,
            added_at: asset.added_at,
            balance,
            media,
        })
    }
}

/// An Inflatable Fungible Asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct AssetIFA {
    /// ID of the asset
    pub asset_id: String,
    /// Ticker of the asset
    pub ticker: String,
    /// Name of the asset
    pub name: String,
    /// Details of the asset
    pub details: Option<String>,
    /// Precision, also known as divisibility, of the asset
    pub precision: u8,
    /// Initial issued supply
    pub initial_supply: u64,
    /// Max issued supply
    pub max_supply: u64,
    /// Known circulating supply
    pub known_circulating_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Timestamp of asset import
    pub added_at: i64,
    /// Current balance of the asset
    pub balance: Balance,
    /// Asset media attachment
    pub media: Option<Media>,
    /// Where the reject list the asset commits to is published, if any
    pub reject_list: Option<RgbRejectListInfo>,
}

impl AssetIFA {
    pub(crate) fn get_asset_details(
        wallet: &Wallet,
        asset: &DbAsset,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
        medias: Option<Vec<DbMedia>>,
    ) -> Result<AssetIFA, Error> {
        let media = {
            let medias = if let Some(m) = medias {
                m
            } else {
                wallet.database.iter_media()?
            };
            medias
                .iter()
                .find(|m| Some(m.idx) == asset.media_idx)
                .map(|m| Media::from_db_media(m, wallet.get_media_dir()))
        };
        let balance = wallet.database.get_asset_balance(
            asset.id.clone(),
            transfers,
            asset_transfers,
            batch_transfers,
            colorings,
            txos,
        )?;
        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        let max_supply = asset.max_supply.as_ref().unwrap().parse::<u64>().unwrap();
        let known_circulating_supply = asset
            .known_circulating_supply
            .as_ref()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        Ok(AssetIFA {
            asset_id: asset.id.clone(),
            ticker: asset.ticker.clone().unwrap(),
            name: asset.name.clone(),
            details: asset.details.clone(),
            precision: asset.precision,
            initial_supply,
            max_supply,
            known_circulating_supply,
            timestamp: asset.timestamp,
            added_at: asset.added_at,
            balance,
            media,
            reject_list: asset.reject_list.clone(),
        })
    }
}

/// A Bridged Fungible Asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct AssetBFA {
    /// ID of the asset
    pub asset_id: String,
    /// Ticker of the asset
    pub ticker: String,
    /// Name of the asset
    pub name: String,
    /// Details of the asset
    pub details: Option<String>,
    /// Precision, also known as divisibility, of the asset
    pub precision: u8,
    /// Amount locked on the bridge contract, as read at the last mint end or asset receive.
    /// It is 0 if the wallet never read it. It counts incomplete mints, so it is an upper bound
    pub known_circulating_supply: u64,
    /// Timestamp of asset genesis
    pub timestamp: i64,
    /// Timestamp of asset import
    pub added_at: i64,
    /// Current balance of the asset
    pub balance: Balance,
    /// Asset media attachment
    pub media: Option<Media>,
    /// Where the bridge contract backing the asset is deployed
    pub bridge_location: RgbBridgeLocationInfo,
    /// Where the reject list the asset commits to is published, if any
    pub reject_list: Option<RgbRejectListInfo>,
}

impl AssetBFA {
    pub(crate) fn get_asset_details(
        wallet: &Wallet,
        asset: &DbAsset,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
        medias: Option<Vec<DbMedia>>,
    ) -> Result<AssetBFA, Error> {
        let media = {
            let medias = if let Some(m) = medias {
                m
            } else {
                wallet.database.iter_media()?
            };
            medias
                .iter()
                .find(|m| Some(m.idx) == asset.media_idx)
                .map(|m| Media::from_db_media(m, wallet.get_media_dir()))
        };
        let balance = wallet.database.get_asset_balance(
            asset.id.clone(),
            transfers,
            asset_transfers,
            batch_transfers,
            colorings,
            txos,
        )?;
        let known_circulating_supply = asset
            .known_circulating_supply
            .as_ref()
            .map_or(0, |s| s.parse::<u64>().unwrap());
        Ok(AssetBFA {
            asset_id: asset.id.clone(),
            ticker: asset.ticker.clone().unwrap(),
            name: asset.name.clone(),
            details: asset.details.clone(),
            precision: asset.precision,
            known_circulating_supply,
            timestamp: asset.timestamp,
            added_at: asset.added_at,
            balance,
            media,
            bridge_location: asset
                .bridge_location
                .clone()
                .expect("a BFA asset has a bridge location"),
            reject_list: asset.reject_list.clone(),
        })
    }
}

/// List of RGB assets, grouped by asset schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Assets {
    /// List of NIA assets
    pub nia: Option<Vec<AssetNIA>>,
    /// List of IFA assets
    pub ifa: Option<Vec<AssetIFA>>,
    /// List of BFA assets
    pub bfa: Option<Vec<AssetBFA>>,
}

/// A balance.
///
/// This structure is used both for RGB assets and BTC balances (in sats). When used for a BTC
/// balance it can be used both for the vanilla wallet and the colored wallet.
///
/// An RGB balance above `u64::MAX` is capped to `u64::MAX`. For example, pending incoming
/// transfers that double-spend each other can make the future balance exceed it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Balance {
    /// Settled balance, based on operations that have reached the final status
    pub settled: u64,
    /// Future balance, including settled operations plus ones are not yet finalized
    pub future: u64,
    /// Spendable balance, only including balance that can actually be spent. It's a subset of the
    /// settled balance. For the RGB balance this excludes the allocations on UTXOs related to
    /// pending operations
    pub spendable: u64,
}

/// Data to receive an RGB transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ReceiveData {
    /// Invoice string
    pub invoice: String,
    /// ID of the receive operation (blinded UTXO or Bitcoin script)
    pub recipient_id: String,
    /// Expiration of the receive operation
    pub expiration_timestamp: u64,
    /// Batch transfer idx
    pub batch_transfer_idx: i32,
    /// Recipient's Ed25519 public key (hex-encoded), as embedded in the invoice
    pub public_key: String,
}

/// Data of a receive operation, ready to be stored.
pub(crate) struct ReceiveDataInternal {
    asset_id: Option<String>,
    detected_assignment: Assignment,
    invoice_string: String,
    recipient_id: String,
    endpoints: Vec<String>,
    created_at: i64,
    expiration_timestamp: i64,
    recipient_type_full: RecipientTypeFull,
    blind_seal: Option<GraphSeal>,
    script_pubkey: Option<ScriptBuf>,
    operation_secret_key: Vec<u8>,
    recipient_pubkey: Vec<u8>,
    encryption: Encryption,
    retention_deadline: Option<i64>,
    quota: u32,
    invoice_deadline: Option<i64>,
    receive_dir: Option<String>,
}

/// Supported database types.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum DatabaseType {
    /// A SQLite database
    Sqlite,
}

/// A bitcoin address.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Address {
    /// The bitcoin address string
    address_string: String,
    /// The bitcoin network of the address
    bitcoin_network: BitcoinNetwork,
}

impl Address {
    /// Parse the provided `address_string`.
    /// Throws an error if the provided string is not a valid bitcoin address for the given
    /// network.
    pub fn new(address_string: String, bitcoin_network: BitcoinNetwork) -> Result<Self, Error> {
        parse_address_str(&address_string, bitcoin_network)?;
        Ok(Address {
            address_string,
            bitcoin_network,
        })
    }
}

/// Data for operations that require the wallet to be online.
///
/// Methods not requiring an `Online` object don't need network access and can be performed
/// offline. Methods taking an optional `Online` will operate offline when it's missing and will
/// use local data only.
///
/// <div class="warning">This should not be manually constructed but should be obtained from the
/// [`Wallet::go_online`] method.</div>
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Online {
    /// Unique ID for this object
    pub id: u64,
    /// URL of the indexer server to be used for online operations
    pub indexer_url: String,
}

impl From<DbTxo> for Outpoint {
    fn from(x: DbTxo) -> Outpoint {
        Outpoint {
            txid: x.txid,
            vout: x.vout,
        }
    }
}

/// A recipient of an RGB transfer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Recipient {
    /// Recipient ID
    pub recipient_id: String,
    /// Witness data (to be provided only with a witness recipient)
    pub witness_data: Option<WitnessData>,
    /// RGB assignment
    pub assignment: Assignment,
    /// Transport endpoints
    pub transport_endpoints: Vec<String>,
    /// Recipient's Ed25519 public key (hex-encoded), taken from the invoice `pk` field.
    pub public_key: Option<String>,
    /// The recipient's consignment/media encryption preference, taken from the invoice `enc`
    /// field. This is not a choice of the sender, who must honor it.
    pub encryption: Encryption,
    /// The invoice expiration (UNIX timestamp, seconds), taken from the invoice.
    ///
    /// The transfer cannot be created after it and the operation expiration needs to leave the
    /// time to detect the ACK and broadcast the TX before it.
    pub expiration_timestamp: Option<u64>,
}

/// A mint received by the wallet, as checked by it: what its sender has to lock on the external
/// chain for the mint to complete.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ReceivedMint {
    /// ID of the mint operation
    pub opid: String,
    /// ID the lock has to be made for: it commits to the mint operation and to the TX including it
    pub lock_id: String,
    /// Amount minted, which the lock has to match
    pub amount: u64,
    /// External chain block the lock has to be included before
    pub before_block: u64,
    /// EIP-155 chain ID of the EVM chain the asset's bridge contract lives on
    pub evm_chain_id: u64,
    /// Address of the asset's bridge contract, where the lock has to be made
    pub bridge_contract: String,
}

/// The mint locks a received consignment waits for, saved once it is validated as far as it can
/// be without them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct PendingMintLocks {
    /// SHA-256 digest (hex) of the consignment waiting for the locks
    pub(crate) consignment_digest: String,
    /// EIP-155 chain ID of the EVM chain the bridge contract lives on
    pub(crate) evm_chain_id: u64,
    /// Address of the bridge contract that records the locks
    pub(crate) bridge_contract: EvmAddress,
    /// The claims of the mints whose locks are not finalized yet
    pub(crate) claims: Vec<Claim>,
}

/// The information needed to receive RGB assets in witness mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WitnessData {
    /// The Bitcoin amount (in sats) to send to the recipient
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub amount_sat: u64,
    /// An optional blinding
    #[serde(deserialize_with = "from_str_or_number_optional")]
    pub blinding: Option<u64>,
}

/// An RGB allocation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbAllocation {
    /// Asset ID
    pub asset_id: Option<String>,
    /// RGB assignment
    pub assignment: Assignment,
    /// Defines if the allocation is settled, meaning it refers to a transfer in the
    /// [`TransferStatus::Settled`] status
    pub settled: bool,
}

impl From<LocalRgbAllocation> for RgbAllocation {
    fn from(x: LocalRgbAllocation) -> RgbAllocation {
        RgbAllocation {
            asset_id: x.asset_id.clone(),
            assignment: x.assignment.clone(),
            settled: x.settled(),
        }
    }
}

/// A Bitcoin transaction.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Transaction {
    /// Type of transaction
    pub transaction_type: TransactionType,
    /// Transaction ID
    pub txid: String,
    /// Received value (in sats), computed as the sum of owned output amounts included in this
    /// transaction
    pub received: u64,
    /// Sent value (in sats), computed as the sum of owned input amounts included in this
    /// transaction
    pub sent: u64,
    /// Fee value (in sats)
    pub fee: u64,
    /// Height and Unix timestamp of the block containing the transaction if confirmed, `None` if
    /// unconfirmed
    pub confirmation_time: Option<BlockTime>,
    /// Whether the transaction belongs to a begun vanilla operation that is not ended yet
    pub pending: bool,
}

/// Block height and timestamp of a block.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockTime {
    /// Confirmation block height
    pub height: u32,
    /// Confirmation block timestamp
    pub timestamp: u64,
}

/// The type of a transaction.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum TransactionType {
    /// Transaction used to perform an RGB send
    RgbSend,
    /// Transaction used to drain the RGB wallet
    Drain,
    /// Transaction used to create UTXOs
    CreateUtxos,
    /// Transaction used to perform a BTC send
    SendBtc,
    /// Incoming transaction
    Incoming,
}

/// Information about a PSBT input for inspection.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtInputInfo {
    /// The outpoint being spent
    pub outpoint: Outpoint,
    /// The amount in satoshis
    pub amount_sat: u64,
    /// Whether this input belongs to the wallet's descriptors
    pub is_mine: bool,
}

/// Information about a PSBT output for inspection.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtOutputInfo {
    /// The output address (if parseable)
    pub address: Option<String>,
    /// The output script pubkey (hex encoded)
    pub script_pubkey_hex: String,
    /// The amount in satoshis
    pub amount_sat: u64,
    /// Whether this is an OP_RETURN output
    pub is_op_return: bool,
    /// Whether this is an output that will belong to the wallet
    pub is_mine: bool,
}

/// Inspection results for a PSBT.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PsbtInspection {
    /// Transaction ID
    pub txid: String,
    /// List of inputs
    pub inputs: Vec<PsbtInputInfo>,
    /// List of outputs
    pub outputs: Vec<PsbtOutputInfo>,
    /// Total input amount in satoshis
    pub total_input_sat: u64,
    /// Total output amount in satoshis
    pub total_output_sat: u64,
    /// Calculated fee in satoshis
    pub fee_sat: u64,
    /// Number of signatures present
    pub signature_count: u16,
    /// Transaction size in virtual bytes
    pub size_vbytes: u64,
}

/// A begun vanilla operation that is not ended or aborted.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct PendingVanillaTx {
    /// Transaction ID
    pub txid: String,
    /// Type of the vanilla operation
    pub r#type: WalletTransactionType,
}

/// An asset filter.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum AssetFilter {
    /// Match any or no asset
    AnyOrNone,
    /// Match no asset
    None,
    /// Match the asset with the given ID
    Id(String),
}

/// Which transfers to list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransferScope {
    /// All transfers
    All,
    /// Transfers committed by the on-chain transaction with the given TXID
    Txid(String),
    /// Transfers belonging to the batch transfer with the given index
    BatchTransfer(i32),
}

/// Proxy data of a received consignment, saved until the transfer ends.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct ReceivedConsignmentMeta {
    pub(crate) proxy_url: Option<String>,
    pub(crate) transfer_id: Option<String>,
    pub(crate) sender_transport_endpoint: String,
    pub(crate) sender_pubkey: String,
    pub(crate) consignment_hash: String,
    pub(crate) batch: bool,
    /// History TXs that must reach the safe height before the ACK
    pub(crate) unsafe_txids: BTreeSet<String>,
}

/// An RGB transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Transfer {
    /// ID of the transfer
    pub idx: i32,
    /// ID of the batch transfer containing this transfer
    pub batch_transfer_idx: i32,
    /// Timestamp of the transfer creation
    pub created_at: i64,
    /// Timestamp of the transfer last update
    pub updated_at: i64,
    /// Status of the transfer
    pub status: TransferStatus,
    /// Requested RGB assignment
    pub requested_assignment: Option<Assignment>,
    /// RGB assignmnents
    pub assignments: Vec<Assignment>,
    /// Type of the transfer
    pub kind: TransferKind,
    /// ID of the Bitcoin transaction anchoring the transfer
    pub txid: Option<String>,
    /// Recipient ID (blinded UTXO or Bitcoin script) of an incoming transfer
    pub recipient_id: Option<String>,
    /// UTXO of an incoming transfer
    pub receive_utxo: Option<Outpoint>,
    /// UTXOs receiving the RGB change of an outgoing transfer, as it can be spread over several
    pub change_utxos: Vec<Outpoint>,
    /// Expiration of the transfer
    pub expiration_timestamp: Option<u64>,
    /// Transport endpoints for the transfer
    pub transport_endpoints: Vec<TransferTransportEndpoint>,
    /// Sender proxy endpoint of the transfer, where the ACK is posted and the signed TX released
    pub sender_transport_endpoint: Option<String>,
    /// Invoice string of the incoming transfer
    pub invoice_string: Option<String>,
    /// Consignment path
    pub consignment_path: Option<String>,
    /// Unsigned PSBT (BIP-174 bytes) produced by the `_begin` step, when available
    pub psbt: Option<Vec<u8>>,
    /// Hex-encoded 32-byte reason of a burn transfer, if any
    pub burn_reason: Option<String>,
}

impl Transfer {
    fn from_db_transfer(
        x: &DbTransfer,
        td: TransferData,
        transport_endpoints: Vec<TransferTransportEndpoint>,
        sender_transport_endpoint: Option<String>,
    ) -> Transfer {
        Transfer {
            idx: x.idx,
            batch_transfer_idx: td.batch_transfer_idx,
            created_at: td.created_at,
            updated_at: td.updated_at,
            status: td.status,
            requested_assignment: x.requested_assignment.clone(),
            assignments: td.assignments,
            kind: td.kind,
            txid: td.txid,
            recipient_id: x.recipient_id.clone(),
            receive_utxo: td.receive_utxo,
            change_utxos: td.change_utxos,
            expiration_timestamp: td.expiration.map(|t| t as u64),
            transport_endpoints,
            sender_transport_endpoint,
            invoice_string: x.invoice_string.clone(),
            consignment_path: td.consignment_path,
            psbt: td.psbt,
            burn_reason: x.burn_reason.clone(),
        }
    }
}

/// An RGB transport endpoint for a transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct TransferTransportEndpoint {
    /// Endpoint address
    pub endpoint: String,
    /// Endpoint transport type
    pub transport_type: TransportType,
    /// Whether the endpoint has been used
    pub used: bool,
}

impl TransferTransportEndpoint {
    fn from_db_transfer_transport_endpoint(
        x: &DbTransferTransportEndpoint,
        ce: &DbTransportEndpoint,
    ) -> TransferTransportEndpoint {
        TransferTransportEndpoint {
            endpoint: ce.endpoint.clone(),
            transport_type: ce.transport_type,
            used: x.used,
        }
    }
}

/// The type of an RGB transfer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransferKind {
    /// A transfer that issued the asset
    Issuance,
    /// An incoming transfer via blinded UTXO
    ReceiveBlind,
    /// An incoming transfer via a Bitcoin script (witness TX)
    ReceiveWitness,
    /// An outgoing transfer
    Send,
    /// An inflation transfer
    Inflation,
    /// A burn transfer
    Burn,
    /// A link transfer
    Link,
    /// A mint transfer
    Mint,
}

/// A wallet unspent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Unspent {
    /// Bitcoin UTXO
    pub utxo: Utxo,
    /// RGB allocations on the UTXO
    pub rgb_allocations: Vec<RgbAllocation>,
    /// Number of pending blind receive operations
    pub pending_blinded: u32,
}

impl From<LocalUnspent> for Unspent {
    fn from(x: LocalUnspent) -> Unspent {
        Unspent {
            utxo: Utxo::from(x.utxo),
            rgb_allocations: x
                .rgb_allocations
                .into_iter()
                .map(RgbAllocation::from)
                .collect::<Vec<RgbAllocation>>(),
            pending_blinded: x.pending_blinded,
        }
    }
}

impl From<LocalOutput> for Unspent {
    fn from(x: LocalOutput) -> Unspent {
        Unspent {
            utxo: Utxo::from(x),
            rgb_allocations: vec![],
            pending_blinded: 0,
        }
    }
}

/// A Bitcoin unspent transaction output.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Utxo {
    /// UTXO outpoint
    pub outpoint: Outpoint,
    /// Amount (in sats)
    pub btc_amount: u64,
    /// Defines if the UTXO can have RGB allocations
    pub colorable: bool,
    /// Defines if the UTXO already exists (TX that creates it has been broadcasted)
    pub exists: bool,
    /// Derivation index of the UTXO's script pubkey. `None` if the transaction creating the UTXO
    /// is not known to the underlying BDK wallet, which is the case while the UTXO does not yet
    /// exist on-chain and for outputs that do not belong to this wallet.
    pub derivation_index: Option<u32>,
}

impl From<DbTxo> for Utxo {
    fn from(x: DbTxo) -> Utxo {
        Utxo {
            outpoint: x.outpoint(),
            btc_amount: x
                .btc_amount
                .parse::<u64>()
                .expect("DB should contain a valid u64 value"),
            colorable: true,
            exists: x.exists,
            derivation_index: None,
        }
    }
}

impl From<LocalOutput> for Utxo {
    fn from(x: LocalOutput) -> Utxo {
        Utxo {
            outpoint: Outpoint::from(x.outpoint),
            btc_amount: x.txout.value.to_sat(),
            colorable: false,
            exists: true,
            derivation_index: Some(x.derivation_index),
        }
    }
}

/// Data that defines a [`Wallet`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WalletData {
    /// Directory where the wallet directory is stored
    pub data_dir: String,
    /// Bitcoin network for the wallet
    pub bitcoin_network: BitcoinNetwork,
    /// Database type for the wallet
    pub database_type: DatabaseType,
    /// The max number of RGB allocations allowed per UTXO
    #[serde(deserialize_with = "from_str_or_number_mandatory")]
    pub max_allocations_per_utxo: u32,
    /// List of schemas the wallet should support (when issuing, sending and receiving). Empty list
    /// means the wallet should support all the schemas rgb-lib supports.
    pub supported_schemas: Vec<AssetSchema>,
    /// When `true`, address generation returns a pinned address instead of creating new ones.
    /// Default: `false`
    #[serde(default)]
    pub reuse_addresses: bool,
    /// When `false`, a mainnet wallet that supports IFA or BFA fails with
    /// [`crate::Error::CannotUseSchemaOnMainnet`]. When `true`, it is created and a warning is logged.
    /// Default: `false`
    #[serde(default)]
    pub allow_reckless_schemas: bool,
}

/// Keys for the singlesig wallet.
#[derive(Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct SinglesigKeys {
    /// Wallet account-level xPub for the vanilla side of the wallet
    pub account_xpub_vanilla: String,
    /// Wallet account-level xPub for the colored side of the wallet
    pub account_xpub_colored: String,
    /// Keychain index for the vanilla-side of the wallet (default: 0)
    #[serde(deserialize_with = "from_str_or_number_optional")]
    pub vanilla_keychain: Option<u8>,
    /// Wallet master fingerprint
    pub master_fingerprint: String,
    /// Witness version these keys were derived with
    pub witness_version: WitnessVersion,
    /// Wallet mnemonic phrase
    pub mnemonic: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for SinglesigKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinglesigKeys")
            .field("account_xpub_vanilla", &self.account_xpub_vanilla)
            .field("account_xpub_colored", &self.account_xpub_colored)
            .field("vanilla_keychain", &self.vanilla_keychain)
            .field("master_fingerprint", &self.master_fingerprint)
            .field("witness_version", &self.witness_version)
            .field("mnemonic", &self.mnemonic.as_ref().map(|_| crate::REDACTED))
            .finish()
    }
}

impl SinglesigKeys {
    pub(crate) fn build_descriptors(
        &self,
        bitcoin_network: BitcoinNetwork,
    ) -> Result<(WalletDescriptors, bool), Error> {
        let bdk_network = BdkNetwork::from(bitcoin_network);
        let xpub_rgb = str_to_xpub(&self.account_xpub_colored, bdk_network)?;
        let xpub_btc = str_to_xpub(&self.account_xpub_vanilla, bdk_network)?;
        let ((colored, vanilla), watch_only) = if let Some(mnemonic) = &self.mnemonic {
            let descs = get_descriptors(
                bitcoin_network,
                mnemonic,
                self.vanilla_keychain,
                xpub_btc,
                xpub_rgb,
                self.witness_version,
            )?;
            // check master fingerprint derived from mnemonic matches provided one
            let mnemonic = Mnemonic::parse_in(Language::English, mnemonic.as_str())?;
            let master_xprv = Xpriv::new_master(bitcoin_network, &mnemonic.to_seed("")).unwrap();
            let master_xpub = Xpub::from_priv(&Secp256k1::new(), &master_xprv);
            if master_xpub.fingerprint()
                != Fingerprint::from_str(&self.master_fingerprint)
                    .map_err(|_| Error::InvalidFingerprint)?
            {
                return Err(Error::FingerprintMismatch);
            }
            (descs, false)
        } else {
            let descs = get_descriptors_from_xpubs(
                bitcoin_network,
                &self.master_fingerprint,
                xpub_rgb,
                xpub_btc,
                self.vanilla_keychain,
                self.witness_version,
            )?;
            (descs, true)
        };
        Ok((WalletDescriptors { colored, vanilla }, watch_only))
    }

    /// Create a new [`SinglesigKeys`] from a [`crate::keys::Keys`] object.
    pub fn from_keys(keys: &crate::keys::Keys, vanilla_keychain: Option<u8>) -> Self {
        Self {
            mnemonic: Some(keys.mnemonic.clone()),
            ..Self::from_keys_no_mnemonic(keys, vanilla_keychain)
        }
    }

    /// Create a new [`SinglesigKeys`] from a [`crate::keys::Keys`] object without a mnemonic.
    pub fn from_keys_no_mnemonic(keys: &crate::keys::Keys, vanilla_keychain: Option<u8>) -> Self {
        Self {
            account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
            account_xpub_colored: keys.account_xpub_colored.clone(),
            vanilla_keychain,
            master_fingerprint: keys.master_fingerprint.clone(),
            witness_version: keys.witness_version,
            mnemonic: None,
        }
    }
}

/// Descriptors for an RGB wallet.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WalletDescriptors {
    /// Colored descriptor
    pub colored: String,
    /// Vanilla descriptor
    pub vanilla: String,
}

// The non-secret settings and keys of a wallet, saved with each snapshot and backup.
// The mnemonic is never stored here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WalletManifest {
    pub(crate) bitcoin_network: BitcoinNetwork,
    pub(crate) database_type: DatabaseType,
    pub(crate) max_allocations_per_utxo: u32,
    pub(crate) supported_schemas: Vec<AssetSchema>,
    pub(crate) reuse_addresses: bool,
    pub(crate) allow_reckless_schemas: bool,
    pub(crate) account_xpub_vanilla: String,
    pub(crate) account_xpub_colored: String,
    pub(crate) vanilla_keychain: u8,
    pub(crate) master_fingerprint: String,
    pub(crate) witness_version: WitnessVersion,
}

impl WalletManifest {
    pub(crate) fn new(wallet_data: &WalletData, keys: &SinglesigKeys) -> Self {
        Self {
            bitcoin_network: wallet_data.bitcoin_network,
            database_type: wallet_data.database_type.clone(),
            max_allocations_per_utxo: wallet_data.max_allocations_per_utxo,
            supported_schemas: wallet_data.supported_schemas.clone(),
            reuse_addresses: wallet_data.reuse_addresses,
            allow_reckless_schemas: wallet_data.allow_reckless_schemas,
            account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
            account_xpub_colored: keys.account_xpub_colored.clone(),
            vanilla_keychain: keys.vanilla_keychain.unwrap_or(KEYCHAIN_BTC),
            master_fingerprint: keys.master_fingerprint.clone(),
            witness_version: keys.witness_version,
        }
    }

    // Fail if wallet_data or keys disagree with the settings fixed at wallet creation.
    pub(crate) fn check_settings_unchanged(
        &self,
        wallet_data: &WalletData,
        keys: &SinglesigKeys,
    ) -> Result<(), Error> {
        let requested = Self::new(wallet_data, keys);

        if self.bitcoin_network != requested.bitcoin_network {
            return Err(Error::BitcoinNetworkMismatch);
        }

        macro_rules! check {
            ($($field:ident),+ $(,)?) => {
                $(if self.$field != requested.$field {
                    return Err(Error::WalletSettingMismatch {
                        setting: stringify!($field).to_string(),
                        expected: format!("{:?}", self.$field),
                        provided: format!("{:?}", requested.$field),
                    });
                })+
            };
        }

        // The witness version comes before the xPubs it derives.
        check!(
            master_fingerprint,
            witness_version,
            vanilla_keychain,
            account_xpub_colored,
            account_xpub_vanilla,
        );
        Ok(())
    }

    pub(crate) fn into_parts(
        self,
        data_dir: String,
        mnemonic: Option<Zeroizing<String>>,
    ) -> (WalletData, SinglesigKeys) {
        (
            WalletData {
                data_dir,
                bitcoin_network: self.bitcoin_network,
                database_type: self.database_type,
                max_allocations_per_utxo: self.max_allocations_per_utxo,
                supported_schemas: self.supported_schemas,
                reuse_addresses: self.reuse_addresses,
                allow_reckless_schemas: self.allow_reckless_schemas,
            },
            SinglesigKeys {
                account_xpub_vanilla: self.account_xpub_vanilla,
                account_xpub_colored: self.account_xpub_colored,
                vanilla_keychain: Some(self.vanilla_keychain),
                master_fingerprint: self.master_fingerprint,
                witness_version: self.witness_version,
                mnemonic,
            },
        )
    }
}

pub(crate) type BdkPersister = super::memory_store::MemoryStore<ChangeSet>;

/// Build the colored and vanilla signers of a wallet from its descriptors.
pub(crate) fn build_signers(
    bdk_wallet: &BdkWallet,
    desc_colored: &str,
    desc_vanilla: &str,
    network_kind: NetworkKind,
) -> Result<[SignersContainer; 2], Error> {
    let secp = bdk_wallet.secp_ctx();
    let build = |desc: &str, keychain| -> Result<SignersContainer, Error> {
        let (_, keymap) = desc
            .into_wallet_descriptor(secp, network_kind)
            .map_err(|e| Error::Internal {
                details: e.to_string(),
            })?;
        Ok(SignersContainer::build(
            keymap,
            bdk_wallet.public_descriptor(keychain),
            secp,
        ))
    };
    Ok([
        build(desc_colored, KeychainKind::External)?,
        build(desc_vanilla, KeychainKind::Internal)?,
    ])
}

pub(crate) type LoggerGuard = ();

pub(crate) type RgbLibDb = crate::database::memory_db::InMemoryDb;

/// An RGB wallet.
///
/// This should not be manually constructed but should be obtained from the [`Wallet::new`]
/// method.
pub struct Wallet {
    pub(crate) wallet_data: WalletData,
    /// Keys the wallet was created with. The mnemonic is missing for a watch-only wallet.
    pub(crate) keys: SinglesigKeys,
    pub(crate) logger: Logger,
    pub(crate) _logger_guard: LoggerGuard,
    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) watch_only: bool,
    pub(crate) database: Arc<RgbLibDb>,
    pub(crate) wallet_dir: PathBuf,
    pub(crate) bdk_wallet: PersistedWallet<BdkPersister>,
    /// Colored and vanilla signers. They are empty for a watch-only wallet.
    pub(crate) signers: [SignersContainer; 2],
    pub(crate) bdk_database: BdkPersister,
    /// Pinned derivation index per keychain for address reuse.
    pub(crate) reuse_address_index: HashMap<KeychainKind, u32>,
    #[cfg(feature = "esplora")]
    pub(crate) online_data: Option<OnlineData>,
    /// In-memory storage for transfer artifacts (replaces filesystem on wasm32).
    /// Key: txid
    #[cfg(feature = "esplora")]
    pub(crate) transfer_artifacts: HashMap<String, super::online::TransferArtifacts>,
    /// In-memory storage for received consignment bytes (replaces filesystem on wasm32).
    /// Key: receive dir of the transfer
    #[cfg(feature = "esplora")]
    pub(crate) received_consignments: HashMap<String, Vec<u8>>,
    /// Validated copy of each received consignment, kept for settlement. Key: receive dir of the transfer
    #[cfg(feature = "esplora")]
    pub(crate) received_valid_consignments: HashMap<String, ValidTransfer>,
    /// Proxy data of each received consignment. Key: receive dir of the transfer
    #[cfg(feature = "esplora")]
    pub(crate) received_consignment_meta: HashMap<String, ReceivedConsignmentMeta>,
    /// Signed TX released by the sender proxy. Key: receive dir of the transfer
    #[cfg(feature = "esplora")]
    pub(crate) received_signed_txs: HashMap<String, Vec<u8>>,
    /// Received mints, as checked. Key: receive dir of the transfer
    pub(crate) received_mints: HashMap<String, ReceivedMint>,
    /// Media bytes keyed by hex digest (replaces the media directory on wasm32).
    pub(crate) media: HashMap<String, Vec<u8>>,
    /// Mint locks a received consignment waits for. Key: receive dir of the transfer
    pub(crate) pending_mint_locks: HashMap<String, PendingMintLocks>,
    /// TXs this wallet broadcast in the running operation. A failed operation keeps them in BDK.
    #[cfg(feature = "esplora")]
    pub(crate) broadcast_txs: Vec<BdkTransaction>,
    pub(crate) idb_queue: std::rc::Rc<super::idb_store::SaveQueue>,
    pub(crate) idb_sequence: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Random id of this object in stored snapshots.
    pub(crate) idb_writer: u64,
    /// Writer and sequence of the snapshot loaded from IndexedDB.
    pub(crate) idb_loaded: Option<(u64, u64)>,
    /// Persistent in-memory RGB stock.
    pub(crate) rgb_stock: std::rc::Rc<std::cell::RefCell<Option<crate::utils::MemStock>>>,
    pub(crate) backup_store: Option<std::rc::Rc<super::backup_store::BackupTarget>>,
    pub(crate) backup_versions: std::cell::RefCell<HashMap<String, i64>>,
    pub(crate) last_vss_backup_error: std::cell::RefCell<Option<String>>,
}

impl Wallet {
    /// Create a wallet and restore its latest durable IndexedDB snapshot, if one exists.
    ///
    /// The returned wallet is ready for use only after the IndexedDB read and snapshot
    /// restoration complete. IndexedDB and snapshot decoding failures are returned to the caller.
    /// A snapshot saved with another fingerprint, witness version, vanilla keychain or account xPub gives
    /// [`Error::WalletSettingMismatch`]. Other settings come from `wallet_data` and are saved
    /// with the next save.
    /// It asks the browser to keep the storage. A refusal does not fail the call.
    pub async fn restore(wallet_data: WalletData, keys: SinglesigKeys) -> Result<Self, Error> {
        let wallet = Self::new(wallet_data, keys)?;
        let snapshot = super::idb_store::load_snapshot(&wallet.idb_key())
            .await
            .map_err(|details| Error::Persistence { details })?;
        wallet.apply_snapshot(snapshot)
    }

    /// Load a saved wallet, identified by its data directory, network and master fingerprint.
    ///
    /// The settings come from the last save. Pass the `mnemonic` to load a wallet able to sign,
    /// or `None` to load it watch-only. A wallet that was never saved cannot be loaded.
    pub async fn load(
        data_dir: &str,
        bitcoin_network: BitcoinNetwork,
        master_fingerprint: &str,
        mnemonic: Option<String>,
    ) -> Result<Self, Error> {
        let key = idb_key_for(
            &PathBuf::from(data_dir).join(master_fingerprint),
            bitcoin_network,
        );
        let snapshot = super::idb_store::load_snapshot(&key)
            .await
            .map_err(|details| Error::Persistence { details })?
            .ok_or(Error::InexistentWalletManifest { path: key })?;
        if snapshot.manifest.master_fingerprint != master_fingerprint {
            return Err(Error::FingerprintMismatch);
        }
        let (wallet_data, keys) = snapshot
            .manifest
            .clone()
            .into_parts(data_dir.to_string(), mnemonic.map(Zeroizing::new));
        Self::new(wallet_data, keys)?.apply_snapshot(Some(snapshot))
    }

    fn apply_snapshot(
        mut self,
        snapshot: Option<super::idb_store::WalletSnapshot>,
    ) -> Result<Self, Error> {
        if let Some(snapshot) = snapshot {
            self.idb_loaded = Some((snapshot.writer, snapshot.sequence));
            self.idb_sequence
                .store(snapshot.sequence, std::sync::atomic::Ordering::SeqCst);
            self.restore_from_snapshot(snapshot)?;
        }
        super::idb_store::request_persistent_storage();
        Ok(self)
    }

    /// Create a new RGB wallet based on the provided [`WalletData`] and [`SinglesigKeys`].
    ///
    /// It does not load IndexedDB. It cannot save over a stored snapshot; use [`Wallet::restore`]
    /// or [`Wallet::load`] for that.
    #[allow(clippy::arc_with_non_send_sync)] // wasm32 is single-threaded; Arc matches native API
    pub fn new(wallet_data: WalletData, keys: SinglesigKeys) -> Result<Self, Error> {
        let wdata = wallet_data.clone();

        let (descs, watch_only) = keys.build_descriptors(wdata.bitcoin_network)?;
        let bdk_network = BdkNetwork::from(wdata.bitcoin_network);
        if wdata.max_allocations_per_utxo == 0 {
            return Err(Error::NoMaxAllocationsPerUtxo);
        }

        // wallet directory and file logging setup
        let wallet_dir = PathBuf::from(&wdata.data_dir).join(&keys.master_fingerprint);
        let (logger, _logger_guard) = setup_logger(&wallet_dir, None)?;
        info!(logger.clone(), "New wallet in '{:?}'", wallet_dir);

        // BDK setup
        let WalletDescriptors {
            colored: desc_colored,
            vanilla: desc_vanilla,
        } = descs;
        let chain_net: ChainNet = wdata.bitcoin_network.into();
        let wallet_params = BdkWallet::load()
            .descriptor(KeychainKind::External, Some(desc_colored.clone()))
            .descriptor(KeychainKind::Internal, Some(desc_vanilla.clone()))
            .check_genesis_hash(BlockHash::from_byte_array(
                chain_net.chain_hash().to_bytes(),
            ));
        let mut bdk_database: BdkPersister = super::memory_store::MemoryStore::new();
        let bdk_wallet = match wallet_params.load_wallet(&mut bdk_database)? {
            Some(wallet) => wallet,
            None => BdkWallet::create(desc_colored.clone(), desc_vanilla.clone())
                .network(bdk_network)
                .create_wallet(&mut bdk_database)?,
        };
        let signers = build_signers(
            &bdk_wallet,
            &desc_colored,
            &desc_vanilla,
            wdata.bitcoin_network.into(),
        )?;

        // RGB setup
        let supported_schemas = wdata.supported_schemas;
        if supported_schemas.is_empty() {
            return Err(Error::NoSupportedSchemas);
        }
        if wdata.bitcoin_network == BitcoinNetwork::Mainnet
            && let Some(schema) = supported_schemas.iter().find(|s| !s.allowed_on_mainnet())
        {
            if !wdata.allow_reckless_schemas {
                return Err(Error::CannotUseSchemaOnMainnet {
                    asset_schema: *schema,
                });
            }
            let msg = format!("Reckless schema {schema} allowed on mainnet");
            warn!(logger, "{msg}");
            if cfg!(target_arch = "wasm32") {
                web_sys::console::warn_1(&msg.into());
            }
        }
        let mut runtime = load_rgb_runtime(wallet_dir.clone())?;
        let known_schemas = runtime.schemata()?;
        if known_schemas.len() < NUM_KNOWN_SCHEMAS {
            let known: HashSet<_> = known_schemas.iter().map(|s| s.id).collect();
            for schema in supported_schemas {
                if !known.contains(&SchemaId::from(schema)) {
                    schema.import_schema_definition(&mut runtime)?;
                }
            }
        }

        // RGB-LIB setup
        let database = crate::database::memory_db::InMemoryDb::new();

        let mut writer = [0u8; 8];
        getrandom::getrandom(&mut writer).map_err(|e| Error::Internal {
            details: format!("RNG unavailable: {e}"),
        })?;
        let idb_writer = u64::from_le_bytes(writer);

        info!(logger, "New wallet completed");
        Ok(Wallet {
            wallet_data,
            keys,
            logger,
            _logger_guard,
            watch_only,
            database: Arc::new(database),
            wallet_dir,
            bdk_wallet,
            signers,
            bdk_database,
            reuse_address_index: HashMap::new(),
            #[cfg(feature = "esplora")]
            online_data: None,
            #[cfg(feature = "esplora")]
            transfer_artifacts: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignments: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_valid_consignments: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignment_meta: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_signed_txs: HashMap::new(),
            received_mints: HashMap::new(),
            media: HashMap::new(),
            pending_mint_locks: HashMap::new(),
            #[cfg(feature = "esplora")]
            broadcast_txs: Vec::new(),
            idb_queue: Default::default(),
            idb_sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            idb_writer,
            idb_loaded: None,
            rgb_stock: {
                let stock = std::mem::take(&mut runtime.stock);
                std::rc::Rc::new(std::cell::RefCell::new(Some(stock)))
            },
            backup_store: None,
            backup_versions: std::cell::RefCell::new(HashMap::new()),
            last_vss_backup_error: std::cell::RefCell::new(None),
        })
    }

    pub(crate) fn bitcoin_network(&self) -> BitcoinNetwork {
        self.wallet_data.bitcoin_network
    }

    pub(crate) fn chain_net(&self) -> ChainNet {
        self.bitcoin_network().into()
    }

    pub(crate) fn rgb_runtime(&self) -> Result<RgbRuntime, Error> {
        let stock = self
            .rgb_stock
            .try_borrow_mut()
            .map_err(|_| Error::Internal {
                details: s!("RGB runtime already in use"),
            })?
            .take()
            .unwrap_or_default();
        Ok(RgbRuntime::with_shared_stock(
            stock,
            self.wallet_dir.clone(),
            self.rgb_stock.clone(),
        ))
    }

    /// Return the data that defines the wallet.
    pub fn get_wallet_data(&self) -> WalletData {
        self.wallet_data.clone()
    }

    /// Return the bitcoin keys of the wallet.
    pub fn get_keys(&self) -> SinglesigKeys {
        self.keys.clone()
    }

    /// Return the descriptors of the wallet.
    pub fn get_descriptors(&self) -> WalletDescriptors {
        self.keys
            .build_descriptors(self.bitcoin_network())
            .expect("already succeeded at wallet creation")
            .0
    }

    /// Return the wallet directory.
    pub fn get_wallet_dir(&self) -> PathBuf {
        self.wallet_dir.clone()
    }

    /// Return the media directory.
    pub fn get_media_dir(&self) -> PathBuf {
        self.wallet_dir.join(MEDIA_DIR)
    }

    pub(crate) fn get_transfers_dir(&self) -> PathBuf {
        self.wallet_dir.join(TRANSFERS_DIR)
    }

    pub(crate) fn get_transfer_dir(&self, transfer_id: &str) -> PathBuf {
        self.get_transfers_dir().join(transfer_id)
    }

    pub(crate) fn get_asset_transfer_dir<P: AsRef<Path>>(
        &self,
        transfer_dir: P,
        asset_id: &str,
    ) -> PathBuf {
        let asset_id_no_prefix = asset_id.replace(ASSET_ID_PREFIX, "");
        transfer_dir.as_ref().join(&asset_id_no_prefix)
    }

    pub(crate) fn max_allocations_per_utxo(&self) -> u32 {
        self.wallet_data.max_allocations_per_utxo
    }

    pub(crate) fn supports_schema(&self, asset_schema: &AssetSchema) -> bool {
        self.wallet_data.supported_schemas.contains(asset_schema)
    }

    pub(crate) fn check_schema_support(&self, asset_schema: &AssetSchema) -> Result<(), Error> {
        if !self.supports_schema(asset_schema) {
            return Err(Error::UnsupportedSchema {
                asset_schema: *asset_schema,
            });
        }
        Ok(())
    }

    pub(crate) fn check_linked_contract_known(
        &self,
        contract_id: &ContractId,
    ) -> Result<DbAsset, Error> {
        self.database
            .get_asset(contract_id.to_string())?
            .ok_or_else(|| Error::UnknownLinkedContract {
                contract_id: contract_id.to_string(),
            })
    }

    /// Check that a new asset can link from this contract.
    fn check_linked_from_contract(&self, linked_from_contract: &ContractId) -> Result<(), Error> {
        let linked_from = self.check_linked_contract_known(linked_from_contract)?;
        if !linked_from.schema.supports_link() {
            return Err(Error::InvalidLinkedContract {
                contract_id: linked_from.id,
                details: format!("schema {} doesn't support links", linked_from.schema),
            });
        }
        if let Some(linked_to_contract) = linked_from.linked_to_contract {
            return Err(Error::InvalidLinkedContract {
                contract_id: linked_from.id,
                details: format!("it's already linked to contract {linked_to_contract}"),
            });
        }
        Ok(())
    }

    pub(crate) fn check_transport_endpoints(
        &self,
        transport_endpoints: &[String],
    ) -> Result<(), Error> {
        if transport_endpoints.is_empty() {
            return Err(Error::InvalidTransportEndpoints {
                details: s!("must provide at least a transport endpoint"),
            });
        }
        if transport_endpoints.len() > MAX_TRANSPORT_ENDPOINTS {
            return Err(Error::InvalidTransportEndpoints {
                details: format!(
                    "library supports at max {MAX_TRANSPORT_ENDPOINTS} transport endpoints"
                ),
            });
        }

        Ok(())
    }

    pub(crate) fn filter_unspents(
        &self,
        keychain: KeychainKind,
    ) -> impl Iterator<Item = LocalOutput> + '_ {
        self.bdk_wallet
            .list_unspent()
            .filter(move |u| u.keychain == keychain)
    }

    pub(crate) fn internal_unspents(&self) -> impl Iterator<Item = LocalOutput> + '_ {
        self.filter_unspents(KeychainKind::Internal)
    }

    pub(crate) fn get_uncolorable_btc_sum(&self) -> Result<u64, Error> {
        Ok(self
            .internal_unspents()
            .map(|u| u.txout.value.to_sat())
            .sum())
    }

    /// The maximum number of allocations a UTXO can hold and still be spent.
    /// It counts all assets, so it is an upper bound for each asset.
    pub(crate) fn max_utxo_allocations(&self) -> u32 {
        Ord::min(
            self.max_allocations_per_utxo(),
            u32::try_from(max_utxo_asset_allocations()).unwrap_or(u32::MAX),
        )
    }

    pub(crate) fn get_available_allocations<T>(
        &self,
        unspents: T,
        exclude_utxos: &[Outpoint],
        max_allocations: Option<u32>,
    ) -> Result<Vec<LocalUnspent>, Error>
    where
        T: Into<Vec<LocalUnspent>>,
    {
        let mut mut_unspents = unspents.into();
        mut_unspents
            .iter_mut()
            .for_each(|u| u.rgb_allocations.retain(|a| !a.status.failed()));
        let max_allocs = max_allocations.unwrap_or_else(|| {
            self.max_utxo_allocations()
                .checked_sub(1)
                .expect("max allocations per UTXO must be greater than 0")
        });
        let reserved = self.get_reserved_outpoints()?;
        Ok(mut_unspents
            .iter()
            .filter(|u| u.utxo.exists)
            .filter(|u| !u.utxo.pending_witness)
            .filter(|u| !exclude_utxos.contains(&u.utxo.outpoint()))
            .filter(|u| !reserved.contains(&u.utxo.outpoint()))
            .filter(|u| {
                (u.rgb_allocations.len() as u32) + u.pending_blinded <= max_allocs
                    && !u.rgb_allocations.iter().any(|a| {
                        !a.incoming && (a.status.initiated() || a.status.waiting_counterparty())
                    })
            })
            .cloned()
            .collect())
    }

    pub(crate) fn detect_btc_unspendable_err(&self) -> Result<Error, Error> {
        let available = self.get_uncolorable_btc_sum()?;
        Ok(if available < MIN_BTC_REQUIRED {
            Error::InsufficientBitcoins {
                needed: MIN_BTC_REQUIRED,
                available,
            }
        } else {
            Error::InsufficientAllocationSlots
        })
    }
}

impl Wallet {
    /// Return the inputs of the transfers that can still fail (Initiated, waiting for the
    /// counterparty or for the broadcast), and of the begun operations with no saved transfer,
    /// until they expire.
    pub(crate) fn get_reserved_outpoints(&self) -> Result<Vec<Outpoint>, Error> {
        let now = now().unix_timestamp();
        let waiting: HashMap<String, bool> = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .filter_map(|bt| Some((bt.txid.clone()?, bt.is_fallible())))
            .collect();
        Ok(self
            .transfer_artifacts
            .iter()
            .filter(|(txid, a)| match waiting.get(*txid) {
                Some(waiting) => *waiting,
                None => a.batch_info.is_some() && now < a.expiration,
            })
            .flat_map(|(_, a)| a.inputs.clone())
            .collect())
    }

    /// Return the inputs of the begun vanilla operations.
    pub(crate) fn get_reserved_vanilla_outpoints(&self) -> Result<Vec<OutPoint>, Error> {
        Ok(self
            .database
            .iter_wallet_transactions()?
            .into_iter()
            .filter(|w| w.status == TransferStatus::Initiated)
            .flat_map(|w| w.inputs)
            .map(OutPoint::from)
            .collect())
    }

    pub(crate) fn get_utxo(
        &self,
        exclude_utxos: &[Outpoint],
        unspents: Option<&[LocalUnspent]>,
        pending_operation: bool,
        max_allocations: Option<u32>,
    ) -> Result<DbTxo, Error> {
        let rgb_allocations = if unspents.is_none() {
            let unspent_txos = self.database.get_unspent_txos(vec![])?;
            Some(
                self.database
                    .get_rgb_allocations(unspent_txos, None, None, None, None)?,
            )
        } else {
            None
        };
        let unspents: &[LocalUnspent] = match unspents {
            Some(u) => u,
            None => rgb_allocations.as_deref().unwrap(),
        };

        let mut allocatable =
            self.get_available_allocations(unspents, exclude_utxos, max_allocations)?;
        allocatable.sort_by_key(|t| t.rgb_allocations.len() + t.pending_blinded as usize);
        match allocatable.first() {
            Some(mut selected) => {
                if allocatable.len() > 1 && !selected.rgb_allocations.is_empty() {
                    let filtered_allocatable: Vec<&LocalUnspent> = if pending_operation {
                        allocatable
                            .iter()
                            .filter(|t| t.rgb_allocations.iter().any(|a| a.future()))
                            .collect()
                    } else {
                        allocatable
                            .iter()
                            .filter(|t| t.rgb_allocations.iter().all(|a| !a.future()))
                            .collect()
                    };
                    if let Some(other) = filtered_allocatable.first() {
                        selected = other;
                    }
                }
                Ok(selected.clone().utxo)
            }
            None => Err(self.detect_btc_unspendable_err()?),
        }
    }

    pub(crate) fn save_transfer_transport_endpoint(
        &self,
        transfer_idx: i32,
        transport_endpoint: &LocalTransportEndpoint,
    ) -> Result<(), Error> {
        let transport_endpoint_idx = match self
            .database
            .get_transport_endpoint(transport_endpoint.endpoint.clone())?
        {
            Some(ce) => ce.idx,
            None => self
                .database
                .set_transport_endpoint(DbTransportEndpointActMod {
                    transport_type: ActiveValue::Set(transport_endpoint.transport_type),
                    endpoint: ActiveValue::Set(transport_endpoint.endpoint.clone()),
                    ..Default::default()
                })?,
        };

        self.database
            .set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
                transfer_idx: ActiveValue::Set(transfer_idx),
                transport_endpoint_idx: ActiveValue::Set(transport_endpoint_idx),
                used: ActiveValue::Set(transport_endpoint.used),
                ..Default::default()
            })?;

        Ok(())
    }

    /// Persist the sender's own proxy endpoint for a transfer (the target for the recipient's ACK
    /// and the signed-TX release), tagged with the [`TransportEndpointKind::SenderProxy`] role.
    #[cfg(feature = "esplora")]
    pub(crate) fn save_sender_transport_endpoint(
        &self,
        transfer_idx: i32,
        endpoint: &str,
    ) -> Result<(), Error> {
        let transport_endpoint_idx =
            match self.database.get_transport_endpoint(endpoint.to_string())? {
                Some(ce) => ce.idx,
                None => self
                    .database
                    .set_transport_endpoint(DbTransportEndpointActMod {
                        transport_type: ActiveValue::Set(TransportType::JsonRpc),
                        endpoint: ActiveValue::Set(endpoint.to_string()),
                        ..Default::default()
                    })?,
            };
        // `used` is false, so the row never shows up as a used endpoint
        self.database
            .set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
                transfer_idx: ActiveValue::Set(transfer_idx),
                transport_endpoint_idx: ActiveValue::Set(transport_endpoint_idx),
                used: ActiveValue::Set(false),
                kind: ActiveValue::Set(TransportEndpointKind::SenderProxy),
                ..Default::default()
            })?;
        Ok(())
    }

    /// Convert a single `RgbTransport`-format endpoint (e.g. `rpc://host`) to its HTTP(S) URL.
    #[cfg(feature = "esplora")]
    pub(crate) fn convert_transport_endpoint_url(&self, endpoint: &str) -> Result<String, Error> {
        let rgb_transport = RgbTransport::from_str(endpoint)?;
        match &rgb_transport {
            RgbTransport::JsonRpc { .. } => Ok(TransportEndpoint::try_from(rgb_transport)
                .map_err(|e| Error::InvalidTransportEndpoint {
                    details: e.to_string(),
                })?
                .endpoint),
            _ => Err(Error::UnsupportedTransportType),
        }
    }

    pub(crate) fn check_details(&self, details: String) -> Result<Details, Error> {
        if details.is_empty() {
            return Err(Error::InvalidDetails {
                details: s!("ident must contain at least one character"),
            });
        }
        Details::from_str(&details).map_err(|e| Error::InvalidDetails {
            details: e.to_string(),
        })
    }

    fn _check_contract_address(&self, contract_address: String) -> Result<EvmAddress, Error> {
        if !contract_address.starts_with("0x") {
            return Err(Error::InvalidEvmContractAddress {
                details: s!("address must start with 0x"),
            });
        }
        EvmAddress::from_str(&contract_address).map_err(|e| Error::InvalidEvmContractAddress {
            details: e.to_string(),
        })
    }

    fn _check_name(&self, name: String) -> Result<Name, Error> {
        Name::try_from(name).map_err(|e| Error::InvalidName {
            details: e.to_string(),
        })
    }

    fn _check_precision(&self, precision: u8) -> Result<Precision, Error> {
        Precision::try_from(precision).map_err(|_| Error::InvalidPrecision {
            details: s!("precision is too high"),
        })
    }

    pub(crate) fn check_burn_reason(&self, burn_reason: String) -> Result<BurnReason, Error> {
        let bytes = hex::decode(burn_reason).map_err(|e| Error::InvalidBurnReason {
            details: format!("not a hex string: {e}"),
        })?;
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|bytes: Vec<u8>| Error::InvalidBurnReason {
                    details: format!("expected 32 bytes, got {}", bytes.len()),
                })?;
        Ok(BurnReason::from(Bytes32::from(bytes)))
    }

    fn _check_reject_list_url(&self, opid_reject_url: String) -> Result<RejectListUrl, Error> {
        RejectListUrl::try_from(opid_reject_url).map_err(|e| Error::InvalidRejectListUrl {
            details: e.to_string(),
        })
    }

    fn _check_ticker(&self, ticker: String) -> Result<Ticker, Error> {
        if ticker.to_ascii_uppercase() != *ticker {
            return Err(Error::InvalidTicker {
                details: s!("ticker needs to be all uppercase"),
            });
        }
        Ticker::try_from(ticker).map_err(|e| Error::InvalidTicker {
            details: e.to_string(),
        })
    }

    fn _check_unicode_ticker(&self, ticker: String) -> Result<UnicodeTicker, Error> {
        UnicodeTicker::try_from(ticker).map_err(|e| Error::InvalidTicker {
            details: e.to_string(),
        })
    }

    fn _get_total_issue_amount(&self, amounts: &[u64], allow_empty: bool) -> Result<u64, Error> {
        if amounts.is_empty() && !allow_empty {
            return Err(Error::NoIssuanceAmounts);
        }
        amounts.iter().try_fold(0u64, |acc, x| {
            if *x == 0 {
                return Err(Error::InvalidAmountZero);
            }
            acc.checked_add(*x).ok_or(Error::TooHighIssuanceAmounts)
        })
    }

    pub(crate) fn get_total_inflation_amount(
        &self,
        inflation_amounts: &[u64],
        issued_supply: u64,
    ) -> Result<u64, Error> {
        let total_inflation = inflation_amounts.iter().try_fold(0u64, |acc, x| {
            if *x == 0 {
                return Err(Error::InvalidAmountZero);
            }
            acc.checked_add(*x).ok_or(Error::TooHighInflationAmounts)
        })?;

        if inflation_amounts.is_empty() {
            return Ok(0);
        }

        issued_supply
            .checked_add(total_inflation)
            .ok_or(Error::TooHighInflationAmounts)?;

        Ok(total_inflation)
    }

    pub(crate) fn new_asset_terms(
        &self,
        text: RicardianContract,
        media: Option<Attachment>,
    ) -> ContractTerms {
        #[cfg(test)]
        let media = media.or_else(|| MOCK_ISSUE_MEDIA.take());
        ContractTerms { text, media }
    }

    pub(crate) fn get_blind_seal(&self, outpoint: impl Into<OutPoint>) -> BlindSeal<RgbTxid> {
        let outpoint = outpoint.into();
        BlindSeal::new_random(outpoint.txid, outpoint.vout)
    }

    pub(crate) fn get_builder_seal(
        &self,
        outpoint: impl Into<OutPoint>,
    ) -> BuilderSeal<BlindSeal<RgbTxid>> {
        BuilderSeal::from(self.get_blind_seal(outpoint))
    }

    fn _get_issue_consignment_path(&self, asset_id: &str) -> PathBuf {
        self.wallet_dir
            .join(ASSETS_DIR)
            .join(format!("{asset_id}.{RGB_BIN_EXTENSION}"))
    }

    pub(crate) fn get_or_insert_media(&self, digest: String, mime: String) -> Result<i32, Error> {
        Ok(match self.database.get_media_by_digest(digest.clone())? {
            Some(media) => media.idx,
            None => self.database.set_media(DbMediaActMod {
                digest: ActiveValue::Set(digest),
                mime: ActiveValue::Set(mime),
                ..Default::default()
            })?,
        })
    }

    pub(crate) fn extract_attachments(
        &self,
        runtime: &RgbRuntime,
        valid_contract: &ValidContract,
        asset_schema: AssetSchema,
    ) -> Result<Vec<Attachment>, Error> {
        let contract_data = runtime.consignment_data(valid_contract)?;
        let media = match asset_schema {
            AssetSchema::Nia => NiaWrapper::with(contract_data).contract_terms().media,
            AssetSchema::Ifa => IfaWrapper::with(contract_data).contract_terms().media,
            AssetSchema::Bfa => BfaWrapper::with(contract_data).contract_terms().media,
        };
        Ok(media.into_iter().collect())
    }

    pub(crate) fn get_asset_medias(&self, media_idx: Option<i32>) -> Result<HashSet<Media>, Error> {
        let mut asset_medias = HashSet::new();
        if let Some(media_idx) = media_idx {
            let db_media = self
                .database
                .get_media(media_idx)?
                .expect("asset media row exists");
            asset_medias.insert(Media::from_db_media(&db_media, self.get_media_dir()));
        }
        Ok(asset_medias)
    }

    pub(crate) fn add_asset_to_db(
        &self,
        asset_id: String,
        schema: &AssetSchema,
        added_at: Option<i64>,
        timestamp: i64,
        asset_data: LocalAssetData,
    ) -> Result<DbAsset, Error> {
        let added_at = added_at.unwrap_or_else(|| now().unix_timestamp());
        let mut db_asset = DbAssetActMod {
            idx: ActiveValue::NotSet,
            media_idx: ActiveValue::Set(asset_data.media_idx),
            id: ActiveValue::Set(asset_id),
            schema: ActiveValue::Set(*schema),
            added_at: ActiveValue::Set(added_at),
            details: ActiveValue::Set(asset_data.details),
            initial_supply: ActiveValue::Set(asset_data.initial_supply.to_string()),
            max_supply: ActiveValue::Set(asset_data.max_supply.map(|s| s.to_string())),
            known_circulating_supply: ActiveValue::Set(
                asset_data.known_circulating_supply.map(|s| s.to_string()),
            ),
            name: ActiveValue::Set(asset_data.name),
            precision: ActiveValue::Set(asset_data.precision),
            ticker: ActiveValue::Set(asset_data.ticker),
            timestamp: ActiveValue::Set(timestamp),
            bridge_location: ActiveValue::Set(asset_data.bridge_location),
            reject_list: ActiveValue::Set(asset_data.reject_list),
            linked_from_contract: ActiveValue::Set(asset_data.linked_from_contract),
            linked_to_contract: ActiveValue::Set(None),
        };
        let idx = self.database.set_asset(db_asset.clone())?;
        db_asset.idx = ActiveValue::Set(idx);
        Ok(db_asset.try_into_model().unwrap())
    }

    fn _get_btc_balance(&self, keychain: KeychainKind) -> Result<Balance, Error> {
        let chain = self.bdk_wallet.local_chain();
        let chain_tip = self.bdk_wallet.latest_checkpoint().block_id();
        let outpoints = self.filter_unspents(keychain).map(|lo| ((), lo.outpoint));
        let balance = self.bdk_wallet.as_ref().balance(
            chain,
            chain_tip,
            CanonicalizationParams::default(),
            outpoints,
            |_, _| false,
        );

        let future = balance.total();
        Ok(Balance {
            settled: balance.confirmed.to_sat(),
            future: future.to_sat(),
            spendable: future.to_sat() - balance.immature.to_sat(),
        })
    }

    /// Issue a new RGB NIA asset with the provided `ticker`, `name`, `precision` and `amounts`,
    /// then return it.
    ///
    /// At least 1 amount needs to be provided and the sum of all amounts cannot exceed the maximum
    /// `u64` value.
    ///
    /// If `amounts` contains more than 1 element, each one will be issued as a separate allocation
    /// for the same asset (on a separate UTXO that needs to be already available).
    pub fn issue_asset_nia(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
    ) -> Result<AssetNIA, Error> {
        let cp = self.shared_checkpoint("issue_asset_nia");
        let res = self._issue_asset_nia(ticker, name, precision, amounts);
        self.commit_shared(cp, res)
    }

    fn _issue_asset_nia(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
    ) -> Result<AssetNIA, Error> {
        info!(
            self.logger,
            "Issuing NIA asset with ticker '{}' name '{}' precision '{}' amounts '{:?}'...",
            ticker,
            name,
            precision,
            amounts
        );

        let asset_schema = &AssetSchema::Nia;

        self.check_schema_support(asset_schema)?;

        let settled = self._get_total_issue_amount(&amounts, false)?;

        let db_data = self.database.get_db_data(false)?;

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(db_data.txos.clone())?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });

        let created_at = now().unix_timestamp();
        let text = RicardianContract::default();
        let terms = self.new_asset_terms(text, None);
        let details = None;
        let spec = AssetSpec {
            ticker: self._check_ticker(ticker.clone())?,
            name: self._check_name(name.clone())?,
            details,
            precision: self._check_precision(precision)?,
        };

        let mut runtime = self.rgb_runtime()?;
        let mut builder = ContractBuilder::with(
            Identity::default(),
            NonInflatableAsset::schema_rules(),
            self.chain_net(),
        )
        .add_global_state("spec", spec.clone())
        .expect("invalid spec")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(settled))
        .expect("invalid issuedSupply");

        let mut issue_utxos: HashMap<DbTxo, u64> = HashMap::new();
        let mut exclude_outpoints = vec![];
        for amount in &amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, None)?;
            exclude_outpoints.push(utxo.outpoint());
            issue_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(RGB_STATE_ASSET_OWNER, self.get_builder_seal(utxo), *amount)
                .expect("invalid global state data");
        }
        debug!(self.logger, "Issuing on UTXOs: {issue_utxos:?}");

        let validated_contract = builder.issue_contract().expect("failure issuing contract");
        let asset_id = validated_contract.contract_id().to_string();
        runtime
            .import_contract(validated_contract.clone(), &DumbResolver)
            .expect("failure importing issued contract");
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let asset_data = LocalAssetData {
            name,
            precision,
            ticker: Some(ticker),
            details: spec.details().map(|d| d.to_string()),
            media_idx: None,
            initial_supply: settled,
            max_supply: None,
            known_circulating_supply: None,
            bridge_location: None,
            reject_list: None,
            linked_from_contract: None,
        };
        let asset = self.add_asset_to_db(
            asset_id.clone(),
            asset_schema,
            Some(created_at),
            validated_contract.genesis.timestamp,
            asset_data,
        )?;
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::Settled),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(0),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id)),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(true),
            ..Default::default()
        };
        self.database.set_transfer(transfer)?;
        for (utxo, amount) in issue_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::Fungible(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        let asset = AssetNIA::get_asset_details(self, &asset, None, None, None, None, None, None)?;

        self.update_backup_info(false)?;

        info!(self.logger, "Issue asset NIA completed");
        Ok(asset)
    }

    /// Issue a new RGB IFA asset with the provided `ticker`, `name`, `precision`, `amounts`,
    /// and `inflation_amounts`, then return it.
    ///
    /// At least 1 amount needs to be provided and the sum of all amounts cannot exceed the maximum
    /// `u64` value.
    ///
    /// If `amounts` contains more than 1 element, each one will be issued as a separate allocation
    /// for the same asset (on a separate UTXO that needs to be already available).
    ///
    /// The `inflation_amounts` can be empty. If provided the sum of its elements plus the sum of
    /// `amounts` cannot exceed the maximum `u64` value.
    ///
    /// If `link_right` is true, the asset is issued with its link right, on a separate UTXO, which
    /// can be spent once to link the asset to another contract (see
    #[cfg_attr(feature = "esplora", doc = "[`link`](Wallet::link)).")]
    #[cfg_attr(not(feature = "esplora"), doc = "`link`).")]
    ///
    /// The optional `linked_from_contract` is the ID of the contract this is linked from. The wallet
    /// needs to know it, to check it can link to the contract being issued: its schema needs to
    /// support links and it needs to be unlinked. Set `skip_contract_checks` to skip these checks
    /// and link from a contract the wallet cannot check, e.g. one it doesn't know.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_ifa(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
        inflation_amounts: Vec<u64>,
        reject_list_url: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
    ) -> Result<AssetIFA, Error> {
        let cp = self.shared_checkpoint("issue_asset_ifa");
        let res = self._issue_asset_ifa(
            ticker,
            name,
            precision,
            amounts,
            inflation_amounts,
            reject_list_url,
            link_right,
            linked_from_contract,
            skip_contract_checks,
        );
        self.commit_shared(cp, res)
    }

    #[allow(clippy::too_many_arguments)]
    fn _issue_asset_ifa(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        amounts: Vec<u64>,
        inflation_amounts: Vec<u64>,
        reject_list_url: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
    ) -> Result<AssetIFA, Error> {
        info!(
            self.logger,
            "Issuing IFA asset with ticker '{}' name '{}' precision '{}' amounts '{:?}' inflation amounts {:?}...",
            ticker,
            name,
            precision,
            amounts,
            inflation_amounts,
        );

        let asset_schema = &AssetSchema::Ifa;

        self.check_schema_support(asset_schema)?;

        let linked_from_contract = linked_from_contract
            .map(|contract_id| {
                ContractId::from_str(&contract_id)
                    .map_err(|_| Error::InvalidContractId { contract_id })
            })
            .transpose()?;
        if let Some(linked_from_contract) = &linked_from_contract
            && !skip_contract_checks
        {
            self.check_linked_from_contract(linked_from_contract)?;
        }

        let settled = self._get_total_issue_amount(&amounts, true)?;
        let inflation_amt = self.get_total_inflation_amount(&inflation_amounts, settled)?;
        if settled == 0 && inflation_amt == 0 {
            return Err(Error::NoIssuanceAmounts);
        }
        let max_supply = settled + inflation_amt;

        let db_data = self.database.get_db_data(false)?;

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(db_data.txos.clone())?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });

        let created_at = now().unix_timestamp();
        let text = RicardianContract::default();
        let terms = self.new_asset_terms(text, None);
        let details = None;
        let spec = AssetSpec {
            ticker: self._check_ticker(ticker.clone())?,
            name: self._check_name(name.clone())?,
            details,
            precision: self._check_precision(precision)?,
        };

        let mut runtime = self.rgb_runtime()?;
        let mut builder = ContractBuilder::with(
            Identity::default(),
            InflatableFungibleAsset::schema_rules(),
            self.chain_net(),
        )
        .add_global_state("spec", spec.clone())
        .expect("invalid spec")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(settled))
        .expect("invalid issuedSupply")
        .add_global_state("maxSupply", Amount::from(max_supply))
        .expect("invalid maxSupply");
        if let Some(reject_list_url) = &reject_list_url {
            builder = builder
                .add_global_state(
                    RGB_GLOBAL_REJECT_LIST_URL,
                    self._check_reject_list_url(reject_list_url.clone())?,
                )
                .expect("invalid rejectListUrl");
        }

        let mut issue_utxos: HashMap<DbTxo, u64> = HashMap::new();
        let mut exclude_outpoints: Vec<Outpoint> = vec![];
        for amount in &amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, None)?;
            exclude_outpoints.push(utxo.outpoint());
            issue_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(RGB_STATE_ASSET_OWNER, self.get_builder_seal(utxo), *amount)
                .expect("invalid global state data");
        }
        debug!(self.logger, "Issuing on UTXOs: {issue_utxos:?}");

        let mut inflation_utxos: HashMap<DbTxo, u64> = HashMap::new();
        for amount in &inflation_amounts {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, Some(0))?;
            exclude_outpoints.push(utxo.outpoint());
            inflation_utxos.insert(utxo.clone(), *amount);

            builder = builder
                .add_fungible_state(
                    RGB_STATE_INFLATION_ALLOWANCE,
                    self.get_builder_seal(utxo),
                    *amount,
                )
                .expect("invalid global state data");
        }
        debug!(
            self.logger,
            "Assigning inflation rights: {inflation_utxos:?}"
        );
        let mut link_utxo = None;
        if link_right {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, Some(0))?;
            builder = builder
                .add_rights(RGB_STATE_LINK_RIGHT, self.get_builder_seal(utxo.clone()))
                .expect("invalid link right");
            link_utxo = Some(utxo);
        }
        if let Some(linked_from_contract) = &linked_from_contract {
            builder = builder
                .add_global_state(RGB_GLOBAL_LINKED_FROM_CONTRACT, *linked_from_contract)
                .expect("invalid global state data");
        }

        #[cfg(test)]
        if let Some(seal) = MOCK_IFA_LINK_RIGHT.take() {
            builder = builder
                .add_rights(RGB_STATE_LINK_RIGHT, seal)
                .expect("invalid link right");
        }

        let validated_contract = builder.issue_contract().expect("failure issuing contract");
        let asset_id = validated_contract.contract_id().to_string();
        runtime
            .import_contract(validated_contract.clone(), &DumbResolver)
            .expect("failure importing issued contract");
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let asset_data = LocalAssetData {
            name,
            precision,
            ticker: Some(ticker),
            details: spec.details().map(|d| d.to_string()),
            media_idx: None,
            initial_supply: settled,
            max_supply: Some(max_supply),
            known_circulating_supply: Some(settled),
            bridge_location: None,
            reject_list: reject_list_url.map(RgbRejectListInfo::Url),
            linked_from_contract: linked_from_contract.map(|c| c.to_string()),
        };
        let asset = self.add_asset_to_db(
            asset_id.clone(),
            asset_schema,
            Some(created_at),
            validated_contract.genesis.timestamp,
            asset_data,
        )?;
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::Settled),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(0),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id)),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(true),
            ..Default::default()
        };
        self.database.set_transfer(transfer)?;
        for (utxo, amount) in issue_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::Fungible(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }
        for (utxo, amount) in inflation_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::InflationRight(amount)),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }
        if let Some(utxo) = link_utxo {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::LinkRight),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        let asset = AssetIFA::get_asset_details(self, &asset, None, None, None, None, None, None)?;

        self.update_backup_info(false)?;

        info!(self.logger, "Issue asset IFA completed");
        Ok(asset)
    }

    /// Issue a new RGB BFA asset with the provided `ticker`, `name`, `precision` and
    /// `mint_rights`, then return it.
    ///
    /// The asset is bridged from the bridge contract at `contract_address` on the EVM chain with
    /// the given `chain_id`. The optional `reject_list_address` is a reject list contract on the
    /// same chain.
    ///
    /// A BFA genesis issues no units. Each mint right goes on its own UTXO.
    ///
    /// If `link_right` is true, the asset is issued with its link right, on a separate UTXO, which
    /// can be spent once to link the asset to another contract (see
    #[cfg_attr(feature = "esplora", doc = "[`link`](Wallet::link)).")]
    #[cfg_attr(not(feature = "esplora"), doc = "`link`).")]
    ///
    /// The optional `linked_from_contract` is the ID of the contract this is linked from. The wallet
    /// needs to know it, to check it can link to the contract being issued: its schema needs to
    /// support links and it needs to be unlinked. Set `skip_contract_checks` to skip these checks
    /// and link from a contract the wallet cannot check, e.g. one it doesn't know.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_bfa(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        mint_rights: u8,
        chain_id: u64,
        contract_address: String,
        reject_list_address: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
    ) -> Result<AssetBFA, Error> {
        let cp = self.shared_checkpoint("issue_asset_bfa");
        let res = self._issue_asset_bfa(
            ticker,
            name,
            precision,
            mint_rights,
            chain_id,
            contract_address,
            reject_list_address,
            link_right,
            linked_from_contract,
            skip_contract_checks,
        );
        self.commit_shared(cp, res)
    }

    #[allow(clippy::too_many_arguments)]
    fn _issue_asset_bfa(
        &self,
        ticker: String,
        name: String,
        precision: u8,
        mint_rights: u8,
        chain_id: u64,
        contract_address: String,
        reject_list_address: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
    ) -> Result<AssetBFA, Error> {
        info!(
            self.logger,
            "Issuing BFA asset with ticker '{}' name '{}' precision '{}' mint rights '{}'...",
            ticker,
            name,
            precision,
            mint_rights,
        );

        let asset_schema = &AssetSchema::Bfa;

        self.check_schema_support(asset_schema)?;

        let linked_from_contract = linked_from_contract
            .map(|contract_id| {
                ContractId::from_str(&contract_id)
                    .map_err(|_| Error::InvalidContractId { contract_id })
            })
            .transpose()?;
        if let Some(linked_from_contract) = &linked_from_contract
            && !skip_contract_checks
        {
            self.check_linked_from_contract(linked_from_contract)?;
        }

        if mint_rights == 0 {
            return Err(Error::NoMintRights);
        }

        let db_data = self.database.get_db_data(false)?;

        let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
            self.database.get_unspent_txos(db_data.txos.clone())?,
            None,
            None,
            None,
            None,
        )?;
        unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| !a.incoming && a.status.waiting_counterparty()))
        });

        let created_at = now().unix_timestamp();
        let text = RicardianContract::default();
        let terms = self.new_asset_terms(text, None);
        let bfa_ticker = self._check_unicode_ticker(ticker.clone())?;
        let bfa_name = self._check_name(name.clone())?;
        let bfa_precision = self._check_precision(precision)?;

        let bridge_location = BridgeLocation::Evm(EvmContract {
            chain_id,
            address: self._check_contract_address(contract_address)?,
        });

        let mut builder = ContractBuilder::with(
            Identity::default(),
            BridgedFungibleAsset::schema_rules(),
            self.chain_net(),
        )
        .add_global_state("ticker", bfa_ticker)
        .expect("invalid ticker")
        .add_global_state("name", bfa_name)
        .expect("invalid name")
        .add_global_state("precision", bfa_precision)
        .expect("invalid precision")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state(RGB_GLOBAL_BRIDGE_LOCATION, bridge_location.clone())
        .expect("invalid bridgeLocation");
        let reject_list = reject_list_address
            .map(|address| {
                Ok::<_, Error>(RejectListLocation::Evm(EvmContract {
                    chain_id,
                    address: self._check_contract_address(address)?,
                }))
            })
            .transpose()?;
        if let Some(reject_list) = reject_list.clone() {
            builder = builder
                .add_global_state(RGB_GLOBAL_REJECT_LIST, reject_list)
                .expect("invalid rejectList");
        }

        let mut issue_utxos: Vec<DbTxo> = vec![];
        let mut exclude_outpoints: Vec<Outpoint> = vec![];
        for _ in 0..mint_rights {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, Some(0))?;
            exclude_outpoints.push(utxo.outpoint());
            issue_utxos.push(utxo.clone());

            builder = builder
                .add_rights(RGB_STATE_MINT_RIGHT, self.get_builder_seal(utxo))
                .expect("invalid mintRight");
        }
        debug!(self.logger, "Assigning mint rights: {issue_utxos:?}");
        let mut link_utxo = None;
        if link_right {
            let utxo = self.get_utxo(&exclude_outpoints, Some(&unspents), false, Some(0))?;
            builder = builder
                .add_rights(RGB_STATE_LINK_RIGHT, self.get_builder_seal(utxo.clone()))
                .expect("invalid link right");
            link_utxo = Some(utxo);
        }
        if let Some(linked_from_contract) = &linked_from_contract {
            builder = builder
                .add_global_state(RGB_GLOBAL_LINKED_FROM_CONTRACT, *linked_from_contract)
                .expect("invalid global state data");
        }

        let mut runtime = self.rgb_runtime()?;
        let validated_contract = builder.issue_contract().expect("failure issuing contract");
        let asset_id = validated_contract.contract_id().to_string();
        runtime
            .import_contract(validated_contract.clone(), &DumbResolver)
            .expect("failure importing issued contract");
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let asset_data = LocalAssetData {
            name,
            precision,
            ticker: Some(ticker),
            details: None,
            media_idx: None,
            initial_supply: 0,
            max_supply: None,
            known_circulating_supply: None,
            bridge_location: Some(bridge_location.into()),
            reject_list: reject_list.map(RgbRejectListInfo::from),
            linked_from_contract: linked_from_contract.map(|c| c.to_string()),
        };
        let asset = self.add_asset_to_db(
            asset_id.clone(),
            asset_schema,
            Some(created_at),
            validated_contract.genesis.timestamp,
            asset_data,
        )?;
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::Settled),
            expiration: ActiveValue::Set(None),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(0),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(Some(asset_id)),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        let transfer = DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(true),
            ..Default::default()
        };
        self.database.set_transfer(transfer)?;
        for utxo in issue_utxos {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::MintRight),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }
        if let Some(utxo) = link_utxo {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo.idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                r#type: ActiveValue::Set(ColoringType::Issue),
                assignment: ActiveValue::Set(Assignment::LinkRight),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        let asset = AssetBFA::get_asset_details(self, &asset, None, None, None, None, None, None)?;

        self.update_backup_info(false)?;

        info!(self.logger, "Issue asset BFA completed");
        Ok(asset)
    }

    /// Finalize a PSBT, optionally providing BDK sign options to tweak the behavior of the
    /// finalizer.
    pub fn finalize_psbt(
        &self,
        signed_psbt: String,
        sign_options: Option<SignOptions>,
    ) -> Result<String, Error> {
        info!(self.logger, "Finalizing PSBT...");
        let mut psbt = Psbt::from_str(&signed_psbt)?;
        let sign_options = sign_options.unwrap_or_default();
        if !self
            .bdk_wallet
            .finalize_psbt(&mut psbt, sign_options)
            .map_err(InternalError::from)?
        {
            return Err(Error::CannotFinalizePsbt);
        }
        info!(self.logger, "Finalize PSBT completed");
        Ok(psbt.to_string())
    }

    fn _sign_psbt(&self, psbt: &mut Psbt, sign_options: Option<SignOptions>) -> Result<(), Error> {
        let sign_options = sign_options.unwrap_or_default();
        let [colored, vanilla] = &self.signers;
        self.bdk_wallet
            .sign_with_signers(psbt, &[colored, vanilla], sign_options)
            .map_err(InternalError::from)?;
        Ok(())
    }

    /// Sign a PSBT, optionally providing BDK sign options.
    pub fn sign_psbt(
        &self,
        unsigned_psbt: String,
        sign_options: Option<SignOptions>,
    ) -> Result<String, Error> {
        info!(self.logger, "Signing PSBT...");
        let mut psbt = Psbt::from_str(&unsigned_psbt)?;
        self._sign_psbt(&mut psbt, sign_options)?;
        info!(self.logger, "Sign PSBT completed");
        Ok(psbt.to_string())
    }

    /// Inspect a PSBT to return its information.
    pub fn inspect_psbt(&self, psbt: String) -> Result<PsbtInspection, Error> {
        info!(self.logger, "Inspecting PSBT...");
        let psbt = Psbt::from_str(&psbt)?;

        let mut inputs = Vec::new();
        let mut total_input_sat = 0u64;
        for (psbt_input, input) in psbt.inputs.iter().zip(psbt.unsigned_tx.input.iter()) {
            let witness_utxo =
                psbt_input
                    .witness_utxo
                    .as_ref()
                    .ok_or_else(|| Error::PsbtInspection {
                        details: s!("cannot inspect non-segwit PSBT"),
                    })?;
            let amount_sat = witness_utxo.value.to_sat();
            let is_mine = self.bdk_wallet.is_mine(witness_utxo.script_pubkey.clone());
            total_input_sat = total_input_sat
                .checked_add(amount_sat)
                .expect("should never overflow");
            inputs.push(PsbtInputInfo {
                outpoint: Outpoint {
                    txid: input.previous_output.txid.to_string(),
                    vout: input.previous_output.vout,
                },
                amount_sat,
                is_mine,
            });
        }

        let mut outputs = Vec::new();
        let mut total_output_sat = 0u64;
        let network = BdkNetwork::from(self.bitcoin_network());
        for output in psbt.unsigned_tx.output.iter() {
            let amount_sat = output.value.to_sat();
            total_output_sat = total_output_sat
                .checked_add(amount_sat)
                .expect("should never overflow");
            let is_op_return = output.script_pubkey.is_op_return();
            let address = if !is_op_return {
                BdkAddress::from_script(&output.script_pubkey, network)
                    .ok()
                    .map(|addr| addr.to_string())
            } else {
                None
            };
            let is_mine = self.bdk_wallet.is_mine(output.script_pubkey.clone());
            outputs.push(PsbtOutputInfo {
                address,
                script_pubkey_hex: output.script_pubkey.to_hex_string(),
                amount_sat,
                is_op_return,
                is_mine,
            });
        }

        let mut signature_count = 0u16;
        for input in psbt.inputs.iter() {
            let partial_sigs_count = u16::try_from(input.partial_sigs.len())
                .map_err(|_| Error::TooManySignaturesInPsbt)?;
            let tap_script_sigs_count = u16::try_from(input.tap_script_sigs.len())
                .map_err(|_| Error::TooManySignaturesInPsbt)?;
            signature_count = signature_count
                .checked_add(partial_sigs_count)
                .and_then(|s| s.checked_add(tap_script_sigs_count))
                .ok_or(Error::TooManySignaturesInPsbt)?;
        }

        let inspection = PsbtInspection {
            txid: psbt.unsigned_tx.compute_txid().to_string(),
            inputs,
            outputs,
            total_input_sat,
            total_output_sat,
            fee_sat: total_input_sat.saturating_sub(total_output_sat),
            signature_count,
            size_vbytes: psbt.unsigned_tx.vsize() as u64,
        };
        info!(self.logger, "PSBT inspection completed");
        Ok(inspection)
    }

    /// Inspect a PSBT and fascia with wallet ownership and schema details.
    ///
    /// A fascia longer than `max_fascia_size` bytes gives [`Error::FileTooLarge`], and a fascia
    /// with more than `max_bundles` bundles gives [`Error::RgbInspection`].
    pub fn inspect_fascia(
        &self,
        psbt: String,
        fascia: &[u8],
        entropy: Option<u64>,
        max_fascia_size: u64,
        max_bundles: u32,
    ) -> Result<RgbInspection, Error> {
        info!(self.logger, "Inspecting fascia...");
        let psbt = Psbt::from_str(&psbt)?;
        let fascia = parse_fascia(fascia, max_fascia_size)?;

        // verify all PSBT inputs belong to this wallet
        for psbt_input in psbt.inputs.iter() {
            let script_pubkey = psbt_input
                .witness_utxo
                .as_ref()
                .ok_or_else(|| Error::RgbInspection {
                    details: s!("RGB-related PSBTs require segwit"),
                })?
                .script_pubkey
                .clone();
            if !self.bdk_wallet.is_mine(script_pubkey) {
                return Err(Error::RgbInspection {
                    details: s!("found a PSBT input that does not belong to this wallet"),
                });
            }
        }

        let runtime = self.rgb_runtime()?;
        let prev_outputs: Vec<OutPoint> = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|input| input.previous_output)
            .collect();
        let is_revealed_seal_ours = |txid: TxPtr, vout: u32| -> bool {
            if txid == TxPtr::WitnessTx {
                psbt.unsigned_tx
                    .output
                    .get(vout as usize)
                    .map(|output| self.bdk_wallet.is_mine(output.script_pubkey.clone()))
                    .unwrap_or(false)
            } else {
                let outpoint = Outpoint {
                    txid: txid.to_string(),
                    vout,
                };
                self.database.get_txo(&outpoint).unwrap_or(None).is_some()
            }
        };
        let inspection = fascia_inspection(
            &psbt,
            &fascia,
            entropy,
            max_bundles,
            |contract_id, bundle, witness_tx| {
                // collect the RGB inputs allocated in stock to the spent outputs
                let mut stock_inputs = HashMap::new();
                if let Ok(ass_map) =
                    runtime.contract_assignments_for(contract_id, prev_outputs.iter().copied())
                {
                    for (explicit_seal, opout_state_map) in ass_map {
                        let outpoint = explicit_seal.to_outpoint();
                        for (opout, state) in opout_state_map {
                            let assignment = Assignment::from_opout_and_state(opout, &state);
                            stock_inputs.insert(opout, (outpoint, assignment));
                        }
                    }
                }

                // the bundle must spend all the stock inputs, else they are lost
                let bundle_input_opouts: HashSet<_> = bundle
                    .known_transitions
                    .iter()
                    .flat_map(|KnownTransition { transition, .. }| transition.inputs())
                    .collect();
                let extra_in_stock: Vec<_> = stock_inputs
                    .keys()
                    .filter(|opout| !bundle_input_opouts.contains(opout))
                    .collect();
                if !extra_in_stock.is_empty() {
                    return Err(Error::RgbInspection {
                        details: format!(
                            "bundle for contract {contract_id}: stock contains {} input(s) not declared in bundle (malformed transfer?): {extra_in_stock:?}",
                            extra_in_stock.len(),
                        ),
                    });
                }

                let chain_net = runtime.genesis(contract_id)?.chain_net;
                let asset_schema = AssetSchema::get_from_contract_id(contract_id, &runtime)?;
                let is_ours = |seal: &BuilderSeal<GraphSeal>| {
                    Some(match seal {
                        BuilderSeal::Revealed(seal) => {
                            is_revealed_seal_ours(seal.txid, seal.vout.into_u32())
                        }
                        BuilderSeal::Concealed(seal) => runtime
                            .stock
                            .as_store()
                            .seal_of_secret(*seal)
                            .unwrap_or(None)
                            .is_some(),
                    })
                };
                bundle_info(
                    contract_id,
                    Some(asset_schema),
                    chain_net,
                    bundle,
                    witness_tx,
                    true,
                    |opout| stock_inputs.get(&opout).cloned(),
                    is_ours,
                    |_| None,
                )
            },
        )?;
        info!(self.logger, "Fascia inspection completed");
        Ok(inspection)
    }

    fn _delete_batch_transfer(
        &self,
        batch_transfer: &DbBatchTransfer,
        asset_transfers: &Vec<DbAssetTransfer>,
        colorings: &[DbColoring],
        txos: &[DbTxo],
    ) -> Result<(), Error> {
        let mut txos_to_delete = HashSet::new();
        for asset_transfer in asset_transfers {
            self.database.del_coloring(asset_transfer.idx)?;
            colorings
                .iter()
                .filter(|c| c.asset_transfer_idx == asset_transfer.idx)
                .for_each(|c| {
                    if let Some(txo) = txos.iter().find(|t| !t.exists && t.idx == c.txo_idx) {
                        txos_to_delete.insert(txo.idx);
                    }
                });
        }
        for txo in txos_to_delete {
            self.database.del_txo(txo)?;
        }
        Ok(self.database.del_batch_transfer(batch_transfer)?)
    }

    /// Delete eligible transfers from the database and return true if any transfer has been
    /// deleted.
    ///
    /// An optional `batch_transfer_idx` can be provided to operate on a single batch transfer.
    ///
    /// If a `batch_transfer_idx` is provided and `no_asset_only` is true, transfers with an
    /// associated asset ID will not be deleted and instead return an error.
    ///
    /// If no `batch_transfer_idx` is provided, all failed transfers will be deleted, and if
    /// `no_asset_only` is true transfers with an associated asset ID will be skipped.
    ///
    /// Eligible transfers are in status [`TransferStatus::Failed`]. Transfers of a live reusable
    /// invoice are not eligible, because the invoice needs them to receive more transfers.
    /// Delete-all skips them. A `batch_transfer_idx` of one returns an error.
    pub fn delete_transfers(
        &self,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
    ) -> Result<bool, Error> {
        let cp = self.shared_checkpoint("delete_transfers");
        let res = self._delete_transfers(batch_transfer_idx, no_asset_only);
        if matches!(res, Ok(false)) {
            return Ok(false);
        }
        self.commit_shared(cp, res)
    }

    fn _delete_transfers(
        &self,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
    ) -> Result<bool, Error> {
        info!(
            self.logger,
            "Deleting batch transfer with idx {:?}...", batch_transfer_idx
        );

        let db_data = self.database.get_db_data(false)?;
        let mut transfers_changed = false;

        // A live reusable invoice reaches its proxies and its UTXO or script through its transfers.
        // Refresh also deletes its failed transfers from the proxies. Keep them while it is live.
        let now = now().unix_timestamp();
        let live_invoices: HashSet<i32> = self
            .database
            .iter_invoices()?
            .into_iter()
            .filter(|i| i.live(now))
            .map(|i| i.idx)
            .collect();
        let live_invoice_asset_transfers: HashSet<i32> = db_data
            .transfers
            .iter()
            .filter(|t| t.invoice_idx.is_some_and(|i| live_invoices.contains(&i)))
            .map(|t| t.asset_transfer_idx)
            .collect();
        let of_live_invoice = |asset_transfers: &[DbAssetTransfer]| {
            asset_transfers
                .iter()
                .any(|at| live_invoice_asset_transfers.contains(&at.idx))
        };

        if let Some(batch_transfer_idx) = batch_transfer_idx {
            let batch_transfer = &self
                .database
                .get_batch_transfer_or_fail(batch_transfer_idx, &db_data.batch_transfers)?;

            if !batch_transfer.failed() {
                return Err(Error::CannotDeleteBatchTransfer);
            }

            let asset_transfers = batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;

            if of_live_invoice(&asset_transfers) {
                return Err(Error::CannotDeleteBatchTransfer);
            }

            if no_asset_only {
                let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                if connected_assets {
                    return Err(Error::CannotDeleteBatchTransfer);
                }
            }

            transfers_changed = true;
            self._delete_batch_transfer(
                batch_transfer,
                &asset_transfers,
                &db_data.colorings,
                &db_data.txos,
            )?
        } else {
            // delete all failed transfers
            let mut batch_transfers: Vec<DbBatchTransfer> = db_data
                .batch_transfers
                .clone()
                .into_iter()
                .filter(|t| t.failed())
                .collect();
            for batch_transfer in batch_transfers.iter_mut() {
                let asset_transfers =
                    batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;
                if of_live_invoice(&asset_transfers) {
                    continue;
                }
                if no_asset_only {
                    let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                    if connected_assets {
                        continue;
                    }
                }
                transfers_changed = true;
                self._delete_batch_transfer(
                    batch_transfer,
                    &asset_transfers,
                    &db_data.colorings,
                    &db_data.txos,
                )?
            }
        }

        if transfers_changed {
            self.update_backup_info(false)?;
        }

        info!(self.logger, "Delete transfer completed");
        Ok(transfers_changed)
    }

    /// Get the checked mints received on `recipient_id`, one per transfer. A failed transfer has none.
    /// Make each lock for [`ReceivedMint::lock_id`] and [`ReceivedMint::amount`], and include it
    /// before [`ReceivedMint::before_block`]. The reject list does not refuse a checked mint later.
    pub fn get_received_mint(&self, recipient_id: String) -> Result<Vec<ReceivedMint>, Error> {
        info!(self.logger, "Getting received mint...");
        // a failed receive has nothing left to lock for
        let db_data = self.database.get_db_data(false)?;
        let mut received_mints = vec![];
        for transfer in db_data
            .transfers
            .iter()
            .filter(|t| t.incoming && t.recipient_id.as_deref() == Some(&recipient_id))
        {
            let failed = db_data
                .asset_transfers
                .iter()
                .find(|at| at.idx == transfer.asset_transfer_idx)
                .and_then(|at| {
                    db_data
                        .batch_transfers
                        .iter()
                        .find(|bt| bt.idx == at.batch_transfer_idx)
                })
                .is_some_and(|bt| bt.status == TransferStatus::Failed);
            if failed {
                continue;
            }
            if let Some(mint) = self.received_mints.get(&self.get_receive_dir(transfer)) {
                received_mints.push(mint.clone());
            }
        }
        info!(self.logger, "Get received mint completed");
        Ok(received_mints)
    }

    /// Return the consignment of the burn of `asset_id` with TXID `txid`. It proves the burn with
    /// the signed transaction and the full history. The wallet keeps it once the burn is ended.
    ///
    /// Fails with [`Error::UnknownTransfer`] if the wallet has no ended burn of the asset with
    /// that TXID.
    pub fn get_burn_consignment(&self, asset_id: String, txid: String) -> Result<Vec<u8>, Error> {
        info!(self.logger, "Getting burn consignment...");
        let transfers = self.list_transfers(
            AssetFilter::Id(asset_id.clone()),
            TransferScope::Txid(txid.clone()),
        )?;
        if !transfers.iter().any(|t| t.kind == TransferKind::Burn) {
            return Err(Error::UnknownTransfer { txid });
        }
        let bytes = self
            .transfer_artifacts
            .get(&txid)
            .and_then(|a| a.consignment_bytes.get(&asset_id))
            .cloned()
            .ok_or(Error::UnknownTransfer { txid })?;
        info!(self.logger, "Get burn consignment completed");
        Ok(bytes)
    }

    /// Return the [`Metadata`] of the RGB asset with the given ID.
    pub fn get_asset_metadata(&self, asset_id: String) -> Result<Metadata, Error> {
        info!(self.logger, "Getting metadata for asset '{}'...", asset_id);
        let asset = self.database.check_asset_exists(asset_id)?;

        let initial_supply = asset.initial_supply.parse::<u64>().unwrap();
        let max_supply = if let Some(max_supply) = asset.max_supply {
            max_supply.parse::<u64>().unwrap()
        } else {
            initial_supply
        };
        let known_circulating_supply =
            if let Some(known_circulating_supply) = asset.known_circulating_supply {
                known_circulating_supply.parse::<u64>().unwrap()
            } else {
                initial_supply
            };

        info!(self.logger, "Get asset metadata completed");
        Ok(Metadata {
            asset_schema: asset.schema,
            initial_supply,
            max_supply,
            known_circulating_supply,
            timestamp: asset.timestamp,
            name: asset.name,
            precision: asset.precision,
            ticker: asset.ticker,
            details: asset.details,
            bridge_location: asset.bridge_location,
            reject_list: asset.reject_list,
            linked_from_contract: asset.linked_from_contract,
            linked_to_contract: asset.linked_to_contract,
        })
    }

    /// Return the contract (no bundles) of the RGB asset with the given ID, with the media it
    /// references. It changes no wallet state.
    pub fn export_contract(&self, asset_id: String) -> Result<ExportedContract, Error> {
        info!(self.logger, "Exporting contract '{}'...", asset_id);
        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let contract_id = ContractId::from_str(&asset_id).expect("asset ID in DB should be valid");
        let runtime = self.rgb_runtime()?;
        let contract = runtime.export_contract(contract_id)?;
        let attachment = match asset.schema {
            AssetSchema::Nia => {
                runtime
                    .contract_wrapper::<NonInflatableAsset>(contract_id)?
                    .contract_terms()
                    .media
            }
            AssetSchema::Ifa => {
                runtime
                    .contract_wrapper::<InflatableFungibleAsset>(contract_id)?
                    .contract_terms()
                    .media
            }
            AssetSchema::Bfa => {
                runtime
                    .contract_wrapper::<BridgedFungibleAsset>(contract_id)?
                    .contract_terms()
                    .media
            }
        };
        drop(runtime);
        let media = attachment
            .into_iter()
            .map(|a| {
                let digest = hex::encode(a.digest);
                self.media.get(&digest).cloned().ok_or(Error::IO {
                    details: format!("media {digest} not found"),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut bytes = vec![];
        contract.save(&mut bytes)?;
        info!(self.logger, "Export contract completed");
        Ok(ExportedContract {
            contract: bytes,
            media,
        })
    }

    /// Import the contract of an RGB asset (as `export_contract` gives it), with no allocations.
    ///
    /// Media come from the wallet, or else from `media`. A refused import changes nothing.
    /// [`flush`](Wallet::flush) makes an import durable.
    pub fn import_contract(&mut self, contract: &[u8], media: Vec<Vec<u8>>) -> Result<(), Error> {
        let cp = self.checkpoint("import_contract");
        let res = self._import_contract(contract, media);
        self.commit_queued(cp, res)
    }

    fn _import_contract(&mut self, contract: &[u8], media: Vec<Vec<u8>>) -> Result<(), Error> {
        info!(self.logger, "Importing contract...");
        if contract.len() as u64 > MAX_CONTRACT_SIZE {
            return Err(Error::FileTooLarge {
                max_size: MAX_CONTRACT_SIZE,
            });
        }
        let contract = Contract::load(contract).map_err(|_| Error::InvalidContract)?;
        let asset_id = contract.contract_id().to_string();
        let asset_schema: AssetSchema = contract.schema_id().try_into()?;
        self.check_schema_support(&asset_schema)?;
        if self.database.get_asset(asset_id.clone())?.is_some() {
            return Err(Error::AssetAlreadyKnown { asset_id });
        }

        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            ..Default::default()
        };
        let valid_contract = contract
            .validate(
                &asset_schema.schema_rules(),
                &DumbResolver,
                &validation_config,
            )
            .map_err(|e| match e {
                ValidationError::InvalidConsignment(e) => Error::ConsignmentValidationFailed {
                    details: e.to_string(),
                },
                e => Error::Internal {
                    details: format!("unexpected consignment validation error: {e}"),
                },
            })?;

        let mut runtime = self.rgb_runtime()?;
        let attachments = self.extract_attachments(&runtime, &valid_contract, asset_schema)?;
        let mut provided: HashMap<String, Vec<u8>> = media
            .into_iter()
            .map(|m| (hex::encode(sha256_digest(&m)), m))
            .collect();
        for attachment in attachments {
            let digest = hex::encode(attachment.digest);
            if self.media.contains_key(&digest) {
                continue;
            }
            let Some(bytes) = provided.remove(&digest) else {
                return Err(Error::InvalidAttachments {
                    details: s!("media defined by the contract was not provided"),
                });
            };
            self.media.insert(digest, bytes);
        }
        self.save_new_asset_internal(&mut runtime, asset_schema, valid_contract, None)?;
        drop(runtime);
        self.update_backup_info(false)?;
        info!(self.logger, "Import contract '{}' completed", asset_id);
        Ok(())
    }

    /// Return the [`Balance`] for the RGB asset with the given ID.
    pub fn get_asset_balance(&self, asset_id: String) -> Result<Balance, Error> {
        info!(self.logger, "Getting balance for asset '{}'...", asset_id);
        self.database.check_asset_exists(asset_id.clone())?;
        let balance = self
            .database
            .get_asset_balance(asset_id, None, None, None, None, None);
        info!(self.logger, "Get asset balance completed");
        balance
    }

    /// Return the [`BtcBalance`] of the internal Bitcoin wallets.
    pub async fn get_btc_balance(
        &mut self,
        online: Option<Online>,
        skip_sync: bool,
    ) -> Result<BtcBalance, Error> {
        info!(self.logger, "Getting BTC balance...");

        self.sync_if_requested(online, skip_sync, "get_btc_balance")
            .await?;

        let mut vanilla = self._get_btc_balance(KeychainKind::Internal)?;
        let reserved = self.get_reserved_vanilla_outpoints()?;
        let reserved_btc: u64 = self
            .internal_unspents()
            .filter(|u| reserved.contains(&u.outpoint))
            .map(|u| u.txout.value.to_sat())
            .sum();
        vanilla.spendable = vanilla.spendable.saturating_sub(reserved_btc);
        let colored = self._get_btc_balance(KeychainKind::External)?;

        let balance = BtcBalance { vanilla, colored };

        info!(self.logger, "Get BTC balance completed");
        Ok(balance)
    }

    /// List the known RGB assets.
    ///
    /// Providing an empty `filter_asset_schemas` will list assets for all schemas, otherwise only
    /// assets for the provided schemas will be listed.
    ///
    /// The returned `Assets` will have fields set to `None` for schemas that have not been
    /// requested.
    pub fn list_assets(&self, mut filter_asset_schemas: Vec<AssetSchema>) -> Result<Assets, Error> {
        info!(self.logger, "Listing assets...");
        if filter_asset_schemas.is_empty() {
            filter_asset_schemas = AssetSchema::VALUES.to_vec()
        }

        let batch_transfers = Some(self.database.iter_batch_transfers()?);
        let colorings = Some(self.database.iter_colorings()?);
        let txos = Some(self.database.iter_txos()?);
        let asset_transfers = Some(self.database.iter_asset_transfers()?);
        let transfers = Some(self.database.iter_transfers()?);
        let medias = Some(self.database.iter_media()?);

        let assets = self.database.iter_assets()?;
        let mut nia = None;
        let mut ifa = None;
        let mut bfa = None;
        for schema in filter_asset_schemas {
            match schema {
                AssetSchema::Nia => {
                    nia = Some(
                        assets
                            .iter()
                            .filter(|a| a.schema == schema)
                            .map(|a| {
                                AssetNIA::get_asset_details(
                                    self,
                                    a,
                                    transfers.clone(),
                                    asset_transfers.clone(),
                                    batch_transfers.clone(),
                                    colorings.clone(),
                                    txos.clone(),
                                    medias.clone(),
                                )
                            })
                            .collect::<Result<Vec<AssetNIA>, Error>>()?,
                    );
                }
                AssetSchema::Ifa => {
                    ifa = Some(
                        assets
                            .iter()
                            .filter(|a| a.schema == schema)
                            .map(|a| {
                                AssetIFA::get_asset_details(
                                    self,
                                    a,
                                    transfers.clone(),
                                    asset_transfers.clone(),
                                    batch_transfers.clone(),
                                    colorings.clone(),
                                    txos.clone(),
                                    medias.clone(),
                                )
                            })
                            .collect::<Result<Vec<AssetIFA>, Error>>()?,
                    );
                }
                AssetSchema::Bfa => {
                    bfa = Some(
                        assets
                            .iter()
                            .filter(|a| a.schema == schema)
                            .map(|a| {
                                AssetBFA::get_asset_details(
                                    self,
                                    a,
                                    transfers.clone(),
                                    asset_transfers.clone(),
                                    batch_transfers.clone(),
                                    colorings.clone(),
                                    txos.clone(),
                                    medias.clone(),
                                )
                            })
                            .collect::<Result<Vec<AssetBFA>, Error>>()?,
                    );
                }
            }
        }

        info!(self.logger, "List assets completed");
        Ok(Assets { nia, ifa, bfa })
    }

    pub(crate) async fn sync_if_requested(
        &mut self,
        #[cfg_attr(not(feature = "esplora"), allow(unused_variables))] online: Option<Online>,
        skip_sync: bool,
        #[cfg_attr(not(feature = "esplora"), allow(unused_variables))] op: &'static str,
    ) -> Result<(), Error> {
        if !skip_sync {
            #[cfg(not(feature = "esplora"))]
            return Err(Error::Offline);
            #[cfg(feature = "esplora")]
            {
                if let Some(online) = online {
                    self.check_online(online)?;
                } else {
                    return Err(Error::OnlineNeeded);
                }
                let cp = self.checkpoint(op);
                let res = async {
                    self.fast_sync(true).await?;
                    self.fast_sync(false).await
                }
                .await;
                self.commit(cp, res, |_| false).await?;
            }
        }
        Ok(())
    }

    /// List the Bitcoin [`Transaction`]s known to the wallet.
    pub async fn list_transactions(
        &mut self,
        online: Option<Online>,
        skip_sync: bool,
    ) -> Result<Vec<Transaction>, Error> {
        info!(self.logger, "Listing transactions...");

        self.sync_if_requested(online, skip_sync, "list_transactions")
            .await?;

        let mut create_utxos_txids = vec![];
        let mut drain_txids = vec![];
        let mut send_btc_txids = vec![];
        let wallet_transactions = self.database.iter_wallet_transactions()?;
        for tx in &wallet_transactions {
            match tx.r#type {
                WalletTransactionType::CreateUtxos => create_utxos_txids.push(tx.txid.clone()),
                WalletTransactionType::Drain => drain_txids.push(tx.txid.clone()),
                WalletTransactionType::SendBtc => send_btc_txids.push(tx.txid.clone()),
            }
        }
        let rgb_send_txids: Vec<String> = self
            .database
            .iter_batch_transfers()?
            .into_iter()
            .filter_map(|t| t.txid)
            .collect();
        let mut transactions = self
            .bdk_wallet
            .transactions()
            .map(|t| {
                let txid = t.tx_node.txid.to_string();
                let transaction_type = if drain_txids.contains(&txid) {
                    TransactionType::Drain
                } else if create_utxos_txids.contains(&txid) {
                    TransactionType::CreateUtxos
                } else if rgb_send_txids.contains(&txid) {
                    TransactionType::RgbSend
                } else if send_btc_txids.contains(&txid) {
                    TransactionType::SendBtc
                } else {
                    TransactionType::Incoming
                };
                let confirmation_time = match t.chain_position {
                    ChainPosition::Confirmed { anchor, .. } => Some(BlockTime {
                        height: anchor.block_id.height,
                        timestamp: anchor.confirmation_time,
                    }),
                    _ => None,
                };
                let (sent, received) = self.bdk_wallet.sent_and_received(&t.tx_node);
                let fee = self.bdk_wallet.calculate_fee(&t.tx_node).unwrap();
                Transaction {
                    transaction_type,
                    txid,
                    received: received.to_sat(),
                    sent: sent.to_sat(),
                    fee: fee.to_sat(),
                    confirmation_time,
                    pending: false,
                }
            })
            .collect::<Vec<_>>();
        let pending: Vec<Transaction> = wallet_transactions
            .into_iter()
            .filter(|w| w.status == TransferStatus::Initiated)
            .filter(|w| !transactions.iter().any(|t| t.txid == w.txid))
            .map(|w| Transaction {
                transaction_type: match w.r#type {
                    WalletTransactionType::CreateUtxos => TransactionType::CreateUtxos,
                    WalletTransactionType::Drain => TransactionType::Drain,
                    WalletTransactionType::SendBtc => TransactionType::SendBtc,
                },
                txid: w.txid,
                received: w.received,
                sent: w.sent,
                fee: w.fee,
                confirmation_time: None,
                pending: true,
            })
            .collect();
        transactions.extend(pending);
        info!(self.logger, "List transactions completed");
        Ok(transactions)
    }

    /// List the begun vanilla operations that are not ended or aborted.
    pub fn list_pending_vanilla_txs(&self) -> Result<Vec<PendingVanillaTx>, Error> {
        info!(self.logger, "Listing pending vanilla TXs...");
        let pending = self
            .database
            .iter_wallet_transactions()?
            .into_iter()
            .filter(|w| w.status == TransferStatus::Initiated)
            .map(|w| PendingVanillaTx {
                txid: w.txid,
                r#type: w.r#type,
            })
            .collect();
        info!(self.logger, "List pending vanilla TXs completed");
        Ok(pending)
    }

    /// Abort the begun vanilla operation with the given TXID: delete it and release its inputs.
    pub fn abort_pending_vanilla_tx(&self, txid: String) -> Result<(), Error> {
        let cp = self.shared_checkpoint("abort_pending_vanilla_tx");
        let res = self._abort_pending_vanilla_tx(txid);
        self.commit_shared(cp, res)
    }

    fn _abort_pending_vanilla_tx(&self, txid: String) -> Result<(), Error> {
        info!(self.logger, "Aborting pending vanilla TX {}...", txid);
        let row = self
            .database
            .get_wallet_transaction_by_txid(&txid)?
            .filter(|w| w.status == TransferStatus::Initiated)
            .ok_or(Error::CannotAbortPendingVanillaTx)?;
        self.database.del_wallet_transaction(row.idx)?;
        self.update_backup_info(false)?;
        info!(self.logger, "Abort pending vanilla TX completed");
        Ok(())
    }

    pub(crate) fn normalize_recipient_id(&self, recipient_id: &str) -> String {
        recipient_id.replace(":", "_")
    }

    /// Key of the data of an incoming transfer: its receive dir, else its normalized recipient ID.
    pub(crate) fn get_receive_dir(&self, transfer: &DbTransfer) -> String {
        transfer.receive_dir.clone().unwrap_or_else(|| {
            self.normalize_recipient_id(
                transfer
                    .recipient_id
                    .as_ref()
                    .expect("transfer should have a recipient ID"),
            )
        })
    }

    pub(crate) fn get_receive_consignment_path(&self, transfer: &DbTransfer) -> PathBuf {
        self.get_transfers_dir()
            .join(self.get_receive_dir(transfer))
            .join(CONSIGNMENT_RCV_FILE.as_str())
    }

    pub(crate) fn get_transfer_data(
        &self,
        transfer: &DbTransfer,
        asset_transfer: &DbAssetTransfer,
        batch_transfer: &DbBatchTransfer,
        txos: &[DbTxo],
        colorings: &[DbColoring],
    ) -> Result<TransferData, Error> {
        let filtered_coloring = colorings
            .iter()
            .filter(|&c| c.asset_transfer_idx == asset_transfer.idx)
            .cloned();

        let assignments = filtered_coloring
            .clone()
            .filter(|c| c.r#type != ColoringType::Input)
            .map(|c| c.assignment)
            .collect();

        let kind = if transfer.incoming {
            if filtered_coloring.clone().count() > 0
                && filtered_coloring
                    .clone()
                    .all(|c| c.r#type == ColoringType::Issue)
            {
                TransferKind::Issuance
            } else {
                match transfer.recipient_type.as_ref().unwrap() {
                    RecipientTypeFull::Blind { .. } => TransferKind::ReceiveBlind,
                    RecipientTypeFull::Witness { .. } => TransferKind::ReceiveWitness,
                }
            }
        } else if asset_transfer.transition_type == Some(TypeOfTransition::Mint) {
            TransferKind::Mint
        } else if asset_transfer.transition_type == Some(TypeOfTransition::Link) {
            TransferKind::Link
        } else if transfer.recipient_id.is_none() {
            // a burn is the only other outgoing transfer without a recipient
            TransferKind::Burn
        } else if filtered_coloring.clone().count() > 0
            && filtered_coloring
                .clone()
                .any(|c| c.r#type == ColoringType::Issue)
        {
            // inflation transfer is outgoing and connected to issue colorings
            TransferKind::Inflation
        } else {
            TransferKind::Send
        };

        let txo_ids: Vec<i32> = filtered_coloring.clone().map(|c| c.txo_idx).collect();
        let transfer_txos: Vec<DbTxo> = txos
            .iter()
            .filter(|&t| txo_ids.contains(&t.idx))
            .cloned()
            .collect();
        let receive_utxo = match &transfer.recipient_type {
            Some(RecipientTypeFull::Blind { unblinded_utxo }) => Some(unblinded_utxo.clone()),
            Some(RecipientTypeFull::Witness { vout }) => {
                let received_txo_idx: Vec<i32> = filtered_coloring
                    .clone()
                    // issue coloring from inflation is considered as received
                    .filter(|c| [ColoringType::Receive, ColoringType::Issue].contains(&c.r#type))
                    .map(|c| c.txo_idx)
                    .collect();
                transfer_txos
                    .clone()
                    .into_iter()
                    .filter(|t| received_txo_idx.contains(&t.idx) && t.vout == vout.unwrap())
                    .map(|t| t.outpoint())
                    .collect::<Vec<Outpoint>>()
                    .first()
                    .cloned()
            }
            _ => None,
        };
        let change_utxos = match kind {
            TransferKind::ReceiveBlind | TransferKind::ReceiveWitness => vec![],
            TransferKind::Send
            | TransferKind::Inflation
            | TransferKind::Burn
            | TransferKind::Link
            | TransferKind::Mint => {
                let change_txo_idx: Vec<i32> = filtered_coloring
                    .filter(|c| c.r#type == ColoringType::Change)
                    .map(|c| c.txo_idx)
                    .collect();
                let mut change_txos: Vec<DbTxo> = transfer_txos
                    .into_iter()
                    .filter(|t| change_txo_idx.contains(&t.idx))
                    .collect();
                change_txos.sort_by_key(|t| t.idx);
                change_txos.into_iter().map(|t| t.outpoint()).collect()
            }
            TransferKind::Issuance => vec![],
        };

        let consignment_path = match (&kind, batch_transfer.status) {
            (
                TransferKind::Send
                | TransferKind::Inflation
                | TransferKind::Burn
                | TransferKind::Link
                | TransferKind::Mint,
                _,
            ) => Some(self.get_send_consignment_path(
                &asset_transfer.asset_id.clone().unwrap(),
                &batch_transfer.txid.clone().unwrap(),
            )),
            (
                TransferKind::ReceiveBlind | TransferKind::ReceiveWitness,
                TransferStatus::WaitingCounterparty,
            ) => None,
            (TransferKind::ReceiveBlind | TransferKind::ReceiveWitness, _) => {
                Some(self.get_receive_consignment_path(transfer))
            }
            (TransferKind::Issuance, _) => {
                Some(self._get_issue_consignment_path(&asset_transfer.asset_id.clone().unwrap()))
            }
        }
        .map(|p| p.to_string_lossy().to_string());

        let psbt = batch_transfer.unsigned_psbt.clone();

        Ok(TransferData {
            kind,
            status: batch_transfer.status,
            batch_transfer_idx: batch_transfer.idx,
            assignments,
            txid: batch_transfer.txid.clone(),
            receive_utxo,
            change_utxos,
            created_at: batch_transfer.created_at,
            updated_at: batch_transfer.updated_at,
            expiration: batch_transfer.expiration,
            consignment_path,
            psbt,
        })
    }

    /// List the RGB [`Transfer`]s known to the wallet, filtered by `asset_filter` and `scope`.
    ///
    /// An unknown TXID yields an empty list, while an unknown batch transfer index returns
    /// [`Error::BatchTransferNotFound`].
    pub fn list_transfers(
        &self,
        asset_filter: AssetFilter,
        scope: TransferScope,
    ) -> Result<Vec<Transfer>, Error> {
        info!(
            self.logger,
            "Listing transfers for filter '{:?}' and scope '{:?}'...", asset_filter, scope
        );
        if let AssetFilter::Id(asset_id) = &asset_filter {
            self.database.check_asset_exists(asset_id.clone())?;
        }
        let db_data = self.database.get_db_data(false)?;
        let batch_transfer_idxs: Option<Vec<i32>> = match &scope {
            TransferScope::All => None,
            TransferScope::Txid(txid) => Some(
                db_data
                    .batch_transfers
                    .iter()
                    .filter(|b| b.txid.as_deref() == Some(txid))
                    .map(|b| b.idx)
                    .collect(),
            ),
            TransferScope::BatchTransfer(idx) => {
                self.database
                    .get_batch_transfer_or_fail(*idx, &db_data.batch_transfers)?;
                Some(vec![*idx])
            }
        };
        let asset_transfer_ids: Vec<i32> = db_data
            .asset_transfers
            .iter()
            .filter(|t| match &asset_filter {
                AssetFilter::AnyOrNone => true,
                AssetFilter::None => t.asset_id.is_none(),
                AssetFilter::Id(asset_id) => t.asset_id.as_ref() == Some(asset_id),
            })
            .filter(|t| {
                batch_transfer_idxs
                    .as_ref()
                    .is_none_or(|idxs| idxs.contains(&t.batch_transfer_idx))
            })
            .filter(|t| t.user_driven)
            .map(|t| t.idx)
            .collect();
        let transfers: Vec<Transfer> = db_data
            .transfers
            .into_iter()
            .filter(|t| asset_transfer_ids.contains(&t.asset_transfer_idx))
            .map(|t| {
                let (asset_transfer, batch_transfer) =
                    t.related_transfers(&db_data.asset_transfers, &db_data.batch_transfers)?;
                let tte_data = self.database.get_transfer_transport_endpoints_data(t.idx)?;
                Ok(Transfer::from_db_transfer(
                    &t,
                    self.get_transfer_data(
                        &t,
                        &asset_transfer,
                        &batch_transfer,
                        &db_data.txos,
                        &db_data.colorings,
                    )?,
                    tte_data
                        .iter()
                        .filter(|(tte, _)| tte.kind == TransportEndpointKind::RecipientProxy)
                        .map(|(tte, ce)| {
                            TransferTransportEndpoint::from_db_transfer_transport_endpoint(tte, ce)
                        })
                        .collect(),
                    tte_data
                        .iter()
                        .find(|(tte, _)| tte.kind == TransportEndpointKind::SenderProxy)
                        .map(|(_, ce)| ce.endpoint.clone()),
                ))
            })
            .collect::<Result<Vec<Transfer>, Error>>()?;

        info!(self.logger, "List transfers completed");
        Ok(transfers)
    }

    /// List the [`Unspent`]s known to the wallet.
    ///
    /// If `settled` is true only show settled RGB allocations, if false also show pending RGB
    /// allocations.
    pub async fn list_unspents(
        &mut self,
        online: Option<Online>,
        settled_only: bool,
        skip_sync: bool,
    ) -> Result<Vec<Unspent>, Error> {
        info!(self.logger, "Listing unspents...");

        self.sync_if_requested(online, skip_sync, "list_unspents")
            .await?;

        let db_data = self.database.get_db_data(false)?;

        let mut allocation_txos = self.database.get_unspent_txos(db_data.txos.clone())?;
        let spent_txos_ids: Vec<i32> = db_data
            .txos
            .clone()
            .into_iter()
            .filter(|t| t.spent)
            .map(|u| u.idx)
            .collect();
        let waiting_confs_batch_transfer_ids: Vec<i32> = db_data
            .batch_transfers
            .clone()
            .into_iter()
            .filter(|t| t.waiting_confirmations())
            .map(|t| t.idx)
            .collect();
        let waiting_confs_transfer_ids: Vec<i32> = db_data
            .asset_transfers
            .clone()
            .into_iter()
            .filter(|t| waiting_confs_batch_transfer_ids.contains(&t.batch_transfer_idx))
            .map(|t| t.idx)
            .collect();
        let almost_spent_txos_ids: Vec<i32> = db_data
            .colorings
            .clone()
            .into_iter()
            .filter(|c| {
                waiting_confs_transfer_ids.contains(&c.asset_transfer_idx)
                    && spent_txos_ids.contains(&c.txo_idx)
            })
            .map(|c| c.txo_idx)
            .collect();
        let mut spent_txos = db_data
            .txos
            .into_iter()
            .filter(|t| almost_spent_txos_ids.contains(&t.idx))
            .collect();
        allocation_txos.append(&mut spent_txos);

        let mut txos_allocations = self.database.get_rgb_allocations(
            allocation_txos,
            Some(db_data.colorings),
            Some(db_data.batch_transfers),
            Some(db_data.asset_transfers),
            Some(db_data.transfers),
        )?;

        txos_allocations
            .iter_mut()
            .for_each(|t| t.rgb_allocations.retain(|a| a.settled() || a.future()));

        txos_allocations.retain(|t| !(t.rgb_allocations.is_empty() && t.utxo.spent));

        let mut unspents: Vec<Unspent> = txos_allocations.into_iter().map(Unspent::from).collect();

        if settled_only {
            unspents
                .iter_mut()
                .for_each(|u| u.rgb_allocations.retain(|a| a.settled));
        }

        let spk_index = self.bdk_wallet.spk_index();
        for unspent in unspents.iter_mut() {
            if let Some(((KeychainKind::External, derivation_index), _)) =
                spk_index.txout(BdkOutPoint::from(unspent.utxo.outpoint.clone()))
            {
                unspent.utxo.derivation_index = Some(derivation_index);
            }
        }
        let mut internal_unspents: Vec<Unspent> =
            self.internal_unspents().map(Unspent::from).collect();

        unspents.append(&mut internal_unspents);

        info!(self.logger, "List unspents completed");
        Ok(unspents)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_receive_data(
        &mut self,
        online: Option<Online>,
        asset_id: Option<String>,
        assignment: Assignment,
        expiration_timestamp: i64,
        transport_endpoints: Vec<String>,
        recipient_type: RecipientType,
        encryption: Option<Encryption>,
        quota: u32,
    ) -> Result<ReceiveDataInternal, Error> {
        let encryption = encryption.unwrap_or(Encryption::Required);

        // A reusable invoice needs proxies to keep its transfers apart.
        // The seal blinding from both parties' keys identifies the outputs of each transfer.
        if quota == 0 {
            return Err(Error::InvalidQuota {
                details: s!("must be at least 1"),
            });
        }
        if transport_endpoints.is_empty() && quota > 1 {
            return Err(Error::InvalidQuota {
                details: s!("a reusable invoice requires transport endpoints"),
            });
        }
        // The blinded UTXO needs one allocation for each transfer.
        if recipient_type == RecipientType::Blind && quota > self.max_utxo_allocations() {
            return Err(Error::InvalidQuota {
                details: format!(
                    "a blind invoice can receive at most {} transfers",
                    self.max_utxo_allocations()
                ),
            });
        }

        #[cfg(not(feature = "esplora"))]
        {
            let _ = online;
            return Err(Error::Offline);
        }
        #[cfg(feature = "esplora")]
        {
            let online = online.ok_or(Error::OnlineNeeded)?;
            self.check_online(online)?;

            let (beneficiary, recipient_type_full, blind_seal, script_pubkey) = match recipient_type
            {
                RecipientType::Blind => {
                    let mut unspents: Vec<LocalUnspent> = self.database.get_rgb_allocations(
                        self.database.get_unspent_txos(vec![])?,
                        None,
                        None,
                        None,
                        None,
                    )?;
                    unspents.retain(|u| {
                        !(u.rgb_allocations
                            .iter()
                            .any(|a| !a.incoming && a.status.waiting_counterparty()))
                    });
                    // A UTXO holds at most one mint right of an asset.
                    if let (Assignment::MintRight, Some(aid)) = (&assignment, &asset_id) {
                        let invoiced = self.database.get_mint_right_invoice_utxos(aid)?;
                        unspents.retain(|u| {
                            !invoiced.contains(&u.utxo.outpoint())
                                && !u.rgb_allocations.iter().any(|a| {
                                    a.incoming
                                        && !a.txo_spent
                                        && !a.status.failed()
                                        && a.asset_id.as_deref() == Some(aid.as_str())
                                        && a.assignment == Assignment::MintRight
                                })
                        });
                    }
                    // The blinded UTXO needs one allocation for each transfer.
                    let max_allocations = self.max_utxo_allocations() - quota;
                    let utxo = self.get_utxo(&[], Some(&unspents), true, Some(max_allocations))?;
                    let unblinded_utxo = utxo.outpoint();
                    debug!(
                        self.logger,
                        "Blinding outpoint '{}'",
                        unblinded_utxo.to_string()
                    );
                    let blind_seal = self.get_blind_seal(utxo.clone()).transmutate();
                    let beneficiary = Beneficiary::BlindedSeal(blind_seal.conceal());
                    let recipient_type_full = RecipientTypeFull::Blind { unblinded_utxo };
                    (beneficiary, recipient_type_full, Some(blind_seal), None)
                }
                RecipientType::Witness => {
                    let script_pubkey = self.get_new_address()?.script_pubkey();
                    let beneficiary = beneficiary_from_script_buf(script_pubkey.clone());
                    let recipient_type_full = RecipientTypeFull::Witness { vout: None };
                    (beneficiary, recipient_type_full, None, Some(script_pubkey))
                }
            };

            let network: ChainNet = self.bitcoin_network().into();
            let beneficiary = XChainNet::with(network, beneficiary);
            let recipient_id = beneficiary.to_string();
            debug!(self.logger, "Recipient ID: {recipient_id}");
            let (schema, contract_id) = if let Some(aid) = asset_id.clone() {
                let asset = self.database.check_asset_exists(aid.clone())?;
                let contract_id = ContractId::from_str(&aid).expect("invalid contract ID");
                (Some(asset.schema), Some(contract_id))
            } else {
                (None, None)
            };

            self.check_transport_endpoints(&transport_endpoints)?;
            let mut transport_endpoints_dedup = transport_endpoints.clone();
            transport_endpoints_dedup.sort();
            transport_endpoints_dedup.dedup();
            if transport_endpoints_dedup.len() != transport_endpoints.len() {
                return Err(Error::InvalidTransportEndpoints {
                    details: s!("no duplicate transport endpoints allowed"),
                });
            }
            let mut endpoints: Vec<String> = vec![];
            for endpoint_str in &transport_endpoints {
                let rgb_transport = RgbTransport::from_str(endpoint_str)?;
                match &rgb_transport {
                    RgbTransport::JsonRpc { .. } => {
                        endpoints.push(
                            TransportEndpoint::try_from(rgb_transport)
                                .unwrap()
                                .endpoint
                                .clone(),
                        );
                    }
                    _ => {
                        return Err(Error::UnsupportedTransportType);
                    }
                }
            }

            let mut invoice_builder = RgbInvoiceBuilder::new(beneficiary);
            if let Some(schema) = schema {
                invoice_builder = invoice_builder.set_schema(schema.into());
            }
            if let Some(contract_id) = contract_id {
                invoice_builder = invoice_builder.set_contract(contract_id);
            }
            let transports: Vec<&str> = transport_endpoints.iter().map(AsRef::as_ref).collect();
            invoice_builder = invoice_builder.add_transports(transports).unwrap();
            let detected_assignment = match (&assignment, schema) {
                (
                    Assignment::Fungible(amt),
                    Some(AssetSchema::Nia) | Some(AssetSchema::Ifa) | Some(AssetSchema::Bfa) | None,
                ) => {
                    invoice_builder = invoice_builder.set_amount_raw(*amt);
                    invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_ASSET_OWNER);
                    assignment
                }
                (Assignment::Any, Some(AssetSchema::Nia)) => {
                    invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_ASSET_OWNER);
                    Assignment::Fungible(0)
                }
                (Assignment::InflationRight(amt), Some(AssetSchema::Ifa)) => {
                    invoice_builder = invoice_builder.set_amount_raw(*amt);
                    invoice_builder =
                        invoice_builder.set_assignment_name(RGB_STATE_INFLATION_ALLOWANCE);
                    assignment
                }
                (Assignment::LinkRight, Some(AssetSchema::Ifa) | Some(AssetSchema::Bfa)) => {
                    invoice_builder = invoice_builder.set_void();
                    invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_LINK_RIGHT);
                    assignment
                }
                (Assignment::MintRight, Some(AssetSchema::Bfa)) => {
                    invoice_builder = invoice_builder.set_void();
                    invoice_builder = invoice_builder.set_assignment_name(RGB_STATE_MINT_RIGHT);
                    assignment
                }
                (Assignment::Any, _) => Assignment::Any,
                _ => return Err(Error::InvalidAssignment),
            };
            let created_at = now().unix_timestamp();
            if expiration_timestamp < created_at {
                return Err(Error::InvalidExpiration);
            }
            invoice_builder = invoice_builder.set_expiry_timestamp(expiration_timestamp);

            // the per-operation Ed25519 key: the invoice carries its public key and the
            // encryption preference
            let recipient_sk = crate::encryption::gen_ed25519();
            let recipient_vk = recipient_sk.verifying_key();
            invoice_builder = invoice_builder.set_public_key(recipient_vk.to_bytes());
            invoice_builder = invoice_builder.set_encryption(encryption.into());

            let invoice_string = invoice_builder.finish().to_string();

            // for a blind receive, the invoice expiration plus the longest retention grace among
            // the proxies, until which a transfer to the invoice can still be downloaded
            let mut retention_deadline: Option<i64> = None;
            // Expiration plus the longest retention grace of the invoice proxies, for all recipient types.
            let mut proxies_retention_deadline: Option<i64> = None;

            // reserve the invoice on the proxies
            let recipient_pk = hex::encode(recipient_vk.to_bytes());
            let invoice_id = invoice_id(&recipient_id, &recipient_pk);
            let reservation_expiration = expiration_timestamp.to_string();
            let mut reserved = false;
            // whether a proxy refused the reservation as it is draining
            let mut draining = false;
            for endpoint in &endpoints {
                // skip endpoints that are unreachable or run an unsupported protocol version
                let Ok(info) = crate::utils::check_proxy(endpoint).await else {
                    continue;
                };
                let proxy_client = ProxyClient::new(endpoint)?;
                // only needed by proxies with proxy-level authentication enabled
                let signature = self.online_data.as_ref().and_then(|online_data| {
                    online_data.proxy_auth_signature(
                        endpoint,
                        &prepare_invoice_auth_message(
                            &invoice_id,
                            &reservation_expiration,
                            &recipient_pk,
                            quota,
                        ),
                    )
                });
                match proxy_client
                    .prepare_invoice(
                        &invoice_id,
                        &reservation_expiration,
                        &recipient_pk,
                        quota,
                        signature,
                    )
                    .await
                {
                    Ok(res) if res.error.is_none() => {
                        reserved = true;
                        let deadline = expiration_timestamp
                            .saturating_add(i64::from(crate::utils::proxy_retention_grace(&info)));
                        proxies_retention_deadline = Some(std::cmp::max(
                            proxies_retention_deadline.unwrap_or(deadline),
                            deadline,
                        ));
                    }
                    Ok(res) => match map_proxy_error(res.error.unwrap()) {
                        // a draining proxy, the invoice can be reserved on the other ones
                        Error::ProxyDraining => draining = true,
                        // a definite proxy-side rejection (e.g. invoice already reserved) is fatal
                        e => return Err(e),
                    },
                    Err(_) => continue,
                }
            }
            if !reserved {
                return Err(if draining {
                    Error::ProxyDraining
                } else {
                    Error::NoValidTransportEndpoint
                });
            }
            if matches!(recipient_type_full, RecipientTypeFull::Blind { .. }) {
                retention_deadline = proxies_retention_deadline;
            }
            // The retention deadline must also pass on the proxy clocks.
            let invoice_deadline = proxies_retention_deadline
                .filter(|_| quota > 1)
                .map(|d| d.saturating_add(i64::from(clock_skew_margin())));

            // Transfers of a reusable invoice share the recipient ID.
            // The unique invoice key gives each receive its own dir.
            let receive_dir = (quota > 1).then(|| {
                format!(
                    "{}_{}",
                    self.normalize_recipient_id(&recipient_id),
                    hex::encode(&recipient_vk.to_bytes()[..8])
                )
            });

            Ok(ReceiveDataInternal {
                asset_id,
                detected_assignment,
                invoice_string,
                recipient_id,
                endpoints,
                created_at,
                expiration_timestamp,
                recipient_type_full,
                blind_seal,
                script_pubkey,
                operation_secret_key: recipient_sk.to_bytes().to_vec(),
                recipient_pubkey: recipient_vk.to_bytes().to_vec(),
                encryption,
                retention_deadline,
                quota,
                invoice_deadline,
                receive_dir,
            })
        }
    }

    fn store_receive_transfer(
        &self,
        data: &ReceiveDataInternal,
        min_confirmations: u8,
    ) -> Result<i32, Error> {
        let batch_transfer = DbBatchTransferActMod {
            status: ActiveValue::Set(TransferStatus::WaitingCounterparty),
            expiration: ActiveValue::Set(Some(data.expiration_timestamp)),
            created_at: ActiveValue::Set(data.created_at),
            min_confirmations: ActiveValue::Set(min_confirmations),
            operation_secret_key: ActiveValue::Set(Some(data.operation_secret_key.clone())),
            retention_deadline: ActiveValue::Set(data.retention_deadline),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let asset_transfer = DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
            asset_id: ActiveValue::Set(data.asset_id.clone()),
            ..Default::default()
        };
        let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
        // A reusable invoice receives more transfers after the first one, until it expires.
        let invoice_idx = data
            .invoice_deadline
            .map(|deadline| {
                self.database.set_invoice(DbInvoiceActMod {
                    quota: ActiveValue::Set(data.quota),
                    asset_id: ActiveValue::Set(data.asset_id.clone()),
                    deadline: ActiveValue::Set(deadline),
                    ..Default::default()
                })
            })
            .transpose()?;
        // witness receives on a reused address share the recipient ID
        let receive_dir = data.receive_dir.clone().or_else(|| {
            (self.wallet_data.reuse_addresses && data.script_pubkey.is_some()).then(|| {
                format!(
                    "{}_{batch_transfer_idx}",
                    self.normalize_recipient_id(&data.recipient_id)
                )
            })
        });
        let transfer = DbTransferActMod {
            invoice_idx: ActiveValue::Set(invoice_idx),
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            receive_dir: ActiveValue::Set(receive_dir),
            requested_assignment: ActiveValue::Set(Some(data.detected_assignment.clone())),
            incoming: ActiveValue::Set(true),
            recipient_id: ActiveValue::Set(Some(data.recipient_id.clone())),
            recipient_type: ActiveValue::Set(Some(data.recipient_type_full.clone())),
            invoice_string: ActiveValue::Set(Some(data.invoice_string.clone())),
            recipient_pubkey: ActiveValue::Set(Some(data.recipient_pubkey.clone())),
            recipient_encryption: ActiveValue::Set(data.encryption),
            ..Default::default()
        };
        let transfer_idx = self.database.set_transfer(transfer)?;
        for endpoint in &data.endpoints {
            self.save_transfer_transport_endpoint(
                transfer_idx,
                &LocalTransportEndpoint {
                    endpoint: endpoint.clone(),
                    transport_type: TransportType::JsonRpc,
                    used: false,
                    usable: true,
                },
            )?;
        }

        if let Some(blind_seal) = data.blind_seal {
            let mut runtime = self.rgb_runtime()?;
            runtime.store_secret_seal(blind_seal)?;
            // Drop runtime so Stock returns to RefCell before auto-save
            drop(runtime);
        }

        if let Some(script_pubkey) = &data.script_pubkey {
            self.database
                .set_pending_witness_script(DbPendingWitnessScriptActMod {
                    script: ActiveValue::Set(script_pubkey.to_hex_string()),
                    ..Default::default()
                })?;
        }

        Ok(batch_transfer_idx)
    }

    /// Blind an UTXO to receive RGB assets and return the resulting [`ReceiveData`].
    ///
    /// An optional asset ID can be specified, which will be embedded in the invoice, resulting in
    /// the refusal of the transfer is the asset doesn't match.
    ///
    /// An optional amount can be specified, which will be embedded in the invoice. It will not be
    /// checked when accepting the transfer.
    ///
    /// A UTXO holds at most one mint right of an asset. An [`Assignment::MintRight`] invoice blinds
    /// a UTXO that holds no mint right and is not due to receive one. The wallet refuses a transfer
    /// that pays a mint right to a UTXO that holds one.
    ///
    /// An expiration UTC timestamp must be specified, which will set the expiration of the
    /// invoice and the transfer.
    ///
    /// Each endpoint in the provided `transport_endpoints` list will be used as RGB data exchange
    /// medium. The list needs to contain at least 1 endpoint and a maximum of 3. Strings
    /// specifying invalid endpoints and duplicate ones will cause an error to be raised. A valid
    /// endpoint string encodes an
    /// [`RgbTransport`](https://docs.rs/rgb-invoicing/latest/rgbinvoice/enum.RgbTransport.html).
    /// At the moment the only supported variant is JsonRpc (e.g. `rpc://127.0.0.1` or
    /// `rpcs://example.com`).
    ///
    /// The wallet reserves the invoice on each endpoint, so it needs to be online.
    ///
    /// The `min_confirmations` number determines the minimum number of confirmations needed for
    /// the transaction anchoring the transfer for it to be considered final and move (while
    /// refreshing) to the [`TransferStatus::Settled`] status.
    ///
    /// The `encryption` preference is embedded in the invoice and is [`Encryption::Required`] when
    /// not specified.
    ///
    /// An optional `quota` (default 1) sets the number of transfers the invoice can receive. A quota
    /// above 1 requires transport endpoints. Refresh creates a transfer for each one after the first.
    /// The blinded UTXO keeps free slots and cannot be spent until the invoice expiration plus the
    /// longest retention grace of its proxies.
    #[allow(clippy::too_many_arguments)]
    pub async fn blind_receive(
        &mut self,
        online: Option<Online>,
        asset_id: Option<String>,
        assignment: Assignment,
        expiration_timestamp: u64,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
        encryption: Option<Encryption>,
        quota: Option<u32>,
    ) -> Result<ReceiveData, Error> {
        let cp = self.checkpoint("blind_receive");
        let res = self
            ._blind_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
                quota,
            )
            .await;
        self.commit(cp, res, |_| false).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn _blind_receive(
        &mut self,
        online: Option<Online>,
        asset_id: Option<String>,
        assignment: Assignment,
        expiration_timestamp: u64,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
        encryption: Option<Encryption>,
        quota: Option<u32>,
    ) -> Result<ReceiveData, Error> {
        info!(
            self.logger,
            "Receiving via blinded UTXO for asset '{:?}' with expiration '{}'...",
            asset_id,
            expiration_timestamp
        );

        let data = self
            .create_receive_data(
                online,
                asset_id,
                assignment,
                expiration_timestamp as i64,
                transport_endpoints,
                RecipientType::Blind,
                encryption,
                quota.unwrap_or(1),
            )
            .await?;
        let batch_transfer_idx = self.store_receive_transfer(&data, min_confirmations)?;

        self.update_backup_info(false)?;

        info!(self.logger, "Blind receive completed");
        Ok(ReceiveData {
            invoice: data.invoice_string,
            recipient_id: data.recipient_id,
            expiration_timestamp,
            batch_transfer_idx,
            public_key: hex::encode(&data.recipient_pubkey),
        })
    }

    /// Create an address to receive RGB assets and return the resulting [`ReceiveData`].
    ///
    /// An optional asset ID can be specified, which will be embedded in the invoice, resulting in
    /// the refusal of the transfer is the asset doesn't match.
    ///
    /// An optional amount can be specified, which will be embedded in the invoice. It will not be
    /// checked when accepting the transfer.
    ///
    /// An expiration UTC timestamp must be specified, which will set the expiration of the
    /// invoice and the transfer.
    ///
    /// Each endpoint in the provided `transport_endpoints` list will be used as RGB data exchange
    /// medium. The list needs to contain at least 1 endpoint and a maximum of 3. Strings
    /// specifying invalid endpoints and duplicate ones will cause an error to be raised. A valid
    /// endpoint string encodes an
    /// [`RgbTransport`](https://docs.rs/rgb-invoicing/latest/rgbinvoice/enum.RgbTransport.html).
    /// At the moment the only supported variant is JsonRpc (e.g. `rpc://127.0.0.1` or
    /// `rpcs://example.com`).
    ///
    /// The wallet reserves the invoice on each endpoint, so it needs to be online.
    ///
    /// The `min_confirmations` number determines the minimum number of confirmations needed for
    /// the transaction anchoring the transfer for it to be considered final and move (while
    /// refreshing) to the [`TransferStatus::Settled`] status.
    ///
    /// The `encryption` preference is embedded in the invoice and is [`Encryption::Required`] when
    /// not specified.
    ///
    /// An optional `quota` (default 1) sets the number of transfers the invoice can receive. A quota
    /// above 1 requires transport endpoints. Refresh creates a transfer for each one after the first.
    /// Each transfer uses its own seal blinding, derived from the keys of both parties. It identifies
    /// the paying output. That output cannot be spent until its transfer is accepted.
    #[allow(clippy::too_many_arguments)]
    pub async fn witness_receive(
        &mut self,
        online: Option<Online>,
        asset_id: Option<String>,
        assignment: Assignment,
        expiration_timestamp: u64,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
        encryption: Option<Encryption>,
        quota: Option<u32>,
    ) -> Result<ReceiveData, Error> {
        let cp = self.checkpoint("witness_receive");
        let res = self
            ._witness_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
                quota,
            )
            .await;
        self.commit(cp, res, |_| false).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn _witness_receive(
        &mut self,
        online: Option<Online>,
        asset_id: Option<String>,
        assignment: Assignment,
        expiration_timestamp: u64,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
        encryption: Option<Encryption>,
        quota: Option<u32>,
    ) -> Result<ReceiveData, Error> {
        info!(
            self.logger,
            "Receiving via witness TX for asset '{:?}' with expiration '{}'...",
            asset_id,
            expiration_timestamp
        );

        let data = self
            .create_receive_data(
                online,
                asset_id,
                assignment,
                expiration_timestamp as i64,
                transport_endpoints,
                RecipientType::Witness,
                encryption,
                quota.unwrap_or(1),
            )
            .await?;
        let batch_transfer_idx = self.store_receive_transfer(&data, min_confirmations)?;

        self.update_backup_info(false)?;

        info!(self.logger, "Witness receive completed");
        Ok(ReceiveData {
            invoice: data.invoice_string,
            recipient_id: data.recipient_id,
            expiration_timestamp,
            batch_transfer_idx,
            public_key: hex::encode(&data.recipient_pubkey),
        })
    }

    pub(crate) fn _get_new_address(&mut self, keychain: KeychainKind) -> Result<BdkAddress, Error> {
        if self.wallet_data.reuse_addresses {
            let index = self
                .reuse_address_index
                .get(&keychain)
                .copied()
                .unwrap_or(0);
            // reveal_addresses_to ensures BDK's sync engine tracks this address
            let _ = self.bdk_wallet.reveal_addresses_to(keychain, index);
            self.bdk_wallet.persist(&mut self.bdk_database)?;
            let address = self.bdk_wallet.peek_address(keychain, index).address;
            return Ok(address);
        }
        let address = self.bdk_wallet.reveal_next_address(keychain).address;
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        Ok(address)
    }

    pub(crate) fn get_new_address(&mut self) -> Result<BdkAddress, Error> {
        self._get_new_address(KeychainKind::External)
    }

    /// Return a new Bitcoin address from the vanilla wallet.
    pub fn get_address(&mut self) -> Result<String, Error> {
        let cp = self.checkpoint("get_address");
        let res = self._get_address();
        self.commit_queued(cp, res)
    }

    fn _get_address(&mut self) -> Result<String, Error> {
        info!(self.logger, "Getting address...");
        let address = self._get_new_address(KeychainKind::Internal)?;

        if !self.wallet_data.reuse_addresses {
            self.update_backup_info(false)?;
        }

        info!(self.logger, "Get address completed");
        Ok(address.to_string())
    }

    /// Rotate the pinned address for the given keychain.
    ///
    /// Only meaningful when `reuse_addresses` is `true`. Increments the pinned derivation
    /// index so subsequent address generation returns a fresh address.
    pub fn rotate_address(&mut self, keychain: KeychainKind) -> Result<String, Error> {
        let cp = self.checkpoint("rotate_address");
        let res = self._rotate_address(keychain);
        self.commit_queued(cp, res)
    }

    fn _rotate_address(&mut self, keychain: KeychainKind) -> Result<String, Error> {
        if !self.wallet_data.reuse_addresses {
            return Err(Error::AddressReuseDisabled);
        }
        let index = self
            .reuse_address_index
            .get(&keychain)
            .copied()
            .unwrap_or(0);
        let new_index = index + 1;
        self.reuse_address_index.insert(keychain, new_index);
        let _ = self.bdk_wallet.reveal_addresses_to(keychain, new_index);
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        let address = self.bdk_wallet.peek_address(keychain, new_index).address;

        Ok(address.to_string())
    }
}

/// The state an operation on `&self` can change.
pub(crate) struct SharedCheckpoint {
    op: &'static str,
    database: RgbLibDb,
    stock: Option<MemRgbStore>,
}

/// The wallet state an operation can change, restored when the operation fails.
pub(crate) struct Checkpoint {
    shared: SharedCheckpoint,
    bdk_changeset: ChangeSet,
    #[cfg(feature = "esplora")]
    transfer_artifacts: HashMap<String, super::online::TransferArtifacts>,
    #[cfg(feature = "esplora")]
    received_consignments: HashMap<String, Vec<u8>>,
    #[cfg(feature = "esplora")]
    received_consignment_meta: HashMap<String, ReceivedConsignmentMeta>,
    #[cfg(feature = "esplora")]
    received_signed_txs: HashMap<String, Vec<u8>>,
    received_mints: HashMap<String, ReceivedMint>,
    media: HashMap<String, Vec<u8>>,
    pending_mint_locks: HashMap<String, PendingMintLocks>,
    reuse_address_index: HashMap<KeychainKind, u32>,
}

fn idb_key_for(wallet_dir: &Path, bitcoin_network: BitcoinNetwork) -> String {
    wallet_dir
        .join(bitcoin_network.to_string())
        .to_string_lossy()
        .to_string()
}

/// IndexedDB persistence: save wallet state after mutations.
impl Wallet {
    pub(crate) fn update_backup_info(
        &self,
        doing_backup: bool,
    ) -> Result<Option<DbBackupInfo>, Error> {
        use crate::database::memory_db::{ActiveValue, DbBackupInfoActMod};

        let existing = self.database.get_backup_info()?;
        #[cfg(not(target_arch = "wasm32"))]
        let time = now().unix_timestamp_nanos();
        // The browser clock counts milliseconds. Each time comes after the last saved time.
        #[cfg(target_arch = "wasm32")]
        let time = {
            use super::backup::parse_backup_timestamp;
            let last = existing.as_ref().map_or(0, |i| {
                parse_backup_timestamp(&i.last_backup_timestamp)
                    .max(parse_backup_timestamp(&i.last_operation_timestamp))
            });
            (js_sys::Date::now() as i128 * 1_000_000).max(last + 1)
        };
        let timestamp = ActiveValue::Set(time.to_string());

        if existing.is_some() {
            let mut act = DbBackupInfoActMod::default();
            if doing_backup {
                act.last_backup_timestamp = timestamp;
            } else {
                act.last_operation_timestamp = timestamp;
            }
            return Ok(Some(self.database.update_backup_info(&mut act)?));
        }
        let (last_backup_timestamp, last_operation_timestamp) = if doing_backup {
            (timestamp, ActiveValue::Set(s!("0")))
        } else {
            (ActiveValue::Set(s!("0")), timestamp)
        };
        self.database.set_backup_info(DbBackupInfoActMod {
            last_backup_timestamp,
            last_operation_timestamp,
            ..Default::default()
        })?;
        Ok(self.database.get_backup_info()?)
    }

    /// Return the IndexedDB key of this wallet: its directory and its network.
    pub fn idb_key(&self) -> String {
        idb_key_for(&self.wallet_dir, self.wallet_data.bitcoin_network)
    }

    /// Return the error of the last failed IndexedDB save, until a save commits.
    pub fn last_save_error(&self) -> Option<String> {
        self.idb_queue.failed()
    }

    /// Queue a save of the current state and return before the write.
    /// Return an error when an earlier save failed and no save since has committed.
    /// The change stays in memory and its save is queued again.
    pub(crate) fn queue_save(&self) -> Result<(), Error> {
        if cfg!(not(target_arch = "wasm32")) {
            return Ok(());
        }
        let failed = self.idb_queue.failed();
        let snapshot = self.snapshot()?;
        let queue = self.idb_queue.clone();
        let key = self.idb_key();
        let loaded = self.idb_loaded;
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = queue.write(&key, &snapshot, loaded).await {
                web_sys::console::error_1(&format!("IDB save error: {e}").into());
            }
        });
        failed.map_or(Ok(()), |details| Err(Error::Persistence { details }))
    }

    pub(crate) fn snapshot(&self) -> Result<super::idb_store::WalletSnapshot, Error> {
        let stock = self
            .rgb_stock
            .try_borrow()
            .map_err(|_| Error::Persistence {
                details: s!("RGB runtime is active; wallet state cannot be snapshotted"),
            })?;
        let stock = stock.as_ref().ok_or_else(|| Error::Persistence {
            details: s!("RGB runtime is active; wallet state cannot be snapshotted"),
        })?;
        let rgb_store =
            super::idb_store::serialize_stock(stock).ok_or_else(|| Error::Persistence {
                details: s!("failed to serialize RGB stock"),
            })?;

        Ok(super::idb_store::WalletSnapshot {
            writer: self.idb_writer,
            sequence: self
                .idb_sequence
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1,
            db: self.database.as_ref().clone(),
            bdk_changeset: self.bdk_database.get_data().clone(),
            transfer_artifacts: self.transfer_artifacts.clone(),
            received_consignments: self.received_consignments.clone(),
            received_consignment_meta: self.received_consignment_meta.clone(),
            received_signed_txs: self.received_signed_txs.clone(),
            received_mints: self.received_mints.clone(),
            media: self.media.clone(),
            pending_mint_locks: self.pending_mint_locks.clone(),
            rgb_store,
            reuse_address_index: self.reuse_address_index.clone(),
            manifest: WalletManifest::new(&self.wallet_data, &self.keys),
            backup_versions: self.backup_versions.borrow().clone(),
        })
    }

    /// Durably persist the current wallet state to IndexedDB.
    ///
    /// This method resolves only after the IndexedDB read-write transaction commits. Callers must
    /// not persist dependent application state until this method succeeds. A failure leaves the
    /// current in-memory wallet state intact and may be retried.
    /// It fails when IndexedDB is not written.
    /// Resolve when this save or a newer one has committed.
    pub async fn flush(&self) -> Result<(), Error> {
        if cfg!(not(target_arch = "wasm32")) {
            return Ok(());
        }
        let snapshot = self.snapshot()?;
        self.idb_queue
            .write(&self.idb_key(), &snapshot, self.idb_loaded)
            .await
            .map_err(|details| Error::Persistence { details })
    }

    /// Restore wallet state from an IndexedDB snapshot.
    /// It refuses a snapshot of other fixed settings with [`Error::WalletSettingMismatch`].
    /// It changes nothing when it fails.
    #[allow(clippy::arc_with_non_send_sync)] // wasm32 is single-threaded; Arc matches native API
    pub fn restore_from_snapshot(
        &mut self,
        snapshot: super::idb_store::WalletSnapshot,
    ) -> Result<(), Error> {
        snapshot
            .manifest
            .check_settings_unchanged(&self.wallet_data, &self.keys)?;
        let stock = super::idb_store::deserialize_stock(&snapshot.rgb_store)
            .map_err(|details| Error::Internal { details })?;

        // Load the BDK wallet from the changeset into a new store
        if let Some(changeset) = snapshot.bdk_changeset {
            let (bdk_database, bdk_wallet) = self.load_bdk(changeset)?;
            // All parts are valid. Nothing below can fail.
            self.bdk_database = bdk_database;
            self.bdk_wallet = bdk_wallet;
        }
        self.database = Arc::new(snapshot.db);

        // Restore complete pending transfers so send_end can resume after a page reload.
        self.transfer_artifacts = snapshot.transfer_artifacts;

        // Restore received consignments
        self.received_consignments = snapshot.received_consignments;
        self.received_consignment_meta = snapshot.received_consignment_meta;
        self.received_signed_txs = snapshot.received_signed_txs;
        self.received_mints = snapshot.received_mints;
        self.media = snapshot.media;
        self.pending_mint_locks = snapshot.pending_mint_locks;

        // Restore address reuse pinned indices
        self.reuse_address_index = snapshot.reuse_address_index;

        *self.rgb_stock.borrow_mut() = Some(stock);
        *self.backup_versions.borrow_mut() = snapshot.backup_versions;

        Ok(())
    }

    /// Load a BDK wallet from `changeset` into a new store.
    fn load_bdk(
        &self,
        changeset: ChangeSet,
    ) -> Result<(BdkPersister, PersistedWallet<BdkPersister>), Error> {
        let mut bdk_database: BdkPersister = super::memory_store::MemoryStore::new();
        bdk_database.set_data(Some(changeset));

        let wdata = &self.wallet_data;
        let WalletDescriptors {
            colored: desc_colored,
            vanilla: desc_vanilla,
        } = self.keys.build_descriptors(wdata.bitcoin_network)?.0;

        let chain_net: ChainNet = wdata.bitcoin_network.into();
        let wallet_params = BdkWallet::load()
            .descriptor(KeychainKind::External, Some(desc_colored))
            .descriptor(KeychainKind::Internal, Some(desc_vanilla))
            .check_genesis_hash(BlockHash::from_byte_array(
                chain_net.chain_hash().to_bytes(),
            ));

        let bdk_wallet = match wallet_params.load_wallet(&mut bdk_database) {
            Ok(Some(wallet)) => wallet,
            Ok(None) => {
                return Err(Error::Persistence {
                    details: s!("persisted BDK changeset did not contain a loadable wallet"),
                });
            }
            Err(error) => {
                return Err(Error::Persistence {
                    details: format!("failed to load persisted BDK wallet: {error}"),
                });
            }
        };
        Ok((bdk_database, bdk_wallet))
    }

    /// Return the BDK changeset with the staged changes.
    pub(crate) fn bdk_changeset(&self) -> ChangeSet {
        let mut changeset = self.bdk_database.get_data().clone().unwrap_or_default();
        if let Some(staged) = self.bdk_wallet.staged() {
            changeset.merge(staged.clone());
        }
        changeset
    }

    pub(crate) fn shared_checkpoint(&self, op: &'static str) -> SharedCheckpoint {
        SharedCheckpoint {
            op,
            database: (*self.database).clone(),
            stock: self
                .rgb_stock
                .borrow()
                .as_ref()
                .map(|s| s.as_store().clone()),
        }
    }

    pub(crate) fn checkpoint(&mut self, op: &'static str) -> Checkpoint {
        #[cfg(feature = "esplora")]
        self.broadcast_txs.clear();
        Checkpoint {
            shared: self.shared_checkpoint(op),
            bdk_changeset: self.bdk_changeset(),
            #[cfg(feature = "esplora")]
            transfer_artifacts: self.transfer_artifacts.clone(),
            #[cfg(feature = "esplora")]
            received_consignments: self.received_consignments.clone(),
            #[cfg(feature = "esplora")]
            received_consignment_meta: self.received_consignment_meta.clone(),
            #[cfg(feature = "esplora")]
            received_signed_txs: self.received_signed_txs.clone(),
            received_mints: self.received_mints.clone(),
            media: self.media.clone(),
            pending_mint_locks: self.pending_mint_locks.clone(),
            reuse_address_index: self.reuse_address_index.clone(),
        }
    }

    pub(crate) fn rollback_shared(&self, cp: SharedCheckpoint) {
        self.database.restore(cp.database);
        *self.rgb_stock.borrow_mut() = cp.stock.map(Stock::with);
    }

    fn rollback(&mut self, cp: Checkpoint) {
        let op = cp.shared.op;
        self.rollback_shared(cp.shared);
        #[cfg(feature = "esplora")]
        {
            self.transfer_artifacts = cp.transfer_artifacts;
            self.received_consignments = cp.received_consignments;
            self.received_consignment_meta = cp.received_consignment_meta;
            self.received_signed_txs = cp.received_signed_txs;
        }
        self.received_mints = cp.received_mints;
        self.media = cp.media;
        self.pending_mint_locks = cp.pending_mint_locks;
        self.reuse_address_index = cp.reuse_address_index;
        #[cfg(feature = "esplora")]
        if !self.broadcast_txs.is_empty() {
            // a broadcast TX stays in BDK, as its change can pay an address revealed here
            let last_seen = (now().unix_timestamp() as u64).saturating_add(1);
            let txs = self.broadcast_txs.iter().cloned().map(|tx| (tx, last_seen));
            self.bdk_wallet.apply_unconfirmed_txs(txs);
            if let Err(e) = self.bdk_wallet.persist(&mut self.bdk_database) {
                error!(self.logger, "Cannot keep the TXs broadcast in {op}: {e}");
            }
            return;
        }
        if self.bdk_changeset() != cp.bdk_changeset {
            match self.load_bdk(cp.bdk_changeset) {
                Ok((bdk_database, bdk_wallet)) => {
                    self.bdk_database = bdk_database;
                    self.bdk_wallet = bdk_wallet;
                }
                Err(e) => error!(self.logger, "Cannot restore BDK after {op}: {e}"),
            }
        }
    }

    /// End an operation: save its result, or restore `cp` when it fails with an error not kept.
    pub(crate) async fn commit<T>(
        &mut self,
        cp: Checkpoint,
        res: Result<T, Error>,
        keep: fn(&Error) -> bool,
    ) -> Result<T, Error> {
        let res = res.and_then(|v| fault_point(cp.shared.op).map(|()| v));
        let res = match res {
            Err(e) if keep(&e) => Err(e),
            _ if crate::abort::fired() => Err(Error::Aborted),
            res => res,
        };
        match res {
            Err(e) if !keep(&e) => {
                self.rollback(cp);
                Err(e)
            }
            res => {
                self.flush().await?;
                res
            }
        }
    }

    /// End an operation as [`commit`](Wallet::commit) does, with a queued save.
    pub(crate) fn commit_queued<T>(
        &mut self,
        cp: Checkpoint,
        res: Result<T, Error>,
    ) -> Result<T, Error> {
        let res = res.and_then(|v| fault_point(cp.shared.op).map(|()| v));
        if res.is_err() {
            self.rollback(cp);
            return res;
        }
        self.queue_save()?;
        res
    }

    /// End an operation on `&self` as [`commit_queued`](Wallet::commit_queued) does.
    pub(crate) fn commit_shared<T>(
        &self,
        cp: SharedCheckpoint,
        res: Result<T, Error>,
    ) -> Result<T, Error> {
        let res = res.and_then(|v| fault_point(cp.op).map(|()| v));
        if res.is_err() {
            self.rollback_shared(cp);
            return res;
        }
        self.queue_save()?;
        res
    }
}

#[cfg(test)]
pub(crate) mod address_reuse_tests {
    use super::*;
    use crate::keys::generate_keys;

    pub(crate) fn pending_mint_locks() -> PendingMintLocks {
        PendingMintLocks {
            consignment_digest: "digest".to_owned(),
            evm_chain_id: 3,
            bridge_contract: "0x1111111111111111111111111111111111111111"
                .parse::<EvmAddress>()
                .unwrap(),
            claims: vec![Claim::Lock {
                opid: OpId::from([1u8; 32]),
                witness_id: "22".repeat(32).parse::<RgbTxid>().unwrap(),
                amount: 1,
                before_block: 2,
            }],
        }
    }

    /// Helper: build a minimal Wallet for native tests.
    ///
    /// Bypasses `Wallet::new` (which requires RGB schema import / js-sys)
    /// by constructing only the BDK wallet and the fields that
    /// `_get_new_address` touches.
    pub(crate) fn make_test_wallet(reuse: bool) -> Wallet {
        make_test_wallet_with_keys(
            generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot),
            reuse,
        )
    }

    pub(crate) fn make_test_wallet_with_keys(keys: crate::keys::Keys, reuse: bool) -> Wallet {
        make_test_wallet_with_singlesig_keys(SinglesigKeys::from_keys(&keys, None), reuse)
    }

    pub(crate) fn make_test_wallet_with_singlesig_keys(keys: SinglesigKeys, reuse: bool) -> Wallet {
        make_test_wallet_on_network(keys, reuse, BitcoinNetwork::Regtest)
    }

    pub(crate) fn make_test_wallet_on_network(
        keys: SinglesigKeys,
        reuse: bool,
        bitcoin_network: BitcoinNetwork,
    ) -> Wallet {
        let bdk_network = BdkNetwork::from(bitcoin_network);
        let WalletDescriptors {
            colored: desc_colored,
            vanilla: desc_vanilla,
        } = keys.build_descriptors(bitcoin_network).unwrap().0;
        let mut bdk_database: BdkPersister = super::super::memory_store::MemoryStore::new();
        let bdk_wallet = BdkWallet::create(desc_colored.clone(), desc_vanilla.clone())
            .network(bdk_network)
            .create_wallet(&mut bdk_database)
            .unwrap();
        let signers = build_signers(
            &bdk_wallet,
            &desc_colored,
            &desc_vanilla,
            bdk_network.into(),
        )
        .unwrap();

        let wd = WalletData {
            data_dir: ":memory:".to_string(),
            bitcoin_network,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            supported_schemas: vec![AssetSchema::Nia],
            reuse_addresses: reuse,
            allow_reckless_schemas: false,
        };

        let drain = slog::Discard;
        let logger = Logger::root(drain, o!());

        Wallet {
            wallet_data: wd,
            keys,
            logger,
            _logger_guard: (),
            watch_only: false,
            database: Arc::new(crate::database::memory_db::InMemoryDb::new()),
            wallet_dir: PathBuf::from(":memory:"),
            bdk_wallet,
            signers,
            bdk_database,
            reuse_address_index: HashMap::new(),
            #[cfg(feature = "esplora")]
            online_data: None,
            #[cfg(feature = "esplora")]
            transfer_artifacts: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignments: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_valid_consignments: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_consignment_meta: HashMap::new(),
            #[cfg(feature = "esplora")]
            received_signed_txs: HashMap::new(),
            received_mints: HashMap::new(),
            media: HashMap::new(),
            pending_mint_locks: HashMap::new(),
            #[cfg(feature = "esplora")]
            broadcast_txs: Vec::new(),
            idb_queue: Default::default(),
            idb_sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            idb_writer: 0,
            idb_loaded: None,
            rgb_stock: std::rc::Rc::new(std::cell::RefCell::new(None)),
            backup_store: None,
            backup_versions: std::cell::RefCell::new(HashMap::new()),
            last_vss_backup_error: std::cell::RefCell::new(None),
        }
    }

    /// Add three unspent TXOs, so that the wallet can issue with no indexer.
    pub(crate) fn add_fake_txos(wallet: &Wallet) {
        for vout in 0..3 {
            wallet
                .database
                .set_txo(DbTxoActMod {
                    txid: ActiveValue::Set("11".repeat(32)),
                    vout: ActiveValue::Set(vout),
                    btc_amount: ActiveValue::Set(s!("1000")),
                    spent: ActiveValue::Set(false),
                    exists: ActiveValue::Set(true),
                    pending_witness: ActiveValue::Set(false),
                    ..Default::default()
                })
                .unwrap();
        }
    }

    pub(crate) fn media_attachment(data: &[u8]) -> Attachment {
        Attachment {
            ty: MediaType::with("text/plain"),
            digest: Bytes32::from_byte_array(sha256_digest(data)),
        }
    }

    /// Set the media of the next asset issued. Return its digest.
    pub(crate) fn set_media(wallet: &mut Wallet, data: &[u8]) -> String {
        MOCK_ISSUE_MEDIA.set(Some(media_attachment(data)));
        let digest = hex::encode(sha256_digest(data));
        wallet.media.insert(digest.clone(), data.to_vec());
        digest
    }

    /// Return all the state an operation can change.
    pub(crate) fn state(wallet: &Wallet) -> serde_json::Value {
        serde_json::json!({
            "db": &*wallet.database,
            "bdk": wallet.bdk_changeset(),
            "stock": wallet
                .rgb_stock
                .borrow()
                .as_ref()
                .map(super::super::idb_store::serialize_stock),
            "transfer_artifacts": wallet.transfer_artifacts,
            "received_consignments": wallet.received_consignments,
            "received_consignment_meta": wallet.received_consignment_meta,
            "received_signed_txs": wallet.received_signed_txs,
            "received_mints": wallet.received_mints,
            "media": wallet.media,
            "pending_mint_locks": wallet.pending_mint_locks,
            "reuse_address_index": wallet.reuse_address_index,
        })
    }

    /// Fail `call` at `fault` and check that it changed nothing but BDK for its broadcasts.
    /// Return the txids it broadcast.
    pub(crate) async fn assert_no_partial_change<T>(
        wallet: &mut Wallet,
        fault: &'static str,
        broadcasts: usize,
        call: impl AsyncFnOnce(&mut Wallet) -> Result<T, Error>,
    ) -> Vec<String> {
        let mut before = state(wallet);
        crate::utils::FAULT.with(|f| f.set(Some(fault)));
        let res = call(wallet).await;
        crate::utils::FAULT.with(|f| f.set(None));
        assert_eq!(
            res.err(),
            Some(Error::Internal {
                details: format!("fault injected in {fault}")
            })
        );
        assert_eq!(wallet.broadcast_txs.len(), broadcasts);
        let mut after = state(wallet);
        if broadcasts > 0 {
            before["bdk"].take();
            after["bdk"].take();
            for tx in &wallet.broadcast_txs {
                assert!(wallet.bdk_wallet.get_tx(tx.compute_txid()).is_some());
            }
        }
        assert_eq!(after, before);
        wallet
            .broadcast_txs
            .iter()
            .map(|tx| tx.compute_txid().to_string())
            .collect()
    }

    #[test]
    fn get_address_is_atomic() {
        let mut wallet = make_test_wallet(false);
        block_on(assert_no_partial_change(
            &mut wallet,
            "get_address",
            0,
            async |w| w.get_address(),
        ));
    }

    #[test]
    fn rotate_address_is_atomic() {
        let mut wallet = make_test_wallet(true);
        block_on(assert_no_partial_change(
            &mut wallet,
            "rotate_address",
            0,
            async |w| w.rotate_address(KeychainKind::Internal),
        ));
    }

    #[test]
    fn reuse_returns_same_address() {
        let mut wallet = make_test_wallet(true);
        let addr1 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let addr2 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_eq!(addr1, addr2, "reuse enabled: same address expected");
    }

    #[test]
    fn no_reuse_returns_different_addresses() {
        let mut wallet = make_test_wallet(false);
        let addr1 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let addr2 = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_ne!(addr1, addr2, "reuse disabled: different addresses expected");
    }

    #[test]
    fn reuse_index_changes_address() {
        let mut wallet = make_test_wallet(true);
        let old_addr = wallet._get_new_address(KeychainKind::Internal).unwrap();

        // Simulate what rotate_address does internally
        wallet.reuse_address_index.insert(KeychainKind::Internal, 1);
        let new_addr = wallet._get_new_address(KeychainKind::Internal).unwrap();

        assert_ne!(
            old_addr, new_addr,
            "bumping index should change the pinned address"
        );
    }

    #[test]
    fn keychains_have_independent_indices() {
        let mut wallet = make_test_wallet(true);
        let internal = wallet._get_new_address(KeychainKind::Internal).unwrap();
        let external = wallet._get_new_address(KeychainKind::External).unwrap();
        assert_ne!(
            internal.to_string(),
            external.to_string(),
            "different keychains"
        );

        // Bump External index — should not affect Internal
        wallet.reuse_address_index.insert(KeychainKind::External, 1);
        let internal_after = wallet._get_new_address(KeychainKind::Internal).unwrap();
        assert_eq!(
            internal.to_string(),
            internal_after.to_string(),
            "Internal unchanged after External index bump"
        );
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use crate::wallet::offline::address_reuse_tests::make_test_wallet;

    #[test]
    fn debug_hides_wallet_mnemonic() {
        let keys = make_test_wallet(false).get_keys();
        let phrase = keys.mnemonic.clone().unwrap();
        assert!(!format!("{keys:?}").contains(phrase.as_str()));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod mainnet_schema_tests {
    use super::*;
    use crate::keys::generate_keys;
    use AssetSchema::{Bfa, Ifa, Nia};

    fn mainnet_schema_wallet_data(
        network: BitcoinNetwork,
        schemas: Vec<AssetSchema>,
        reckless: bool,
    ) -> (WalletData, SinglesigKeys) {
        let keys = generate_keys(network, WitnessVersion::Taproot);
        let wd = WalletData {
            data_dir: s!(":memory:"),
            bitcoin_network: network,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            supported_schemas: schemas,
            reuse_addresses: false,
            allow_reckless_schemas: reckless,
        };
        (wd, SinglesigKeys::from_keys(&keys, None))
    }

    #[test]
    fn new_refuses_zero_max_allocations_per_utxo() {
        let (mut wd, keys) = mainnet_schema_wallet_data(BitcoinNetwork::Regtest, vec![Nia], false);
        wd.max_allocations_per_utxo = 0;
        assert!(matches!(
            Wallet::new(wd, keys),
            Err(Error::NoMaxAllocationsPerUtxo)
        ));
    }

    #[test]
    fn mainnet_schema_refuses_ifa_and_bfa() {
        for expected in [Ifa, Bfa] {
            let (wd, keys) =
                mainnet_schema_wallet_data(BitcoinNetwork::Mainnet, vec![Nia, expected], false);
            assert!(matches!(
                Wallet::new(wd, keys),
                Err(Error::CannotUseSchemaOnMainnet { asset_schema }) if asset_schema == expected
            ));
        }
        let (wd, keys) = mainnet_schema_wallet_data(BitcoinNetwork::Mainnet, vec![Nia], false);
        assert!(Wallet::new(wd, keys).is_ok());
    }

    #[test]
    fn mainnet_schema_reckless_allowed() {
        let (wd, keys) =
            mainnet_schema_wallet_data(BitcoinNetwork::Mainnet, vec![Nia, Ifa, Bfa], true);
        let wallet = Wallet::new(wd, keys).unwrap();
        assert!(wallet.get_wallet_data().allow_reckless_schemas);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod keys_and_settings_tests {
    use super::*;
    use crate::keys::{Keys, WitnessVersion, generate_keys, restore_keys};

    const NET: BitcoinNetwork = BitcoinNetwork::Regtest;

    fn wallet_data() -> WalletData {
        WalletData {
            data_dir: s!(":memory:"),
            bitcoin_network: NET,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            supported_schemas: vec![AssetSchema::Nia],
            reuse_addresses: false,
            allow_reckless_schemas: false,
        }
    }

    fn new_err(keys: SinglesigKeys) -> Error {
        Wallet::new(wallet_data(), keys).err().unwrap()
    }

    // Return the expected and provided values of a mismatch of `setting`.
    fn mismatch(res: Result<(), Error>, setting: &str) -> (String, String) {
        match res {
            Err(Error::WalletSettingMismatch {
                setting: s,
                expected,
                provided,
            }) if s == setting => (expected, provided),
            other => panic!("expected a {setting} mismatch, got {other:?}"),
        }
    }

    #[test]
    fn get_descriptors_success() {
        let keys = generate_keys(NET, WitnessVersion::Taproot);
        let signing = SinglesigKeys::from_keys(&keys, None);
        let wallet = Wallet::new(wallet_data(), signing.clone()).unwrap();
        let descs = wallet.get_descriptors();
        assert_eq!(signing.build_descriptors(NET).unwrap().0, descs);
        assert!(descs.colored.contains("tprv") && descs.vanilla.contains("tprv"));

        let watch_only = SinglesigKeys::from_keys_no_mnemonic(&keys, None);
        let wo_descs = Wallet::new(wallet_data(), watch_only)
            .unwrap()
            .get_descriptors();
        for desc in [&wo_descs.colored, &wo_descs.vanilla] {
            assert!(desc.contains("tpub") && !desc.contains("tprv"), "{desc}");
        }
        assert_ne!(wo_descs, descs);
    }

    #[test]
    fn keys_need_witness_version() {
        let field = if cfg!(feature = "camel_case") {
            "witnessVersion"
        } else {
            "witness_version"
        };
        let missing = format!("missing field `{field}`");
        let keys = generate_keys(NET, WitnessVersion::SegWitV0);

        let mut v = serde_json::to_value(&keys).unwrap();
        v.as_object_mut().unwrap().remove(field).unwrap();
        let err = serde_json::from_value::<Keys>(v).unwrap_err();
        assert_eq!(err.to_string(), missing);

        let mut v = serde_json::to_value(SinglesigKeys::from_keys(&keys, None)).unwrap();
        v.as_object_mut().unwrap().remove(field).unwrap();
        let err = serde_json::from_value::<SinglesigKeys>(v).unwrap_err();
        assert_eq!(err.to_string(), missing);
    }

    #[test]
    fn segwit_v0_wallet_success() {
        let keys = generate_keys(NET, WitnessVersion::SegWitV0);
        let mut addresses = vec![];
        for sk in [
            SinglesigKeys::from_keys(&keys, None),
            SinglesigKeys::from_keys_no_mnemonic(&keys, None),
        ] {
            let mut wallet = Wallet::new(wallet_data(), sk).unwrap();
            for (keychain, path) in [
                (KeychainKind::External, "m/84'/827167'/0'/0"),
                (KeychainKind::Internal, "m/84'/1'/0'/0"),
            ] {
                let bdk_wallet::miniscript::Descriptor::Wpkh(w) =
                    wallet.bdk_wallet.public_descriptor(keychain)
                else {
                    panic!("expected a wpkh descriptor");
                };
                assert_eq!(
                    w.as_inner().full_derivation_path().unwrap(),
                    DerivationPath::from_str(path).unwrap()
                );
            }
            addresses.push(wallet.get_address().unwrap());
        }
        assert_eq!(addresses[0], addresses[1]);
        let address = BdkAddress::from_str(&addresses[0]).unwrap();
        assert!(address.assume_checked().script_pubkey().is_p2wpkh());
    }

    #[test]
    fn new_fail() {
        let keys = generate_keys(NET, WitnessVersion::Taproot);
        let other = generate_keys(NET, WitnessVersion::Taproot);
        let signing = SinglesigKeys::from_keys(&keys, None);

        // invalid colored xPub
        for xpub in ["", "l1iI0"] {
            let err = new_err(SinglesigKeys {
                account_xpub_colored: xpub.to_string(),
                ..signing.clone()
            });
            assert!(matches!(err, Error::InvalidPubkey { .. }), "{err:?}");
        }

        // invalid mnemonic
        let err = new_err(SinglesigKeys {
            mnemonic: Some(Zeroizing::new(s!(""))),
            ..signing.clone()
        });
        assert!(matches!(err, Error::InvalidMnemonic { .. }), "{err:?}");

        // xPub not derived from the mnemonic
        let err = new_err(SinglesigKeys {
            account_xpub_colored: other.xpub.clone(),
            ..signing.clone()
        });
        assert!(matches!(err, Error::InvalidBitcoinKeys), "{err:?}");

        // Taproot xPubs with the SegWitV0 witness version
        let err = new_err(SinglesigKeys {
            witness_version: WitnessVersion::SegWitV0,
            ..signing.clone()
        });
        assert!(matches!(err, Error::InvalidBitcoinKeys), "{err:?}");

        // invalid fingerprint, signing and watch-only
        for keys in [
            signing.clone(),
            SinglesigKeys::from_keys_no_mnemonic(&keys, None),
        ] {
            let err = new_err(SinglesigKeys {
                master_fingerprint: s!("invalid"),
                ..keys
            });
            assert!(matches!(err, Error::InvalidFingerprint), "{err:?}");
        }

        // fingerprint not derived from the mnemonic
        let err = new_err(SinglesigKeys {
            master_fingerprint: s!("badbadff"),
            ..signing.clone()
        });
        assert!(matches!(err, Error::FingerprintMismatch), "{err:?}");

        // mnemonic of another wallet: the keys are checked before the fingerprint
        let err = new_err(SinglesigKeys {
            mnemonic: Some(other.mnemonic.clone()),
            ..signing
        });
        assert!(matches!(err, Error::InvalidBitcoinKeys), "{err:?}");
    }

    #[test]
    fn settings_check_refuses_fixed_settings() {
        let wd = wallet_data();
        let keys = generate_keys(NET, WitnessVersion::Taproot);
        let sk = SinglesigKeys::from_keys(&keys, None);
        let manifest = WalletManifest::new(&wd, &sk);
        let check = |wd: &WalletData, sk: &SinglesigKeys| manifest.check_settings_unchanged(wd, sk);

        let testnet = WalletData {
            bitcoin_network: BitcoinNetwork::Testnet,
            ..wallet_data()
        };
        assert!(matches!(
            check(&testnet, &sk),
            Err(Error::BitcoinNetworkMismatch)
        ));

        // all keys change: the fingerprint is reported first
        let other = SinglesigKeys::from_keys_no_mnemonic(
            &generate_keys(NET, WitnessVersion::Taproot),
            None,
        );
        mismatch(check(&wd, &other), "master_fingerprint");

        // other witness version: it is reported before the xPubs
        let segwit = SinglesigKeys::from_keys(
            &restore_keys(NET, keys.mnemonic.to_string(), WitnessVersion::SegWitV0).unwrap(),
            None,
        );
        let (expected, provided) = mismatch(check(&wd, &segwit), "witness_version");
        assert_eq!(
            (expected.as_str(), provided.as_str()),
            ("Taproot", "SegWitV0")
        );

        let keychain = SinglesigKeys::from_keys(&keys, Some(2));
        let (expected, provided) = mismatch(check(&wd, &keychain), "vanilla_keychain");
        assert_eq!((expected.as_str(), provided.as_str()), ("0", "2"));

        let colored = SinglesigKeys {
            account_xpub_colored: other.account_xpub_colored.clone(),
            ..sk.clone()
        };
        mismatch(check(&wd, &colored), "account_xpub_colored");

        let vanilla = SinglesigKeys {
            account_xpub_vanilla: other.account_xpub_vanilla,
            ..sk
        };
        mismatch(check(&wd, &vanilla), "account_xpub_vanilla");
    }

    #[test]
    fn settings_check_allows_mutable_settings() {
        let wd = wallet_data();
        let keys = generate_keys(NET, WitnessVersion::Taproot);
        let manifest = WalletManifest::new(&wd, &SinglesigKeys::from_keys(&keys, None));

        manifest
            .check_settings_unchanged(&wd, &SinglesigKeys::from_keys(&keys, Some(0)))
            .unwrap();
        manifest
            .check_settings_unchanged(&wd, &SinglesigKeys::from_keys_no_mnemonic(&keys, None))
            .unwrap();
        let changed = WalletData {
            data_dir: s!("other"),
            max_allocations_per_utxo: 1,
            supported_schemas: vec![AssetSchema::Nia, AssetSchema::Ifa],
            reuse_addresses: true,
            allow_reckless_schemas: true,
            ..wallet_data()
        };
        manifest
            .check_settings_unchanged(&changed, &SinglesigKeys::from_keys(&keys, None))
            .unwrap();
    }

    #[test]
    fn manifest_round_trip() {
        let keys = generate_keys(NET, WitnessVersion::SegWitV0);
        let wd = WalletData {
            max_allocations_per_utxo: 1,
            supported_schemas: vec![AssetSchema::Nia, AssetSchema::Ifa],
            reuse_addresses: true,
            allow_reckless_schemas: true,
            ..wallet_data()
        };
        let sk = SinglesigKeys::from_keys(&keys, Some(2));
        let json = serde_json::to_string(&WalletManifest::new(&wd, &sk)).unwrap();
        assert!(!json.contains(keys.mnemonic.as_str()));

        let manifest: WalletManifest = serde_json::from_str(&json).unwrap();
        let (got_wd, got_sk) = manifest.into_parts(s!("dir"), Some(keys.mnemonic.clone()));
        let expected_wd = WalletData {
            data_dir: s!("dir"),
            ..wd.clone()
        };
        assert_eq!(
            serde_json::to_value(&got_wd).unwrap(),
            serde_json::to_value(&expected_wd).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&got_sk).unwrap(),
            serde_json::to_value(&sk).unwrap()
        );

        let (_, got_sk) = WalletManifest::new(&wd, &SinglesigKeys::from_keys(&keys, None))
            .into_parts(s!("dir"), None);
        assert_eq!(got_sk.vanilla_keychain, Some(0));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod receive_expiration_tests {
    use super::address_reuse_tests::{add_fake_txos, make_test_wallet};
    use super::*;

    const INDEXER_URL: &str = "http://127.0.0.1:1";

    fn online(wallet: &mut Wallet) -> Online {
        wallet.online_data = Some(OnlineData::for_test(INDEXER_URL));
        Online {
            id: 1,
            indexer_url: INDEXER_URL.to_string(),
        }
    }

    // A proxy that answers `server.info` with `grace` and `invoice.prepare` for `quota`.
    fn proxy_with_quota(quota: u32, grace: u32) -> (mockito::ServerGuard, Vec<mockito::Mock>) {
        let mut server = mockito::Server::new();
        let info = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(s!("server.info")))
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"jsonrpc":"2.0","id":null,"result":{{"protocol_version":"1.0","version":"0.4.0","uptime":1,"retention_grace_secs":"{grace}"}}}}"#
            ))
            .create();
        let prepare = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex(s!("invoice.prepare")),
                mockito::Matcher::Regex(format!(r#""quota":"{quota}""#)),
            ]))
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":null,"result":true}"#)
            .create();
        (server, vec![info, prepare])
    }

    #[test]
    fn receive_past_expiration_fails() {
        let mut wallet = make_test_wallet(false);
        let online = online(&mut wallet);
        let result = crate::block_on(wallet.witness_receive(
            Some(online),
            None,
            Assignment::Any,
            now().unix_timestamp() as u64 - 1,
            vec![s!("rpc://127.0.0.1")],
            1,
            None,
            None,
        ));
        assert!(matches!(result, Err(Error::InvalidExpiration)));
        let db_data = wallet.database.get_db_data(false).unwrap();
        assert!(db_data.batch_transfers.is_empty());
    }

    #[test]
    fn receive_needs_online() {
        let mut wallet = make_test_wallet(false);
        let exp = now().unix_timestamp() as u64 + 100_000;
        for result in [
            crate::block_on(wallet.witness_receive(
                None,
                None,
                Assignment::Any,
                exp,
                vec![s!("rpc://127.0.0.1")],
                1,
                None,
                None,
            )),
            crate::block_on(wallet.blind_receive(
                None,
                None,
                Assignment::Any,
                exp,
                vec![s!("rpc://127.0.0.1")],
                1,
                None,
                None,
            )),
        ] {
            assert!(matches!(result, Err(Error::OnlineNeeded)));
        }
        let db_data = wallet.database.get_db_data(false).unwrap();
        assert!(db_data.batch_transfers.is_empty());
    }

    #[test]
    fn receive_keeps_given_expiration_and_stores_v1_data() {
        let mut wallet = make_test_wallet(false);
        let online = online(&mut wallet);
        let (server, _mocks) = proxy_with_quota(1, 100);
        let exp = now().unix_timestamp() + 100_000;
        let data = crate::block_on(wallet.create_receive_data(
            Some(online),
            None,
            Assignment::Any,
            exp,
            vec![format!("rpc://{}", server.host_with_port())],
            RecipientType::Witness,
            Some(Encryption::Optional),
            1,
        ))
        .unwrap();
        wallet.store_receive_transfer(&data, 1).unwrap();
        let public_key = hex::encode(&data.recipient_pubkey);
        let db_data = wallet.database.get_db_data(false).unwrap();
        assert_eq!(db_data.batch_transfers.len(), 1);
        let batch = &db_data.batch_transfers[0];
        assert_eq!(batch.expiration, Some(exp));
        assert!(batch.operation_secret_key.is_some());
        // the retention deadline is only for a blind receive
        assert_eq!(batch.retention_deadline, None);
        let transfer = &db_data.transfers[0];
        assert_eq!(transfer.invoice_idx, None);
        assert_eq!(transfer.receive_dir, None);
        assert!(wallet.database.iter_invoices().unwrap().is_empty());
        assert_eq!(
            hex::encode(transfer.recipient_pubkey.as_ref().unwrap()),
            public_key
        );
        assert_eq!(transfer.recipient_encryption, Encryption::Optional);
        let invoice_data = Invoice::new(format!("{}&foo=bar&baz=qux", data.invoice_string))
            .unwrap()
            .invoice_data();
        assert_eq!(invoice_data.expiration_timestamp, u64::try_from(exp).ok());
        assert_eq!(invoice_data.public_key, Some(public_key));
        assert_eq!(invoice_data.encryption, Encryption::Optional);
        assert_eq!(
            invoice_data.unknown_query_params,
            HashMap::from([
                ("foo".to_string(), "bar".to_string()),
                ("baz".to_string(), "qux".to_string()),
            ])
        );
    }

    #[test]
    fn reuse_fail() {
        let mut wallet = make_test_wallet(false);
        let online = online(&mut wallet);
        let exp = now().unix_timestamp() as u64 + 100_000;
        let endpoints = vec![s!("rpc://127.0.0.1")];
        let max = wallet.max_utxo_allocations();
        let receive = |wallet: &mut Wallet, witness: bool, endpoints: Vec<String>, quota| {
            let online = Some(online.clone());
            let res = crate::block_on(async {
                if witness {
                    let rcv = wallet.witness_receive(
                        online,
                        None,
                        Assignment::Any,
                        exp,
                        endpoints,
                        1,
                        None,
                        quota,
                    );
                    rcv.await
                } else {
                    let rcv = wallet.blind_receive(
                        online,
                        None,
                        Assignment::Any,
                        exp,
                        endpoints,
                        1,
                        None,
                        quota,
                    );
                    rcv.await
                }
            });
            match res {
                Err(Error::InvalidQuota { details }) => details,
                other => panic!("unexpected {other:?}"),
            }
        };
        for witness in [false, true] {
            assert_eq!(
                receive(&mut wallet, witness, endpoints.clone(), Some(0)),
                "must be at least 1"
            );
            assert_eq!(
                receive(&mut wallet, witness, vec![], Some(2)),
                "a reusable invoice requires transport endpoints"
            );
        }
        assert_eq!(
            receive(&mut wallet, false, endpoints.clone(), Some(max + 1)),
            format!("a blind invoice can receive at most {max} transfers")
        );
        MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(2));
        let details = receive(&mut wallet, false, endpoints, Some(3));
        MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(None);
        assert_eq!(details, "a blind invoice can receive at most 2 transfers");
        assert!(wallet.database.iter_batch_transfers().unwrap().is_empty());
    }

    #[test]
    fn reusable_receive_stores_invoice() {
        let mut wallet = make_test_wallet(false);
        let online = online(&mut wallet);
        let (server, mocks) = proxy_with_quota(3, 100);
        let (long_server, long_mocks) = proxy_with_quota(3, 300);
        let exp = now().unix_timestamp() + 100_000;
        let data = crate::block_on(wallet.create_receive_data(
            Some(online.clone()),
            None,
            Assignment::Any,
            exp,
            vec![
                format!("rpc://{}", server.host_with_port()),
                format!("rpc://{}", long_server.host_with_port()),
            ],
            RecipientType::Witness,
            None,
            3,
        ))
        .unwrap();
        long_mocks[1].assert();
        // The longest proxy retention grace sets the deadline.
        let deadline = exp + 300 + i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS);
        assert_eq!(data.invoice_deadline, Some(deadline));
        let receive_dir = format!(
            "{}_{}",
            wallet.normalize_recipient_id(&data.recipient_id),
            &hex::encode(&data.recipient_pubkey)[..16]
        );
        assert_eq!(data.receive_dir, Some(receive_dir.clone()));
        wallet.store_receive_transfer(&data, 1).unwrap();
        mocks[1].assert();
        assert_eq!(
            wallet.database.iter_invoices().unwrap(),
            vec![DbInvoice {
                idx: 1,
                quota: 3,
                asset_id: None,
                deadline,
            }]
        );
        let db_data = wallet.database.get_db_data(false).unwrap();
        assert_eq!(db_data.batch_transfers[0].retention_deadline, None);
        let transfer = &db_data.transfers[0];
        assert_eq!(transfer.invoice_idx, Some(1));
        assert_eq!(wallet.get_receive_dir(transfer), receive_dir);

        // A blind invoice needs a UTXO with room for its full quota.
        add_fake_txos(&wallet);
        let blind_receive = |wallet: &mut Wallet, quota| {
            let (server, _mocks) = proxy_with_quota(quota, 100);
            let res = crate::block_on(wallet.create_receive_data(
                Some(online.clone()),
                None,
                Assignment::Any,
                exp,
                vec![format!("rpc://{}", server.host_with_port())],
                RecipientType::Blind,
                None,
                quota,
            ));
            res.map(|data| wallet.store_receive_transfer(&data, 1).unwrap())
        };
        let max = wallet.max_utxo_allocations();
        for _ in 0..3 {
            blind_receive(&mut wallet, max - 1).unwrap();
        }
        // No UTXO has room for 2. The wallet has no bitcoins to color a new one.
        assert!(matches!(
            blind_receive(&mut wallet, 2),
            Err(Error::InsufficientBitcoins { .. })
        ));
        blind_receive(&mut wallet, 1).unwrap();
    }

    #[test]
    fn witness_receives_on_reused_address_keep_own_data() {
        let mut wallet = make_test_wallet(true);
        let online = online(&mut wallet);
        let (server, _mocks) = proxy_with_quota(1, 100);
        for _ in 0..2 {
            let data = crate::block_on(wallet.create_receive_data(
                Some(online.clone()),
                None,
                Assignment::Any,
                now().unix_timestamp() + 100_000,
                vec![format!("rpc://{}", server.host_with_port())],
                RecipientType::Witness,
                None,
                1,
            ))
            .unwrap();
            wallet.store_receive_transfer(&data, 1).unwrap();
        }
        let transfers = wallet.database.get_db_data(false).unwrap().transfers;
        assert_eq!(transfers[0].recipient_id, transfers[1].recipient_id);
        assert_ne!(
            wallet.get_receive_dir(&transfers[0]),
            wallet.get_receive_dir(&transfers[1])
        );
    }
}

#[cfg(test)]
mod available_allocations_tests {
    use super::address_reuse_tests::make_test_wallet;
    use super::*;

    fn unspent(vout: u32, allocations: usize, pending_blinded: u32) -> LocalUnspent {
        LocalUnspent {
            utxo: DbTxo {
                idx: vout as i32 + 1,
                txid: "33".repeat(32),
                vout,
                btc_amount: s!("1000"),
                spent: false,
                exists: true,
                pending_witness: false,
            },
            rgb_allocations: vec![
                LocalRgbAllocation {
                    asset_id: Some(s!("asset")),
                    assignment: Assignment::Fungible(1),
                    status: TransferStatus::Settled,
                    incoming: true,
                    txo_spent: false,
                };
                allocations
            ],
            pending_blinded,
        }
    }

    #[test]
    fn available_allocations_capped_by_the_bundle_limit() {
        let mut wallet = make_test_wallet(false);
        wallet.wallet_data.max_allocations_per_utxo = u32::MAX;
        let max = MAX_UTXO_ASSET_ALLOCATIONS;
        let mut unspents = vec![
            unspent(0, max - 1, 0),
            unspent(1, max, 0),
            unspent(2, max - 1, 1),
        ];
        let available = |wallet: &Wallet, unspents: &[LocalUnspent], max_allocations| {
            wallet
                .get_available_allocations(unspents.to_vec(), &[], max_allocations)
                .unwrap()
                .iter()
                .map(|u| u.utxo.vout)
                .collect::<Vec<_>>()
        };
        assert_eq!(available(&wallet, &unspents, None), vec![0]);
        // an explicit maximum is not capped
        assert_eq!(available(&wallet, &unspents, Some(u32::MAX)), vec![0, 1, 2]);
        // the lower limit wins
        wallet.wallet_data.max_allocations_per_utxo = 5;
        unspents.push(unspent(3, 4, 0));
        assert_eq!(available(&wallet, &unspents, None), vec![3]);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod inspect_psbt_tests {
    //! Expected values come from rgb-lib inspect_psbt at 4d87b372.
    use super::*;
    use bdk_wallet::bitcoin::{TapLeafHash, hashes::Hash};
    use serde_json::Value;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/inspection/",
                $name
            ))
        };
    }

    fn wallet(keys: &str) -> Wallet {
        address_reuse_tests::make_test_wallet_with_singlesig_keys(
            serde_json::from_str(keys).unwrap(),
            false,
        )
    }

    fn matches_rgb_lib(w: &Wallet, psbt: &str, expected: &str) -> PsbtInspection {
        let res = w.inspect_psbt(psbt.trim().to_string()).unwrap();
        assert_eq!(
            serde_json::to_value(&res).unwrap(),
            serde_json::from_str::<Value>(expected).unwrap()
        );
        res
    }

    #[test]
    fn create_utxos_matches_rgb_lib() {
        let w = wallet(fixture!("psbt_keys_taproot.json"));
        matches_rgb_lib(
            &w,
            fixture!("psbt_create_utxos.psbt"),
            fixture!("psbt_create_utxos.json"),
        );

        // a key-path signature is not counted: same result as the unsigned PSBT
        matches_rgb_lib(
            &w,
            fixture!("psbt_create_utxos_signed.psbt"),
            fixture!("psbt_create_utxos.json"),
        );

        // a script-path signature is counted
        let mut psbt = Psbt::from_str(fixture!("psbt_create_utxos_signed.psbt").trim()).unwrap();
        let input = &mut psbt.inputs[0];
        let sig = input.tap_key_sig.unwrap();
        let key = input.tap_internal_key.unwrap();
        input
            .tap_script_sigs
            .insert((key, TapLeafHash::from_byte_array([0; 32])), sig);
        let res = w.inspect_psbt(psbt.to_string()).unwrap();
        assert_eq!(res.signature_count, 1);
    }

    #[test]
    fn signed_p2wpkh_matches_rgb_lib() {
        let w = wallet(fixture!("psbt_keys_p2wpkh.json"));
        matches_rgb_lib(
            &w,
            fixture!("psbt_p2wpkh_signed.psbt"),
            fixture!("psbt_p2wpkh_signed.json"),
        );
    }

    #[test]
    fn send_matches_rgb_lib() {
        let w = wallet(fixture!("psbt_keys_taproot.json"));
        matches_rgb_lib(&w, fixture!("psbt_send.psbt"), fixture!("psbt_send.json"));
    }

    #[test]
    fn edge_matches_rgb_lib() {
        let w = wallet(fixture!("psbt_keys_taproot.json"));
        // a foreign input, an output without address, outputs above inputs (fee 0)
        matches_rgb_lib(&w, fixture!("psbt_edge.psbt"), fixture!("psbt_edge.json"));
    }

    #[test]
    fn inspect_psbt_errors() {
        let w = wallet(fixture!("psbt_keys_taproot.json"));
        let res = w.inspect_psbt(s!("not a psbt"));
        assert!(matches!(res, Err(Error::InvalidPsbt { .. })), "{res:?}");

        let mut psbt = Psbt::from_str(fixture!("psbt_create_utxos.psbt").trim()).unwrap();
        psbt.inputs[0].witness_utxo = None;
        let res = w.inspect_psbt(psbt.to_string());
        assert!(
            matches!(&res, Err(Error::PsbtInspection { details }) if details == "cannot inspect non-segwit PSBT"),
            "{res:?}"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod export_contract_tests {
    use super::address_reuse_tests::{add_fake_txos, media_attachment, set_media, state};
    use super::*;
    use crate::keys::generate_keys;

    fn exporting_wallet() -> Wallet {
        let keys = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot);
        let wd = WalletData {
            data_dir: s!(":memory:"),
            bitcoin_network: BitcoinNetwork::Regtest,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            supported_schemas: vec![AssetSchema::Nia, AssetSchema::Ifa, AssetSchema::Bfa],
            reuse_addresses: false,
            allow_reckless_schemas: false,
        };
        let wallet = Wallet::new(wd, SinglesigKeys::from_keys(&keys, None)).unwrap();
        add_fake_txos(&wallet);
        wallet
    }

    fn assert_exported(wallet: &Wallet, asset_id: &str, schema: AssetSchema) -> ExportedContract {
        let before = state(wallet);
        let exported = wallet.export_contract(asset_id.to_string()).unwrap();
        assert_eq!(state(wallet), before);
        let contract = Contract::load(&exported.contract[..]).unwrap();
        assert_eq!(contract.contract_id().to_string(), asset_id);
        assert_eq!(contract.schema_id(), SchemaId::from(schema));
        assert!(!contract.transfer);
        assert!(contract.bundles.is_empty());
        assert_eq!(
            wallet
                .get_asset_metadata(asset_id.to_string())
                .unwrap()
                .timestamp,
            contract.genesis.timestamp
        );
        exported
    }

    #[test]
    fn export_contract_success() {
        // the clock of the wallet differs from the genesis clock
        crate::utils::CLOCK_OFFSET.with(|o| o.set(10));
        let mut wallet = exporting_wallet();
        set_media(&mut wallet, b"nia media");
        let nia = wallet
            .issue_asset_nia(s!("NIA"), s!("Nia"), 0, vec![100])
            .unwrap();
        set_media(&mut wallet, b"ifa media");
        let ifa = wallet
            .issue_asset_ifa(
                s!("IFA"),
                s!("Ifa"),
                0,
                vec![100],
                vec![],
                None,
                false,
                None,
                false,
            )
            .unwrap();
        set_media(&mut wallet, b"bfa media");
        let bfa = wallet
            .issue_asset_bfa(
                s!("BFA"),
                s!("Bfa"),
                0,
                1,
                31337,
                s!("0x5FbDB2315678afecb367f032d93F642f64180aa3"),
                None,
                false,
                None,
                false,
            )
            .unwrap();

        let no_media = wallet
            .issue_asset_nia(s!("NONE"), s!("None"), 0, vec![100])
            .unwrap();

        let exported = assert_exported(&wallet, &nia.asset_id, AssetSchema::Nia);
        assert_eq!(exported.media, vec![b"nia media".to_vec()]);
        let exported = assert_exported(&wallet, &ifa.asset_id, AssetSchema::Ifa);
        assert_eq!(exported.media, vec![b"ifa media".to_vec()]);
        let exported = assert_exported(&wallet, &bfa.asset_id, AssetSchema::Bfa);
        assert_eq!(exported.media, vec![b"bfa media".to_vec()]);
        let exported = assert_exported(&wallet, &no_media.asset_id, AssetSchema::Nia);
        assert!(exported.media.is_empty());
    }

    #[test]
    fn export_contract_fail() {
        let wallet = exporting_wallet();
        let res = wallet.export_contract(s!("rgb:unknown"));
        assert!(
            matches!(&res, Err(Error::AssetNotFound { asset_id }) if asset_id == "rgb:unknown"),
            "{res:?}"
        );

        MOCK_ISSUE_MEDIA.set(Some(media_attachment(b"media")));
        let nia = wallet
            .issue_asset_nia(s!("NIA"), s!("Nia"), 0, vec![100])
            .unwrap();
        let res = wallet.export_contract(nia.asset_id);
        let expected = format!("media {} not found", hex::encode(sha256_digest(b"media")));
        assert!(
            matches!(&res, Err(Error::IO { details }) if *details == expected),
            "{res:?}"
        );
    }
}
