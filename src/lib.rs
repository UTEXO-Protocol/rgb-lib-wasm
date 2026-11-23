#![allow(clippy::too_many_arguments)]
#![allow(dead_code, unused_imports)]
#![warn(missing_docs)]

//! A library to manage wallets for RGB assets.
//!
//! ## Wallet
//! The main component of the library is the [`Wallet`].
//!
//! It allows to create and operate an RGB wallet that can issue, send and receive NIA, IFA and
//! BFA assets. The library also manages UTXOs and asset allocations.
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
//! The [`no_wallet`] module provides the objects and helpers that need no wallet, like
//! [`no_wallet::inspect_consignment`]. The wallet APIs use and re-export them.
//!
//! ## Errors
//! Errors are handled with the crate `thiserror`.

#[cfg(all(test, target_arch = "wasm32"))]
extern crate self as rgb_lib_wasm;

pub(crate) mod api;
pub(crate) mod database;
pub(crate) mod encryption;
pub(crate) mod error;
pub mod keys;
pub mod no_wallet;
pub mod utils;
pub mod wallet;

pub use bdk_wallet;
pub use bdk_wallet::bitcoin;
pub use rgbinvoice::RgbTransport;
pub use rgbstd::{
    AssignmentType, ChainNet, ContractId, Genesis, GraphSeal, OpId, Txid as RgbTxid,
    containers::{
        BuilderSeal, ConsignmentExt, Fascia, FileContent, LoadError, SpvProof,
        Transfer as RgbTransfer, WitnessBundle,
    },
    indexers::AnyResolver,
    persistence::UpdateRes,
    schema::SchemaId,
    validation::{ValidationConfig, ValidationError},
    vm::{WitnessOrd, WitnessPos},
};
pub use rgbstd::{
    Opout,
    containers::{ClaimResolverError, LockId, ResolveClaim},
    stl::{BridgeLocation, EvmAddress, EvmContract},
    validation::Claim,
};

pub use crate::{
    database::enums::{
        AssetSchema, Assignment, Encryption, TransferStatus, TransportType, TypeOfAssignment,
        TypeOfTransition, WalletTransactionType,
    },
    error::Error,
    keys::{WitnessVersion, generate_keys, restore_keys},
    utils::BitcoinNetwork,
    wallet::{RecipientType, TransactionType, TransferKind, Wallet},
};

#[cfg(feature = "esplora")]
#[doc(hidden)]
pub use crate::encryption::solve_pow;

#[cfg(test)]
pub(crate) fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[cfg(feature = "esplora")]
use std::{
    cmp::{Ordering, max, min},
    collections::hash_map::{DefaultHasher, Entry},
    hash::Hasher,
    num::NonZeroU32,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
    hash::Hash,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::database::{memory_db::ActiveValue, rgb_store::MemRgbStore};
#[cfg(test)]
use amplify::confinement::NonEmptyVec;
use amplify::{
    Bytes32, Wrapper, bmap,
    confinement::{Confined, U32 as FILE_MAX_LEN},
    s,
};
#[cfg(feature = "esplora")]
use base64::{Engine as _, engine::general_purpose};
use bdk_wallet::bitcoin::Txid;
#[cfg(feature = "esplora")]
use bdk_wallet::bitcoin::{TxMerkleNode, block::Header as BlockHeader};
use bdk_wallet::{
    ChangeSet, KeychainKind, LocalOutput, PersistedWallet, SignOptions, Wallet as BdkWallet,
    bitcoin::{
        Address as BdkAddress, Amount as BdkAmount, BlockHash, Network as BdkNetwork, NetworkKind,
        OutPoint, OutPoint as BdkOutPoint, ScriptBuf, Transaction as BdkTransaction, TxOut,
        bip32::{ChildNumber, DerivationPath, Fingerprint, KeySource, Xpriv, Xpub},
        constants::ChainHash,
        hashes::{Hash as Sha256Hash, sha256},
        psbt::{ExtractTxError, Psbt},
        secp256k1::Secp256k1,
    },
    chain::{CanonicalizationParams, ChainPosition, Merge},
    descriptor::{IntoWalletDescriptor, Segwitv0},
    keys::{
        DerivableKey, DescriptorKey,
        DescriptorKey::{Public, Secret},
        ExtendedKey, GeneratableKey,
        bip39::{Language, Mnemonic, WordCount},
    },
    signer::SignersContainer,
};
#[cfg(feature = "esplora")]
use bdk_wallet::{
    Update,
    bitcoin::{
        TxIn, absolute::LockTime as BdkLockTime, blockdata::fee_rate::FeeRate,
        transaction::Version as BdkTxVersion,
    },
    chain::{
        DescriptorExt,
        spk_client::{FullScanRequest, FullScanResponse, SyncRequest, SyncResponse},
    },
    coin_selection::InsufficientFunds,
};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
use ed25519_dalek::SigningKey;
#[cfg(feature = "esplora")]
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
#[cfg(feature = "esplora")]
use hkdf::Hkdf;
#[cfg(feature = "esplora")]
use hmac::{Mac, SimpleHmac};
use psrgbt::{
    PSBT_MPC_PREFIX, PSBT_OPRET_PREFIX, PSBT_RGB_PREFIX, PSBT_TAPRET_PREFIX, RgbOutExt, RgbPsbtExt,
};
use rand::{Rng, TryRng, distr::Alphanumeric, rngs::SysRng};
#[cfg(feature = "esplora")]
use reqwest::{Client as RestClient, RequestBuilder, header::CONTENT_TYPE, multipart};
use rgbinvoice::{AddressPayload, Beneficiary, RgbInvoice, RgbInvoiceBuilder, XChainNet};
use rgbstd::{
    Allocation, Amount, Assign, Assignments, ExposedSeal, GlobalStateType, Identity,
    KnownTransition, Layer1, Operation, Opout as RgbOpout, OutputSeal, OwnedFraction, Precision,
    Schema, SecretSeal, Transition, TransitionBundle, TransitionType, TypedAssigns,
    containers::{Consignment, UniversalFile, ValidConsignment, ValidContract, ValidTransfer},
    contract::{
        AllocatedState, ContractBuilder, ContractData, FilteredContractState, IssuerWrapper,
        TransitionBuilder,
    },
    info::{ContractInfo, SchemaInfo},
    invoice::{InvoiceState, Pay2Vout},
    persistence::{
        ContractAssignments, ContractStateSnapshot, RetrievedSpvProofs, RgbStore, Stock,
    },
    rgbcore::{
        commit_verify::{Conceal, mpc::Commitment},
        dbc::Proof as _,
    },
    stl::{
        AssetSpec, Attachment, BlockNumber, BurnReason, ContractTerms, Details, MediaType, Name,
        RejectListLocation, RejectListUrl, RicardianContract, Ticker, UnicodeTicker,
    },
    txout::{BlindSeal, CloseMethod, ExplicitSeal, SealTxid, TxPtr, TxoSeal},
    validation::{
        DbcProof, ResolveWitness, SchemaDefinition, SchemaRules, Status as ValidationStatus,
        WitnessOrdProvider, WitnessResolverError, WitnessStatus,
    },
};
#[cfg(test)]
use rgbstd::{RevealedData, RevealedValue, VoidState, assignments::AssignVec};
#[cfg(feature = "esplora")]
use rgbstd::{
    containers::{CheckedClaimResolver, PendingConsignment},
    contract::{LinkError, LinkableSchemaWrapper, SchemaWrapper},
    daggy::NodeIndex,
    validation::{OpsDag, Validity, Warning},
};
#[cfg(feature = "esplora")]
use schemata::{BfaWrapper, IfaWrapper, NiaWrapper};
use schemata::{
    BridgedFungibleAsset, GS_BRIDGE_LOCATION, GS_BURN_REASON, GS_BURNED_ASSET, GS_BURNED_INFLATION,
    GS_DETAILS, GS_ISSUED_SUPPLY, GS_LINKED_FROM_CONTRACT, GS_LINKED_TO_CONTRACT, GS_MAX_SUPPLY,
    GS_NAME, GS_NOMINAL, GS_PRECISION, GS_REJECT_LIST, GS_REJECT_LIST_URL, GS_TERMS, GS_TICKER,
    InflatableFungibleAsset, MS_ALLOWED_INFLATION, MS_BEFORE_BLOCK, NonInflatableAsset, OS_ASSET,
    OS_INFLATION, OS_LINK, OS_MINT, TS_BURN, TS_INFLATION, TS_LINK, TS_MINT, TS_PROPAGATE,
    TS_TRANSFER, TS_TRANSFER_INFLATION, TS_TRANSFER_LINK,
};
use serde::de::{self, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
#[cfg(feature = "esplora")]
use sha2::Sha512;
use sha2::{Digest, Sha256};
use slog::{Drain, Logger, debug, error, info, o, warn};
use strict_encoding::{
    DecodeError, DeserializeError, FieldName, StreamReader, StrictDecode, StrictDeserialize,
    StrictReader, StrictSerialize,
};
use time::OffsetDateTime;
#[cfg(feature = "esplora")]
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};
use zeroize::Zeroizing;

use crate::database::{
    DbAsset, DbAssetActMod, DbAssetTransfer, DbAssetTransferActMod, DbBackupInfo,
    DbBackupInfoActMod, DbBatchTransfer, DbBatchTransferActMod, DbColoring, DbColoringActMod,
    DbMedia, DbMediaActMod, DbPendingWitnessScriptActMod, DbTransfer, DbTransferActMod,
    DbTransferTransportEndpoint, DbTransferTransportEndpointActMod, DbTransportEndpoint,
    DbTransportEndpointActMod, DbTxo,
};
#[cfg(feature = "esplora")]
use crate::database::{DbTxoActMod, DbWalletTransactionActMod};
use crate::no_wallet::{
    Outpoint, RgbBridgeLocationInfo, RgbRejectListInfo, beneficiary_from_script_buf,
    recipient_id_from_script_buf, script_buf_from_recipient_id,
};
#[cfg(feature = "esplora")]
use crate::utils::INDEXER_PARALLEL_REQUESTS;
#[cfg(feature = "esplora")]
use crate::utils::{OffchainResolverWasm, PreFetchResolver, spv_proof_from_esplora};
use crate::utils::{check_psbt_parsable, strip_rgb_data};
#[cfg(feature = "esplora")]
use crate::{
    api::{
        evm::{EvmClient, MintClaimResolver, MintLockStatus},
        proxy_v1::{
            AckTransferData, CONSIGNMENT_GONE_CODES, GetAckResponse, GetConsignmentResponse,
            INVOICE_UNKNOWN_CODES, InvoiceStatus, JsonRpcError, JsonRpcResponse, PostAckResponse,
            PostConsignmentData, ProxyClient, ProxyTransferState, ServerInfoResponse,
            get_ack_message, list_transfers_message, lookup_answer, map_proxy_error,
            post_consignment_message, post_media_message, pow_difficulty_required,
            prepare_ack_auth_message, prepare_invoice_auth_message, transfer_retrieved_message,
        },
        reject_list::RejectListClient,
    },
    database::{DbData, LocalRecipient, LocalRecipientData, LocalWitnessData},
    encryption::{
        decode_hash_hex, decrypt_blob, derive_shared, ed25519_from_seed, encrypt_blob, invoice_id,
        sha256_digest, sign_ed25519, solve_pow as solve_pow_nonce, transfer_id, verify_ed25519,
        verifying_key_from_bytes,
    },
    error::IndexerError,
    utils::{INDEXER_STOP_GAP, proxy_max_media},
    wallet::{AssignmentsCollection, Indexer},
};
use crate::{
    database::{
        LocalRgbAllocation, LocalTransportEndpoint, LocalUnspent, TransferData,
        enums::{ColoringType, RecipientTypeFull, TransportEndpointKind},
    },
    error::InternalError,
    utils::{
        DumbResolver, KEYCHAIN_BTC, LOG_FILE, RgbRuntime, adjust_canonicalization, fault_point,
        from_str_or_number_mandatory, from_str_or_number_optional, get_account_xpubs,
        get_descriptors, get_descriptors_from_xpubs, load_rgb_runtime, now, parse_address_str,
        setup_logger, str_to_xpub,
    },
    wallet::{
        Balance, NUM_KNOWN_SCHEMAS, RGB_GLOBAL_BURN_REASON, RGB_GLOBAL_BURNED_ASSET,
        RGB_GLOBAL_BURNED_INFLATION, RGB_GLOBAL_ISSUED_SUPPLY, RGB_GLOBAL_LINKED_TO_CONTRACT,
        RGB_METADATA_ALLOWED_INFLATION, RGB_METADATA_BEFORE_BLOCK, SCHEMA_ID_BFA, SCHEMA_ID_IFA,
        SCHEMA_ID_NIA,
    },
};

/// Text that replaces a secret in Debug output.
const REDACTED: &str = "[redacted]";

pub(crate) const RGB_STATE_ASSET_OWNER: &str = "assetOwner";
pub(crate) const RGB_STATE_INFLATION_ALLOWANCE: &str = "inflationAllowance";
pub(crate) const RGB_STATE_LINK_RIGHT: &str = "linkRight";
pub(crate) const RGB_STATE_MINT_RIGHT: &str = "mintRight";
