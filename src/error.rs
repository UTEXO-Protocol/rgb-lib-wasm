//! Error
//!
//! This module defines the [`Error`] enum, containing all error variants returned by functions in
//! the library.

use super::*;

/// The error variants returned by functions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Deserialize, Serialize)]
pub enum Error {
    /// Address rotation called when reuse_addresses is false
    #[error("Address reuse is not enabled")]
    AddressReuseDisabled,

    /// No need to create more allocations
    #[error("Allocations already available")]
    AllocationsAlreadyAvailable,

    /// Requested asset was not found
    #[error("Asset with id {asset_id} not found")]
    AssetNotFound {
        /// Asset ID
        asset_id: String,
    },

    /// The requested batch transfer was not found
    #[error("Batch transfer with idx {idx} not found")]
    BatchTransferNotFound {
        /// Transfer idx
        idx: i32,
    },

    /// The wallet has already been loaded on a different bitcoin network
    #[error("Bitcoin network mismatch")]
    BitcoinNetworkMismatch,

    /// Requested pending vanilla TX cannot be aborted
    #[error("Pending vanilla TX cannot be aborted")]
    CannotAbortPendingVanillaTx,

    /// A wallet cannot go online twice with different data
    #[error("Cannot change online object")]
    CannotChangeOnline,

    /// Requested batch transfer cannot be deleted
    #[error("Batch transfer cannot be deleted")]
    CannotDeleteBatchTransfer,

    /// Cannot estimate fees
    #[error("Cannot estimate fees")]
    CannotEstimateFees,

    /// The transfer has failed, expired or progressed, so it cannot be ended
    #[error("Transfer cannot be ended: {txid}")]
    CannotEndTransfer {
        /// Transfer TXID
        txid: String,
    },

    /// Requested batch transfer cannot be failed
    #[error("Batch transfer cannot be set to failed status")]
    CannotFailBatchTransfer,

    /// The given PSBT cannot be finalized
    #[error("The given PSBT cannot be finalized")]
    CannotFinalizePsbt,

    /// An asset cannot be linked to itself
    #[error("An asset cannot be linked to itself")]
    CannotLinkToItself,

    /// Cannot use BFA schema on mainnet
    #[error("Cannot use BFA schema on mainnet")]
    CannotUseBfaOnMainnet,

    /// Cannot use IFA schema on mainnet
    #[error("Cannot use IFA schema on mainnet")]
    CannotUseIfaOnMainnet,

    /// Two proxy authentication key entries resolve to the same proxy URL
    #[error("Duplicate proxy authentication key: {details}")]
    DuplicateProxyAuthKey {
        /// Error details
        details: String,
    },

    /// The EVM RPC endpoint is on a chain other than the configured one
    #[error("EVM chain ID mismatch: expected {expected}, the RPC endpoint is on {actual}")]
    EvmChainIdMismatch {
        /// Configured chain ID
        expected: u64,
        /// Chain ID reported by the EVM RPC endpoint
        actual: u64,
    },

    /// The provided file is empty
    #[error("Empty file: {file_path}")]
    EmptyFile {
        /// File path
        file_path: String,
    },

    /// The provided expiration is invalid (in the past or too far in the future)
    #[error("Invalid expiration")]
    ExpirationInvalid,

    /// Syncing BDK with the blockchain has failed
    #[error("Failed bdk sync: {details}")]
    FailedBdkSync {
        /// Error details
        details: String,
    },

    /// Broadcasting the PSBT has failed
    #[error("Failed broadcast: {details}")]
    FailedBroadcast {
        /// Error details
        details: String,
    },

    /// Issued RGB asset has failed the validity check
    #[error("Failed issuance. Register status: {details}")]
    FailedIssuance {
        /// Error details
        details: String,
    },

    /// The file already exists
    #[error("The file already exists: {path}")]
    FileAlreadyExists {
        /// The file path
        path: String,
    },

    /// A master fingerprint does not match the wallet's
    #[error("Fingerprint mismatch")]
    FingerprintMismatch,

    /// A proxy payload, decrypted if it was encrypted, doesn't match its expected hash
    #[error("Hash mismatch")]
    HashMismatch,

    /// An I/O error has been encountered
    #[error("I/O error: {details}")]
    IO {
        /// Error details
        details: String,
    },

    /// An inconsistency has been detected between the wallet's internal (database) and external
    /// (BDK, RGB) data
    #[error("Data is inconsistent ({details}). Please check its integrity.")]
    Inconsistency {
        /// Error details
        details: String,
    },

    /// The content signature was rejected as invalid
    #[error("Invalid content signature")]
    InvalidContentSignature,

    /// A proxy payload is malformed or cannot be decrypted
    #[error("Invalid encrypted blob: {details}")]
    InvalidEncryptedBlob {
        /// Error details
        details: String,
    },

    /// The endpoint signature was rejected as invalid
    #[error("Invalid endpoint auth signature")]
    InvalidEndpointAuthSignature,

    /// The proxy auth signature was rejected as invalid
    #[error("Invalid proxy auth signature")]
    InvalidProxyAuthSignature,

    /// The recipient proxy refused the transfer as the invoice has expired, the invoice expiration
    /// has passed before the transfer could be created, or it leaves too little time for a mint
    /// lock
    #[error("Invoice expired")]
    InvoiceExpired,

    /// The invoice has not been reserved on the recipient proxy
    #[error("Invoice not reserved")]
    InvoiceNotReserved,

    /// The invoice cannot accept more transfers on the recipient proxy
    #[error("Invoice quota exceeded")]
    InvoiceQuotaExceeded,

    /// No media found
    #[error("No media found")]
    NoMedia,

    /// The proxy operation has expired
    #[error("Operation expired")]
    OperationExpired,

    /// The proxy operation was not found
    #[error("Operation not found")]
    OperationNotFound,

    /// Durable wallet persistence failed
    #[error("Persistence error: {details}")]
    Persistence {
        /// Error details
        details: String,
    },

    /// An error was received from the indexer
    #[error("Indexer error: {details}")]
    Indexer {
        /// Error details
        details: String,
    },

    /// The provided directory does not exist
    #[error("Inexistent data directory")]
    InexistentDataDir,

    /// There are not enough available allocation slots (UTXOs with available slots)
    #[error("Insufficient allocations")]
    InsufficientAllocationSlots,

    /// There are not enough assignments of the requested asset to fulfill the request
    #[cfg(feature = "esplora")]
    #[error("Insufficient total assignments for asset: {asset_id}")]
    InsufficientAssignments {
        /// Asset ID
        asset_id: String,
        /// Available assignments
        available: AssignmentsCollection,
    },

    /// There are not enough bitcoins to fulfill the request
    #[error("Insufficient bitcoin funds: needed '{needed}', available '{available}'")]
    InsufficientBitcoins {
        /// Sats needed for some transaction
        needed: u64,
        /// Sats available for spending
        available: u64,
    },

    /// An internal error has been encountered
    #[error("Internal error: {details}")]
    Internal {
        /// Error details
        details: String,
    },

    /// An invalid bitcoin address has been provided
    #[error("Address error: {details}")]
    InvalidAddress {
        /// Error details
        details: String,
    },

    /// An invalid 0 amount has been provided
    #[error("Amount 0 is invalid")]
    InvalidAmountZero,

    /// An invalid assignment has been provided
    #[error("Invalid assignment")]
    InvalidAssignment,

    /// The provided attachments are invalid
    #[error("Invalid attachments: {details}")]
    InvalidAttachments {
        /// Error details
        details: String,
    },

    /// The provided external chain block, before which the lock of a mint has to be included,
    /// leaves no block for it
    #[error("Invalid before block: {details}")]
    InvalidBeforeBlock {
        /// Error details
        details: String,
    },

    /// Keys derived from the provided data do not match
    #[error("Invalid bitcoin keys")]
    InvalidBitcoinKeys,

    /// Invalid bitcoin network
    #[error("Invalid bitcoin network: {network}")]
    InvalidBitcoinNetwork {
        /// The invalid network
        network: String,
    },

    /// The provided coloring info is invalid
    #[error("Invalid coloring info")]
    InvalidColoringInfo {
        /// Error details
        details: String,
    },

    /// The backup data is invalid or corrupted
    #[error("Invalid backup data")]
    InvalidBackup,

    /// The consignment is invalid
    #[error("Invalid consignment")]
    InvalidConsignment,

    /// The provided burn reason is invalid
    #[error("Invalid burn reason: {details}")]
    InvalidBurnReason {
        /// Error details
        details: String,
    },

    /// The provided contract ID is invalid
    #[error("Invalid contract ID: {contract_id}")]
    InvalidContractId {
        /// The invalid contract ID
        contract_id: String,
    },

    /// The provided asset details is invalid
    #[error("Invalid details: {details}")]
    InvalidDetails {
        /// Error details
        details: String,
    },

    /// Electrum server does not provide the required functionality
    ///
    /// There are multiple electrum server variants and one with `verbose` support in
    /// `blockchain.transaction.get` is required, see this
    /// [issue](https://github.com/Blockstream/electrs/pull/36) on blockstream's electrs fork for
    /// more info
    #[error("Invalid electrum server: {details}")]
    InvalidElectrum {
        /// Error details
        details: String,
    },

    /// Trying to request fee estimation for an invalid block number, it must be between 1 and 1008
    #[error("Trying to request fee estimation for an invalid block number")]
    InvalidEstimationBlocks,

    /// The provided expiration is invalid
    #[error("Invalid expiration")]
    InvalidExpiration,

    /// The provided EVM contract address is invalid
    #[error("Invalid EVM contract address: {details}")]
    InvalidEvmContractAddress {
        /// Error details
        details: String,
    },

    /// The provided EVM chain configuration is invalid (see `OnlineOptions::evm_chains`)
    #[error("Invalid EVM chain: {details}")]
    InvalidEvmChain {
        /// Error details
        details: String,
    },

    /// The provided EVM RPC URL is invalid, the endpoint cannot be reached or it does not serve
    /// what confirming mint locks needs (the latest finalized block and the contract state as of
    /// it)
    #[error("Invalid EVM RPC URL: {details}")]
    InvalidEvmRpcUrl {
        /// Error details
        details: String,
    },

    /// The provided fee rate is invalid
    #[error("Invalid fee rate: {details}")]
    InvalidFeeRate {
        /// Error details
        details: String,
    },

    /// The provided file path is invalid
    #[error("Invalid file path: {file_path}")]
    InvalidFilePath {
        /// File path
        file_path: String,
    },

    /// The fingerprint is invalid
    #[error("Invalid fingerprint")]
    InvalidFingerprint,

    /// The provided indexer is invalid
    #[error("Invalid indexer: {details}")]
    InvalidIndexer {
        /// Error details
        details: String,
    },

    /// The provided invoice is invalid
    #[error("Invalid invoice: {details}")]
    InvalidInvoice {
        /// Error details
        details: String,
    },

    /// The provided mnemonic phrase is invalid
    #[error("Invalid mnemonic error: {details}")]
    InvalidMnemonic {
        /// Error details
        details: String,
    },

    /// The provided asset name is invalid
    #[error("Invalid name: {details}")]
    InvalidName {
        /// Error details
        details: String,
    },

    /// The provided asset precision is invalid
    #[error("Invalid precision: {details}")]
    InvalidPrecision {
        /// Error details
        details: String,
    },

    /// The provided proxy authentication key is invalid
    #[error("Invalid proxy authentication key: {details}")]
    InvalidProxyAuthKey {
        /// Error details
        details: String,
    },

    /// The provided proxy URL points to a proxy running an unsupported protocol version
    #[error("Invalid proxy protocol version: {version}")]
    InvalidProxyProtocol {
        /// Detected version
        version: String,
    },

    /// The provided PSBT could not be parsed
    #[error("Invalid PSBT: {details}")]
    InvalidPsbt {
        /// Error details
        details: String,
    },

    /// The provided pubkey is invalid
    #[error("Invalid pubkey: {details}")]
    InvalidPubkey {
        /// Error details
        details: String,
    },

    /// The provided recipient data is invalid
    #[error("The provided recipient data is invalid: {details}")]
    InvalidRecipientData {
        /// Error details
        details: String,
    },

    /// The provided recipient map is invalid
    #[error("The provided recipient map is invalid")]
    InvalidRecipientMap,

    /// The provided recipient ID is neither a blinded UTXO or a script
    #[error("The provided recipient ID is neither a blinded UTXO or a script")]
    InvalidRecipientID,

    /// The provided recipient ID is for a different network than the wallet's one
    #[error("The provided recipient ID is for a different network than the wallet's one")]
    InvalidRecipientNetwork,

    /// The provided reject list URL is invalid
    #[error("Invalid reject list URL: {details}")]
    InvalidRejectListUrl {
        /// Error details
        details: String,
    },

    /// The provided asset ticker is invalid
    #[error("Invalid ticker: {details}")]
    InvalidTicker {
        /// Error details
        details: String,
    },

    /// The provided transport endpoint is invalid
    #[error("Invalid transport endpoint: {details}")]
    InvalidTransportEndpoint {
        /// Error details
        details: String,
    },

    /// The provided transport endpoints are invalid
    #[error("Invalid transport endpoints: {details}")]
    InvalidTransportEndpoints {
        /// Error details
        details: String,
    },

    /// The provided TXID is invalid
    #[error("Invalid TXID")]
    InvalidTxid,

    /// The provided vanilla keychain is invalid
    #[error("Invalid vanilla keychain")]
    InvalidVanillaKeychain,

    /// The maximum fee has been exceeded
    #[error("Max fee exceeded for transfer with TXID: {txid}")]
    MaxFeeExceeded {
        /// TXID of the transfer having fee issues
        txid: String,
    },

    /// The minimum fee is not met
    #[error("Min fee not met for transfer with TXID: {txid}")]
    MinFeeNotMet {
        /// TXID of the transfer having fee issues
        txid: String,
    },

    /// A mint cannot spend the last mint right of the wallet, which would be left unable to mint
    #[error("Cannot mint with the last mint right")]
    LastMintRight,

    /// The lock of the mint operation was not included on the external chain before the block the
    /// mint commits to, and never will be: the mint will never be confirmed
    #[error("Lock for mint operation {opid} not included before block {before_block}")]
    MintLockExpired {
        /// ID of the mint operation
        opid: String,
        /// External chain block the lock had to be included before
        before_block: u64,
    },

    /// No confirmed mint lock committing to the mint operation and to the TX including it (see
    /// [`ReceivedMint::lock_id`](crate::wallet::ReceivedMint::lock_id)) was found on the external
    /// chain (yet): it may still be, e.g. once the lock is made or finalized
    #[error("Lock for mint operation {opid} not found on the external chain")]
    MintLockNotFound {
        /// ID of the mint operation
        opid: String,
    },

    /// The signed PSBT of a transfer is missing, so the transfer has failed
    #[error("Signed PSBT not found for transfer with TXID: {txid}, the transfer has failed")]
    MissingTransferArtifacts {
        /// TXID of the failed transfer
        txid: String,
    },

    /// A network error occurred
    #[error("Network error: {details}")]
    Network {
        /// Error details
        details: String,
    },

    /// Cannot burn an asset with zero amount
    #[error("Burn request with zero amount")]
    NoBurnAmount,

    /// No consignment found
    #[error("No consignment found")]
    NoConsignment,

    /// No EVM RPC URL is configured for the chain
    #[error("No EVM RPC URL configured for chain ID {chain_id}")]
    NoEvmRpcUrl {
        /// EVM chain ID
        chain_id: u64,
    },

    /// Cannot inflate an asset with unknown o zero amounts
    #[error("Inflation request with no amounts or zero amounts")]
    NoInflationAmounts,

    /// Cannot issue an asset without knowing the amounts
    #[error("Issuance request with no provided amounts")]
    NoIssuanceAmounts,

    /// Cannot issue an asset with zero mint rights
    #[error("Issuance request with zero mint rights")]
    NoMintRights,

    /// Cannot create a wallet with no supported schemas
    #[error("Cannot create a wallet with no supported schemas")]
    NoSupportedSchemas,

    /// No valid transport endpoint found
    #[error("No valid transport endpoint found")]
    NoValidTransportEndpoint,

    /// Trying to perform an online operation with offline wallet
    #[error("Wallet is offline. Hint: call go_online")]
    Offline,

    /// The Online object is needed to perform this operation
    #[error("The Online object is needed to perform this operation")]
    OnlineNeeded,

    /// Output created is under the dust limit
    #[error("Output below the dust limit")]
    OutputBelowDustLimit,

    /// Error contacting the RGB proxy
    #[error("Proxy error: {details}")]
    Proxy {
        /// Error details
        details: String,
    },

    /// The proxy rejected a change to immutable operation data
    #[error("Proxy cannot change: {details}")]
    ProxyCannotChange {
        /// Error details
        details: String,
    },

    /// The proxy is draining and doesn't accept new transfers
    #[error("Proxy draining")]
    ProxyDraining,

    /// The proxy doesn't accept any more media for the transfer
    #[error("Proxy doesn't accept so many media")]
    ProxyTooManyMedia,

    /// The signed TX was already released to a recipient, so it can no longer be revoked
    #[error("TX already released")]
    ProxyTxAlreadyReleased,

    /// Provided recipient ID has already been used for another transfer
    #[error("Recipient ID already used")]
    RecipientIDAlreadyUsed,

    /// Provided recipient map has duplicated recipient IDs
    #[error("Recipient ID duplicated")]
    RecipientIDDuplicated,

    /// Error contacting the reject list service
    #[error("Reject list service error: {details}")]
    RejectListService {
        /// Error details
        details: String,
    },

    /// Error building a rest client
    #[error("Error building a rest client")]
    RestClientBuild {
        /// Error details
        details: String,
    },

    /// The proxy's registered signed TX for this operation was previously cleared
    #[error("Signed TX was cleared")]
    SignedTxCleared,

    /// The inflation amount exceeds the max possible supply
    #[error("The inflation amount exceeds the max possible supply")]
    TooHighInflationAmounts,

    /// Trying to issue too many assets
    #[error("Trying to issue too many assets")]
    TooHighIssuanceAmounts,

    /// The transfer ID has been deleted on the recipient proxy and cannot be reused
    #[error("Transfer ID deleted")]
    TransferIDDeleted,

    /// The transfer ID is already in use by a different operation on the sender proxy
    #[error("Transfer ID in use")]
    TransferIDInUse,

    /// The detected RGB schema is unknown
    #[error("Unknown RGB schema: {schema_id}")]
    UnknownRgbSchema {
        /// RGB schema ID
        schema_id: String,
    },

    /// The transfer has expired
    #[error("Transfer expired")]
    TransferExpired,

    /// The detected transfer is unknown
    #[error("Unknown transfer: {txid}")]
    UnknownTransfer {
        /// Transfer TXID
        txid: String,
    },

    /// The backup version is not supported
    #[error("Backup version not supported")]
    UnsupportedBackupVersion {
        /// Backup version
        version: String,
    },

    /// The schema doesn't support burn transitions
    #[error("Burn not supported")]
    UnsupportedBurn {
        /// Asset schema
        asset_schema: AssetSchema,
    },

    /// The schema doesn't support mint transitions
    #[error("Mint not supported")]
    UnsupportedMint {
        /// Asset schema
        asset_schema: AssetSchema,
    },

    /// The schema doesn't support inflation
    #[error("Inflation not supported")]
    UnsupportedInflation {
        /// Asset schema
        asset_schema: AssetSchema,
    },

    /// The given layer 1 is not supported
    #[error("Layer 1 {layer_1} is not supported")]
    UnsupportedLayer1 {
        /// Layer 1
        layer_1: String,
    },

    /// The schema doesn't support link transitions
    #[error("Link not supported")]
    UnsupportedLink {
        /// Asset schema
        asset_schema: AssetSchema,
    },

    /// The given schema is not supported
    #[error("Schema {asset_schema} is not supported")]
    UnsupportedSchema {
        /// Asset schema
        asset_schema: AssetSchema,
    },

    /// The given transport type is not supported
    #[error("Transport type is not supported")]
    UnsupportedTransportType,

    /// VSS authentication failed
    #[error("VSS authentication failed: {details}")]
    VssAuth {
        /// Error details
        details: String,
    },

    /// VSS backup not found on server
    #[error("VSS backup not found")]
    VssBackupNotFound,

    /// VSS server communication error
    #[error("VSS error: {details}")]
    VssError {
        /// Error details
        details: String,
    },

    /// VSS version conflict (concurrent modification)
    #[error("VSS version conflict: {details}")]
    VssVersionConflict {
        /// Error details
        details: String,
    },

    /// The specified wallet directory already exists
    #[error("The specified wallet directory already exists: {path}")]
    WalletDirAlreadyExists {
        /// The directory path
        path: String,
    },

    /// The requested operation cannot be processed by a watch-only wallet
    #[error("Operation not allowed on watch only wallet")]
    WatchOnly,

    /// The PSBT belongs to a different kind of operation than the expected one
    #[error("PSBT belongs to a different operation: {txid}")]
    WrongOperation {
        /// TXID of the PSBT
        txid: String,
    },

    /// The provided password is incorrect
    #[error("The provided password is incorrect")]
    WrongPassword,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IndexerError {
    #[cfg(feature = "esplora")]
    #[error("Esplora async error: {0}")]
    EsploraAsync(String),
}

#[cfg(feature = "esplora")]
impl From<bdk_esplora::esplora_client::Error> for IndexerError {
    fn from(e: bdk_esplora::esplora_client::Error) -> Self {
        IndexerError::EsploraAsync(e.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InternalError {
    #[error("Aead error: {0}")]
    AeadError(String),

    #[cfg(feature = "esplora")]
    #[error("API error: {0}")]
    Api(#[from] reqwest::Error),

    #[error("Invalid backup path")]
    BackupInvalidPath(#[from] std::io::Error),

    #[error("Base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Error from bdk adding UTXOs: {0}")]
    BdkAddUtxoError(#[from] bdk_wallet::tx_builder::AddUtxoError),

    #[error("Error from bdk extracting TX: {0}")]
    BdkExtractTxError(String),

    #[error("Error from bdk signing: {0}")]
    BdkSignerError(#[from] bdk_wallet::signer::SignerError),

    #[error("Confinement error: {0}")]
    Confinement(#[from] amplify::confinement::Error),

    #[error("Encode error: {0}")]
    Encode(#[from] bitcoin::consensus::encode::Error),

    #[error("From slice error: {0}")]
    FromSlice(#[from] amplify::FromSliceError),

    #[error("Hash error: {0}")]
    HashError(#[from] scrypt::password_hash::phc::Error),

    #[error("Hex decode error: {0}")]
    HexError(String),

    #[error("Infallible error: {0}")]
    Infallible(#[from] std::convert::Infallible),

    #[error("PSBT parse error: {0}")]
    PsbtParse(#[from] bdk_wallet::bitcoin::psbt::PsbtParseError),

    #[error("RGB load error: {0}")]
    RgbLoad(#[from] rgbstd::containers::LoadError),

    #[error("RGB PSBT error: {0}")]
    RgbPsbtError(String),

    #[error("Seal parse error: {0}")]
    SealParse(#[from] rgbstd::txout::explicit::ParseError),

    #[error("Serde JSON error: {0}")]
    SerdeJSON(#[from] serde_json::Error),

    #[error("Strip prefix error: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),

    #[error("System time error: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),

    #[error("Unexpected error")]
    Unexpected,
}

impl From<bdk_wallet::keys::bip39::Error> for Error {
    fn from(e: bdk_wallet::keys::bip39::Error) -> Self {
        Error::InvalidMnemonic {
            details: e.to_string(),
        }
    }
}

impl From<bdk_wallet::bitcoin::bip32::Error> for Error {
    fn from(e: bdk_wallet::bitcoin::bip32::Error) -> Self {
        Error::InvalidPubkey {
            details: e.to_string(),
        }
    }
}

impl From<bdk_wallet::bitcoin::psbt::PsbtParseError> for Error {
    fn from(e: bdk_wallet::bitcoin::psbt::PsbtParseError) -> Self {
        Error::InvalidPsbt {
            details: e.to_string(),
        }
    }
}

impl From<IndexerError> for Error {
    fn from(e: IndexerError) -> Self {
        Error::Indexer {
            details: e.to_string(),
        }
    }
}

impl From<bdk_wallet::bitcoin::psbt::ExtractTxError> for InternalError {
    fn from(e: bdk_wallet::bitcoin::psbt::ExtractTxError) -> Self {
        InternalError::BdkExtractTxError(e.to_string())
    }
}

impl From<psrgbt::RgbPsbtError> for InternalError {
    fn from(e: psrgbt::RgbPsbtError) -> Self {
        InternalError::RgbPsbtError(e.to_string())
    }
}

impl From<psrgbt::OpretKeyError> for InternalError {
    fn from(e: psrgbt::OpretKeyError) -> Self {
        InternalError::RgbPsbtError(e.to_string())
    }
}

impl From<psrgbt::MpcPsbtError> for InternalError {
    fn from(e: psrgbt::MpcPsbtError) -> Self {
        InternalError::RgbPsbtError(e.to_string())
    }
}

impl<E: std::error::Error> From<rgbstd::persistence::StockError<MemRgbStore, E>> for Error {
    fn from(e: rgbstd::persistence::StockError<MemRgbStore, E>) -> Self {
        match e {
            rgbstd::persistence::StockError::Resolver(details) => Error::Indexer { details },
            rgbstd::persistence::StockError::WitnessUnresolved(txid, e) => Error::Indexer {
                details: format!("cannot resolve witness {txid}: {e}"),
            },
            e => Error::Internal {
                details: format!("Stock err: {e}"),
            },
        }
    }
}

impl From<rgbinvoice::TransportParseError> for Error {
    fn from(e: rgbinvoice::TransportParseError) -> Self {
        Error::InvalidTransportEndpoint {
            details: e.to_string(),
        }
    }
}

impl From<bdk_wallet::CreateWithPersistError<std::convert::Infallible>> for Error {
    fn from(e: bdk_wallet::CreateWithPersistError<std::convert::Infallible>) -> Self {
        Error::IO {
            details: e.to_string(),
        }
    }
}

impl From<bdk_wallet::LoadWithPersistError<std::convert::Infallible>> for Error {
    fn from(e: bdk_wallet::LoadWithPersistError<std::convert::Infallible>) -> Self {
        match e {
            bdk_wallet::LoadWithPersistError::InvalidChangeSet(
                bdk_wallet::LoadError::Mismatch(bdk_wallet::LoadMismatch::Genesis { .. }),
            ) => Error::BitcoinNetworkMismatch,
            _ => Error::IO {
                details: e.to_string(),
            },
        }
    }
}

impl From<std::convert::Infallible> for Error {
    fn from(e: std::convert::Infallible) -> Self {
        match e {}
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::IO {
            details: e.to_string(),
        }
    }
}

impl From<ClaimResolverError> for Error {
    fn from(e: ClaimResolverError) -> Self {
        match e {
            ClaimResolverError::WrongChainId { expected, actual } => {
                Error::EvmChainIdMismatch { expected, actual }
            }
            ClaimResolverError::Unconfirmed(opid) => Error::MintLockNotFound {
                opid: opid.to_string(),
            },
            ClaimResolverError::ResolverIssue(details) => Error::Network { details },
            // only the claims the validation asks about are resolved
            e @ ClaimResolverError::Validation(_) => Error::Internal {
                details: format!("unexpected claim resolution error: {e}"),
            },
        }
    }
}

impl From<InternalError> for Error {
    fn from(e: InternalError) -> Self {
        Error::Internal {
            details: e.to_string(),
        }
    }
}

impl From<rgbstd::contract::BuilderError> for Error {
    fn from(e: rgbstd::contract::BuilderError) -> Self {
        Error::Internal {
            details: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_errors_map_to_rgb_lib_errors() {
        type StockErr = rgbstd::persistence::StockError<MemRgbStore, std::convert::Infallible>;

        let e: Error = StockErr::Resolver(s!("down")).into();
        assert_eq!(
            e,
            Error::Indexer {
                details: s!("down")
            }
        );

        let id = format!("{}1", "0".repeat(63));
        let txid = RgbTxid::from_str(&id).unwrap();
        let e: Error = StockErr::WitnessUnresolved(txid, WitnessResolverError::NotSupported).into();
        assert_eq!(
            e,
            Error::Indexer {
                details: format!(
                    "cannot resolve witness {id}: operation is not supported by this resolver"
                )
            }
        );

        let e: Error = StockErr::AbsentValidWitness.into();
        assert_eq!(
            e,
            Error::Internal {
                details: s!("Stock err: AbsentValidWitness")
            }
        );
    }

    #[test]
    fn claim_resolver_errors_map() {
        let e: Error = ClaimResolverError::WrongChainId {
            expected: 1,
            actual: 2,
        }
        .into();
        assert_eq!(
            e,
            Error::EvmChainIdMismatch {
                expected: 1,
                actual: 2
            }
        );
        let opid = OpId::from([1u8; 32]);
        let e: Error = ClaimResolverError::Unconfirmed(opid).into();
        assert_eq!(
            e,
            Error::MintLockNotFound {
                opid: opid.to_string()
            }
        );
        let e: Error = ClaimResolverError::ResolverIssue(s!("down")).into();
        assert_eq!(
            e,
            Error::Network {
                details: s!("down")
            }
        );
        let e: Error = ClaimResolverError::Validation(ValidationError::UnknownClaim).into();
        assert!(matches!(e, Error::Internal { .. }));
    }
}
