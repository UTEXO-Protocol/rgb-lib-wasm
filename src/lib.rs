#![allow(clippy::too_many_arguments)]
#![allow(dead_code, unused_imports)]
#![warn(missing_docs)]

//! A library to manage wallets for RGB assets.
//!
//! ## Wallet
//! The main component of the library is the [`Wallet`].
//!
//! It allows to create and operate an RGB wallet that can issue, send and receive NIA and IFA
//! assets. The library also manages UTXOs and asset allocations.
//!
//! ## Backend
//! The library uses BDK for walleting operations and several components from the RGB ecosystem for
//! RGB asset operations.
//!
//! ## Database
//! An in-memory database with IndexedDB persistence is used for data storage.
//!
//! ## Api
//! RGB asset transfers require the exchange of off-chain data in the form of consignment or media
//! files.
//!
//! The library currently implements the API for a proxy server to support these data exchanges
//! between sender and receiver.
//!
//! ## Errors
//! Errors are handled with the crate `thiserror`.

pub(crate) mod api;
pub(crate) mod database;
pub(crate) mod error;
pub mod keys;
pub mod utils;
pub mod wallet;

pub use bdk_wallet;
pub use bdk_wallet::bitcoin;
pub use rgbinvoice::RgbTransport;
pub use rgbstd::{
    ChainNet, ContractId, Txid as RgbTxid,
    containers::{ConsignmentExt, Fascia, FileContent, PubWitness, Transfer as RgbTransfer},
    indexers::AnyResolver,
    persistence::UpdateRes,
    schema::SchemaId,
    validation::{ValidationConfig, ValidationError},
    vm::{WitnessOrd, WitnessPos},
};

pub use crate::{
    database::enums::{AssetSchema, Assignment, TransferStatus, TransportType},
    error::Error,
    keys::{generate_keys, restore_keys},
    utils::BitcoinNetwork,
    wallet::{RecipientType, TransactionType, TransferKind, Wallet},
};

#[cfg(feature = "esplora")]
use std::{
    cmp::{Ordering, max, min},
    collections::hash_map::DefaultHasher,
    hash::Hasher,
    num::NonZeroU32,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
    hash::Hash,
    io::{self, ErrorKind},
    panic,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::database::memory_db::ActiveValue;
use amplify::{Wrapper, bmap, confinement::Confined, s};
#[cfg(feature = "esplora")]
use base64::{Engine as _, engine::general_purpose};
#[cfg(feature = "esplora")]
use bdk_wallet::bitcoin::Txid;
use bdk_wallet::{
    ChangeSet, KeychainKind, LocalOutput, PersistedWallet, SignOptions, Wallet as BdkWallet,
    bitcoin::{
        Address as BdkAddress, Amount as BdkAmount, BlockHash, Network as BdkNetwork, NetworkKind,
        OutPoint, OutPoint as BdkOutPoint, ScriptBuf, TxOut,
        bip32::{ChildNumber, DerivationPath, Fingerprint, KeySource, Xpriv, Xpub},
        constants::ChainHash,
        hashes::{Hash as Sha256Hash, sha256},
        psbt::{ExtractTxError, Psbt},
        secp256k1::Secp256k1,
    },
    chain::{CanonicalizationParams, ChainPosition},
    descriptor::Segwitv0,
    keys::{
        DerivableKey, DescriptorKey,
        DescriptorKey::{Public, Secret},
        ExtendedKey, GeneratableKey,
        bip39::{Language, Mnemonic, WordCount},
    },
};
#[cfg(feature = "esplora")]
use bdk_wallet::{
    Update,
    bitcoin::{Transaction as BdkTransaction, blockdata::fee_rate::FeeRate},
    chain::{
        DescriptorExt,
        spk_client::{FullScanRequest, FullScanResponse, SyncRequest, SyncResponse},
    },
    coin_selection::InsufficientFunds,
};
use chacha20poly1305::{
    Key, KeyInit, XChaCha20Poly1305,
    aead::{generic_array::GenericArray, stream},
};
use psrgbt::{RgbOutExt, RgbPsbtExt};
use rand::{Rng, distr::Alphanumeric};
use rgbinvoice::{AddressPayload, Beneficiary, RgbInvoice, RgbInvoiceBuilder, XChainNet};
use rgbstd::{
    Allocation, Amount, Genesis, GraphSeal, Identity, Layer1, Operation, Opout, OutputSeal,
    OwnedFraction, Precision, Schema, SecretSeal, Transition, TransitionType, TypeSystem,
    containers::{BuilderSeal, Kit, ValidContract, ValidKit, ValidTransfer},
    contract::{AllocatedState, ContractBuilder, IssuerWrapper, TransitionBuilder},
    info::{ContractInfo, SchemaInfo},
    invoice::{InvoiceState, Pay2Vout},
    persistence::{MemContract, MemContractState, StashReadProvider, Stock, fs::FsBinStore},
    rgbcore::commit_verify::Conceal,
    stl::{
        AssetSpec, Attachment, ContractTerms, Details, MediaType, Name, RejectListUrl,
        RicardianContract, Ticker,
    },
    txout::{BlindSeal, CloseMethod, ExplicitSeal},
    validation::{
        ResolveWitness, Scripts, Status, WitnessOrdProvider, WitnessResolverError, WitnessStatus,
    },
};
#[cfg(feature = "esplora")]
use rgbstd::{
    Assign, KnownTransition,
    containers::Consignment,
    contract::SchemaWrapper,
    daggy::Walker,
    txout::TxPtr,
    validation::{OpoutsDagData, Validity, Warning},
};
#[cfg(feature = "esplora")]
use schemata::{IfaWrapper, NiaWrapper, OS_ASSET, OS_INFLATION, OS_REPLACE};
use schemata::{InflatableFungibleAsset, NonInflatableAsset};
use scrypt::{
    Params, Scrypt,
    password_hash::{PasswordHasher, Salt, SaltString, rand_core::OsRng},
};
use serde::de::{self, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use slog::{Drain, Logger, debug, error, info, o, warn};
use strict_encoding::{DecodeError, DeserializeError, FieldName};
use time::OffsetDateTime;
use typenum::consts::U32;

use crate::database::{
    DbAsset, DbAssetActMod, DbAssetTransfer, DbAssetTransferActMod, DbBackupInfo,
    DbBackupInfoActMod, DbBatchTransfer, DbBatchTransferActMod, DbColoring, DbColoringActMod,
    DbMedia, DbMediaActMod, DbPendingWitnessScriptActMod, DbTransfer, DbTransferActMod,
    DbTransferTransportEndpoint, DbTransferTransportEndpointActMod, DbTransportEndpoint,
    DbTransportEndpointActMod, DbTxo,
};
#[cfg(feature = "esplora")]
use crate::database::{DbTxoActMod, DbWalletTransactionActMod};
#[cfg(feature = "esplora")]
use crate::utils::INDEXER_PARALLEL_REQUESTS;
#[cfg(feature = "esplora")]
use crate::utils::{OffchainResolverWasm, WasmResolver};
#[cfg(feature = "esplora")]
use crate::{
    api::proxy::GetConsignmentResponse,
    database::{DbData, LocalRecipient, LocalRecipientData, LocalWitnessData},
    error::IndexerError,
    utils::{INDEXER_STOP_GAP, script_buf_from_recipient_id},
    wallet::{AssignmentsCollection, Indexer},
};
use crate::{
    database::{
        LocalRgbAllocation, LocalTransportEndpoint, LocalUnspent, TransferData,
        enums::{ColoringType, RecipientTypeFull, WalletTransactionType},
    },
    error::InternalError,
    utils::{
        DumbResolver, LOG_FILE, RgbRuntime, adjust_canonicalization, beneficiary_from_script_buf,
        from_str_or_number_mandatory, from_str_or_number_optional, get_account_xpubs,
        get_descriptors, get_descriptors_from_xpubs, load_rgb_runtime, now, parse_address_str,
        setup_logger, str_to_xpub,
    },
    wallet::{Balance, NUM_KNOWN_SCHEMAS, Outpoint, SCHEMA_ID_IFA, SCHEMA_ID_NIA},
};
