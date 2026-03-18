//! In-memory wallet DB for wasm32: same API as RgbLibDatabase, backed by Vec/HashMap.
#![allow(clippy::derivable_impls)]

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::database::enums::{
    AssetSchema, Assignment, ColoringType, RecipientTypeFull, TransferStatus, TransportType,
    WalletTransactionType,
};
use crate::database::{DbBatchTransferData, LocalRgbAllocation, LocalUnspent};
use crate::error::InternalError;
use crate::wallet::{Balance, Outpoint};

/// Lightweight ActiveValue (mirrors sea_orm::ActiveValue without the dependency).
#[derive(Clone, Debug)]
pub enum ActiveValue<T> {
    Set(T),
    Unchanged(T),
    NotSet,
}

impl<T: Default> Default for ActiveValue<T> {
    fn default() -> Self {
        ActiveValue::NotSet
    }
}

impl<T> ActiveValue<T> {
    pub fn unwrap(self) -> T {
        match self {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v,
            ActiveValue::NotSet => panic!("called unwrap on ActiveValue::NotSet"),
        }
    }
}

/// Extract value from ActiveValue; use default for NotSet.
fn av<T: Default>(v: ActiveValue<T>) -> T {
    match v {
        ActiveValue::Set(x) | ActiveValue::Unchanged(x) => x,
        ActiveValue::NotSet => T::default(),
    }
}

/// Extract value from ActiveValue; use given default for NotSet.
fn av_or<T>(v: ActiveValue<T>, default: T) -> T {
    match v {
        ActiveValue::Set(x) | ActiveValue::Unchanged(x) => x,
        ActiveValue::NotSet => default,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbAsset {
    pub idx: i32,
    pub media_idx: Option<i32>,
    pub id: String,
    pub schema: AssetSchema,
    pub added_at: i64,
    pub details: Option<String>,
    pub initial_supply: String,
    pub name: String,
    pub precision: u8,
    pub ticker: Option<String>,
    pub timestamp: i64,
    pub max_supply: Option<String>,
    pub known_circulating_supply: Option<String>,
    pub reject_list_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DbAssetActMod {
    pub idx: ActiveValue<i32>,
    pub media_idx: ActiveValue<Option<i32>>,
    pub id: ActiveValue<String>,
    pub schema: ActiveValue<AssetSchema>,
    pub added_at: ActiveValue<i64>,
    pub details: ActiveValue<Option<String>>,
    pub initial_supply: ActiveValue<String>,
    pub name: ActiveValue<String>,
    pub precision: ActiveValue<u8>,
    pub ticker: ActiveValue<Option<String>>,
    pub timestamp: ActiveValue<i64>,
    pub max_supply: ActiveValue<Option<String>>,
    pub known_circulating_supply: ActiveValue<Option<String>>,
    pub reject_list_url: ActiveValue<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbAssetTransfer {
    pub idx: i32,
    pub user_driven: bool,
    pub batch_transfer_idx: i32,
    pub asset_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DbAssetTransferActMod {
    pub idx: ActiveValue<i32>,
    pub user_driven: ActiveValue<bool>,
    pub batch_transfer_idx: ActiveValue<i32>,
    pub asset_id: ActiveValue<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbBackupInfo {
    pub idx: i32,
    pub last_backup_timestamp: String,
    pub last_operation_timestamp: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbBackupInfoActMod {
    pub idx: ActiveValue<i32>,
    pub last_backup_timestamp: ActiveValue<String>,
    pub last_operation_timestamp: ActiveValue<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbBatchTransfer {
    pub idx: i32,
    pub txid: Option<String>,
    pub status: TransferStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub expiration: Option<i64>,
    pub min_confirmations: u8,
}

#[derive(Clone, Debug, Default)]
pub struct DbBatchTransferActMod {
    pub idx: ActiveValue<i32>,
    pub txid: ActiveValue<Option<String>>,
    pub status: ActiveValue<TransferStatus>,
    pub created_at: ActiveValue<i64>,
    pub updated_at: ActiveValue<i64>,
    pub expiration: ActiveValue<Option<i64>>,
    pub min_confirmations: ActiveValue<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbColoring {
    pub idx: i32,
    pub txo_idx: i32,
    pub asset_transfer_idx: i32,
    pub r#type: ColoringType,
    pub assignment: Assignment,
}

#[derive(Clone, Debug, Default)]
pub struct DbColoringActMod {
    pub idx: ActiveValue<i32>,
    pub txo_idx: ActiveValue<i32>,
    pub asset_transfer_idx: ActiveValue<i32>,
    pub r#type: ActiveValue<ColoringType>,
    pub assignment: ActiveValue<Assignment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbMedia {
    pub idx: i32,
    pub digest: String,
    pub mime: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbMediaActMod {
    pub idx: ActiveValue<i32>,
    pub digest: ActiveValue<String>,
    pub mime: ActiveValue<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbPendingWitnessScript {
    pub idx: i32,
    pub script: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbPendingWitnessScriptActMod {
    pub idx: ActiveValue<i32>,
    pub script: ActiveValue<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbTransfer {
    pub idx: i32,
    pub asset_transfer_idx: i32,
    pub requested_assignment: Option<Assignment>,
    pub incoming: bool,
    pub recipient_type: Option<RecipientTypeFull>,
    pub recipient_id: Option<String>,
    pub ack: Option<bool>,
    pub invoice_string: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DbTransferActMod {
    pub idx: ActiveValue<i32>,
    pub asset_transfer_idx: ActiveValue<i32>,
    pub requested_assignment: ActiveValue<Option<Assignment>>,
    pub incoming: ActiveValue<bool>,
    pub recipient_type: ActiveValue<Option<RecipientTypeFull>>,
    pub recipient_id: ActiveValue<Option<String>>,
    pub ack: ActiveValue<Option<bool>>,
    pub invoice_string: ActiveValue<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbTransferTransportEndpoint {
    pub idx: i32,
    pub transfer_idx: i32,
    pub transport_endpoint_idx: i32,
    pub used: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DbTransferTransportEndpointActMod {
    pub idx: ActiveValue<i32>,
    pub transfer_idx: ActiveValue<i32>,
    pub transport_endpoint_idx: ActiveValue<i32>,
    pub used: ActiveValue<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbTransportEndpoint {
    pub idx: i32,
    pub transport_type: TransportType,
    pub endpoint: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbTransportEndpointActMod {
    pub idx: ActiveValue<i32>,
    pub transport_type: ActiveValue<TransportType>,
    pub endpoint: ActiveValue<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DbTxo {
    pub idx: i32,
    pub txid: String,
    pub vout: u32,
    pub btc_amount: String,
    pub spent: bool,
    pub exists: bool,
    pub pending_witness: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DbTxoActMod {
    pub idx: ActiveValue<i32>,
    pub txid: ActiveValue<String>,
    pub vout: ActiveValue<u32>,
    pub btc_amount: ActiveValue<String>,
    pub spent: ActiveValue<bool>,
    pub exists: ActiveValue<bool>,
    pub pending_witness: ActiveValue<bool>,
}

impl From<DbTxo> for DbTxoActMod {
    fn from(x: DbTxo) -> DbTxoActMod {
        DbTxoActMod {
            idx: ActiveValue::Unchanged(x.idx),
            txid: ActiveValue::Unchanged(x.txid),
            vout: ActiveValue::Unchanged(x.vout),
            btc_amount: ActiveValue::Unchanged(x.btc_amount),
            spent: ActiveValue::Unchanged(x.spent),
            exists: ActiveValue::Unchanged(x.exists),
            pending_witness: ActiveValue::Unchanged(x.pending_witness),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbWalletTransaction {
    pub idx: i32,
    pub txid: String,
    pub r#type: WalletTransactionType,
}

#[derive(Clone, Debug, Default)]
pub struct DbWalletTransactionActMod {
    pub idx: ActiveValue<i32>,
    pub txid: ActiveValue<String>,
    pub r#type: ActiveValue<WalletTransactionType>,
}

impl Default for AssetSchema {
    fn default() -> Self {
        AssetSchema::Nia
    }
}

impl Default for TransferStatus {
    fn default() -> Self {
        TransferStatus::WaitingCounterparty
    }
}

impl Default for ColoringType {
    fn default() -> Self {
        ColoringType::Receive
    }
}

impl Default for TransportType {
    fn default() -> Self {
        TransportType::JsonRpc
    }
}

impl Default for WalletTransactionType {
    fn default() -> Self {
        WalletTransactionType::CreateUtxos
    }
}

impl Default for Assignment {
    fn default() -> Self {
        Assignment::NonFungible
    }
}

impl DbAssetActMod {
    pub fn try_into_model(self) -> Result<DbAsset, ()> {
        Ok(DbAsset {
            idx: av(self.idx),
            media_idx: av(self.media_idx),
            id: av(self.id),
            schema: av(self.schema),
            added_at: av(self.added_at),
            details: av(self.details),
            initial_supply: av(self.initial_supply),
            name: av(self.name),
            precision: av(self.precision),
            ticker: av(self.ticker),
            timestamp: av(self.timestamp),
            max_supply: av(self.max_supply),
            known_circulating_supply: av(self.known_circulating_supply),
            reject_list_url: av(self.reject_list_url),
        })
    }
}

impl From<DbBatchTransfer> for DbBatchTransferActMod {
    fn from(m: DbBatchTransfer) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            txid: ActiveValue::Unchanged(m.txid),
            status: ActiveValue::Unchanged(m.status),
            created_at: ActiveValue::Unchanged(m.created_at),
            updated_at: ActiveValue::Unchanged(m.updated_at),
            expiration: ActiveValue::Unchanged(m.expiration),
            min_confirmations: ActiveValue::Unchanged(m.min_confirmations),
        }
    }
}

impl From<DbBackupInfo> for DbBackupInfoActMod {
    fn from(m: DbBackupInfo) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            last_backup_timestamp: ActiveValue::Unchanged(m.last_backup_timestamp),
            last_operation_timestamp: ActiveValue::Unchanged(m.last_operation_timestamp),
        }
    }
}

impl From<DbAssetTransfer> for DbAssetTransferActMod {
    fn from(m: DbAssetTransfer) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            user_driven: ActiveValue::Unchanged(m.user_driven),
            batch_transfer_idx: ActiveValue::Unchanged(m.batch_transfer_idx),
            asset_id: ActiveValue::Unchanged(m.asset_id),
        }
    }
}

impl From<DbTransfer> for DbTransferActMod {
    fn from(m: DbTransfer) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            asset_transfer_idx: ActiveValue::Unchanged(m.asset_transfer_idx),
            requested_assignment: ActiveValue::Unchanged(m.requested_assignment),
            incoming: ActiveValue::Unchanged(m.incoming),
            recipient_type: ActiveValue::Unchanged(m.recipient_type),
            recipient_id: ActiveValue::Unchanged(m.recipient_id),
            ack: ActiveValue::Unchanged(m.ack),
            invoice_string: ActiveValue::Unchanged(m.invoice_string),
        }
    }
}

impl From<DbTransferTransportEndpoint> for DbTransferTransportEndpointActMod {
    fn from(m: DbTransferTransportEndpoint) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            transfer_idx: ActiveValue::Unchanged(m.transfer_idx),
            transport_endpoint_idx: ActiveValue::Unchanged(m.transport_endpoint_idx),
            used: ActiveValue::Unchanged(m.used),
        }
    }
}

impl From<DbAsset> for DbAssetActMod {
    fn from(m: DbAsset) -> Self {
        Self {
            idx: ActiveValue::Unchanged(m.idx),
            media_idx: ActiveValue::Unchanged(m.media_idx),
            id: ActiveValue::Unchanged(m.id),
            schema: ActiveValue::Unchanged(m.schema),
            added_at: ActiveValue::Unchanged(m.added_at),
            details: ActiveValue::Unchanged(m.details),
            initial_supply: ActiveValue::Unchanged(m.initial_supply),
            name: ActiveValue::Unchanged(m.name),
            precision: ActiveValue::Unchanged(m.precision),
            ticker: ActiveValue::Unchanged(m.ticker),
            timestamp: ActiveValue::Unchanged(m.timestamp),
            max_supply: ActiveValue::Unchanged(m.max_supply),
            known_circulating_supply: ActiveValue::Unchanged(m.known_circulating_supply),
            reject_list_url: ActiveValue::Unchanged(m.reject_list_url),
        }
    }
}

/// In-memory store: one Vec per entity, next_id per table for insert idx.
/// RefCell for interior mutability so methods can take &self (Arc<Backend>).
#[derive(Clone, Serialize, Deserialize)]
/// In-memory database replacing SeaORM/SQLite for WASM targets.
///
/// **Design constraint:** All fields use `RefCell` for interior mutability. Methods must NOT
/// call other `InMemoryDb` methods while holding a borrow — this causes a runtime panic.
/// WASM is single-threaded so concurrent access is not a concern, but nested borrows within
/// a single call chain will panic. If future code needs nested DB access, refactor to
/// clone-on-read or use a different interior mutability strategy.
pub struct InMemoryDb {
    txos: RefCell<Vec<DbTxo>>,
    next_txo_idx: RefCell<i32>,
    media: RefCell<Vec<DbMedia>>,
    next_media_idx: RefCell<i32>,
    assets: RefCell<Vec<DbAsset>>,
    next_asset_idx: RefCell<i32>,
    batch_transfers: RefCell<Vec<DbBatchTransfer>>,
    next_batch_transfer_idx: RefCell<i32>,
    asset_transfers: RefCell<Vec<DbAssetTransfer>>,
    next_asset_transfer_idx: RefCell<i32>,
    colorings: RefCell<Vec<DbColoring>>,
    next_coloring_idx: RefCell<i32>,
    transfers: RefCell<Vec<DbTransfer>>,
    next_transfer_idx: RefCell<i32>,
    transport_endpoints: RefCell<Vec<DbTransportEndpoint>>,
    next_transport_endpoint_idx: RefCell<i32>,
    transfer_transport_endpoints: RefCell<Vec<DbTransferTransportEndpoint>>,
    next_transfer_transport_endpoint_idx: RefCell<i32>,
    wallet_transactions: RefCell<Vec<DbWalletTransaction>>,
    next_wallet_transaction_idx: RefCell<i32>,
    pending_witness_scripts: RefCell<Vec<DbPendingWitnessScript>>,
    next_pending_witness_script_idx: RefCell<i32>,
    backup_info: RefCell<Option<DbBackupInfo>>,
    next_backup_info_idx: RefCell<i32>,
}

impl InMemoryDb {
    pub fn new() -> Self {
        Self {
            txos: RefCell::new(Vec::new()),
            next_txo_idx: RefCell::new(1),
            media: RefCell::new(Vec::new()),
            next_media_idx: RefCell::new(1),
            assets: RefCell::new(Vec::new()),
            next_asset_idx: RefCell::new(1),
            batch_transfers: RefCell::new(Vec::new()),
            next_batch_transfer_idx: RefCell::new(1),
            asset_transfers: RefCell::new(Vec::new()),
            next_asset_transfer_idx: RefCell::new(1),
            colorings: RefCell::new(Vec::new()),
            next_coloring_idx: RefCell::new(1),
            transfers: RefCell::new(Vec::new()),
            next_transfer_idx: RefCell::new(1),
            transport_endpoints: RefCell::new(Vec::new()),
            next_transport_endpoint_idx: RefCell::new(1),
            transfer_transport_endpoints: RefCell::new(Vec::new()),
            next_transfer_transport_endpoint_idx: RefCell::new(1),
            wallet_transactions: RefCell::new(Vec::new()),
            next_wallet_transaction_idx: RefCell::new(1),
            pending_witness_scripts: RefCell::new(Vec::new()),
            next_pending_witness_script_idx: RefCell::new(1),
            backup_info: RefCell::new(None),
            next_backup_info_idx: RefCell::new(1),
        }
    }

    pub(crate) fn set_asset(&self, a: DbAssetActMod) -> Result<i32, InternalError> {
        let idx = *self.next_asset_idx.borrow();
        *self.next_asset_idx.borrow_mut() += 1;
        let row = DbAsset {
            idx,
            media_idx: av(a.media_idx),
            id: av(a.id),
            schema: av_or(a.schema, AssetSchema::Nia),
            added_at: av(a.added_at),
            details: av(a.details),
            initial_supply: av(a.initial_supply),
            name: av(a.name),
            precision: av(a.precision),
            ticker: av(a.ticker),
            timestamp: av(a.timestamp),
            max_supply: av(a.max_supply),
            known_circulating_supply: av(a.known_circulating_supply),
            reject_list_url: av(a.reject_list_url),
        };
        self.assets.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_asset_transfer(
        &self,
        a: DbAssetTransferActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_asset_transfer_idx.borrow();
        *self.next_asset_transfer_idx.borrow_mut() += 1;
        let row = DbAssetTransfer {
            idx,
            user_driven: av(a.user_driven),
            batch_transfer_idx: av(a.batch_transfer_idx),
            asset_id: av(a.asset_id),
        };
        self.asset_transfers.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_backup_info(&self, b: DbBackupInfoActMod) -> Result<i32, InternalError> {
        let idx = *self.next_backup_info_idx.borrow();
        *self.next_backup_info_idx.borrow_mut() += 1;
        let row = DbBackupInfo {
            idx,
            last_backup_timestamp: av(b.last_backup_timestamp),
            last_operation_timestamp: av(b.last_operation_timestamp),
        };
        *self.backup_info.borrow_mut() = Some(row);
        Ok(idx)
    }

    pub(crate) fn set_batch_transfer(
        &self,
        b: DbBatchTransferActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_batch_transfer_idx.borrow();
        *self.next_batch_transfer_idx.borrow_mut() += 1;
        let created = av(b.created_at);
        let row = DbBatchTransfer {
            idx,
            txid: av(b.txid),
            status: av_or(b.status, TransferStatus::Settled),
            created_at: created,
            updated_at: av_or(b.updated_at, created),
            expiration: av(b.expiration),
            min_confirmations: av(b.min_confirmations),
        };
        self.batch_transfers.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_coloring(&self, c: DbColoringActMod) -> Result<i32, InternalError> {
        let idx = *self.next_coloring_idx.borrow();
        *self.next_coloring_idx.borrow_mut() += 1;
        let row = DbColoring {
            idx,
            txo_idx: av(c.txo_idx),
            asset_transfer_idx: av(c.asset_transfer_idx),
            r#type: av_or(c.r#type, ColoringType::Receive),
            assignment: av_or(c.assignment, Assignment::Fungible(0)),
        };
        self.colorings.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_media(&self, m: DbMediaActMod) -> Result<i32, InternalError> {
        let idx = *self.next_media_idx.borrow();
        *self.next_media_idx.borrow_mut() += 1;
        let row = DbMedia {
            idx,
            digest: av(m.digest),
            mime: av(m.mime),
        };
        self.media.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_pending_witness_script(
        &self,
        p: DbPendingWitnessScriptActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_pending_witness_script_idx.borrow();
        *self.next_pending_witness_script_idx.borrow_mut() += 1;
        let row = DbPendingWitnessScript {
            idx,
            script: av(p.script),
        };
        self.pending_witness_scripts.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_transport_endpoint(
        &self,
        t: DbTransportEndpointActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_transport_endpoint_idx.borrow();
        *self.next_transport_endpoint_idx.borrow_mut() += 1;
        let row = DbTransportEndpoint {
            idx,
            transport_type: av_or(t.transport_type, TransportType::JsonRpc),
            endpoint: av(t.endpoint),
        };
        self.transport_endpoints.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_transfer(&self, t: DbTransferActMod) -> Result<i32, InternalError> {
        let idx = *self.next_transfer_idx.borrow();
        *self.next_transfer_idx.borrow_mut() += 1;
        let row = DbTransfer {
            idx,
            asset_transfer_idx: av(t.asset_transfer_idx),
            requested_assignment: av(t.requested_assignment),
            incoming: av(t.incoming),
            recipient_type: av(t.recipient_type),
            recipient_id: av(t.recipient_id),
            ack: av(t.ack),
            invoice_string: av(t.invoice_string),
        };
        self.transfers.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_transfer_transport_endpoint(
        &self,
        t: DbTransferTransportEndpointActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_transfer_transport_endpoint_idx.borrow();
        *self.next_transfer_transport_endpoint_idx.borrow_mut() += 1;
        let row = DbTransferTransportEndpoint {
            idx,
            transfer_idx: av(t.transfer_idx),
            transport_endpoint_idx: av(t.transport_endpoint_idx),
            used: av_or(t.used, false),
        };
        self.transfer_transport_endpoints.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_txo(&self, t: DbTxoActMod) -> Result<i32, InternalError> {
        let txid = av(t.txid.clone());
        let vout = av(t.vout);
        let existing_pos = self
            .txos
            .borrow()
            .iter()
            .position(|r| r.txid == txid && r.vout == vout);
        if let Some(pos) = existing_pos {
            let exists = av(t.exists);
            let btc_amount = av(t.btc_amount.clone());
            let mut txos = self.txos.borrow_mut();
            if exists {
                txos[pos].exists = exists;
            }
            if btc_amount != "0" {
                txos[pos].btc_amount = btc_amount;
            }
            return Ok(txos[pos].idx);
        }
        let idx = *self.next_txo_idx.borrow();
        *self.next_txo_idx.borrow_mut() += 1;
        let row = DbTxo {
            idx,
            txid,
            vout,
            btc_amount: av(t.btc_amount),
            spent: av(t.spent),
            exists: av(t.exists),
            pending_witness: av(t.pending_witness),
        };
        self.txos.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_wallet_transaction(
        &self,
        w: DbWalletTransactionActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_wallet_transaction_idx.borrow();
        *self.next_wallet_transaction_idx.borrow_mut() += 1;
        let row = DbWalletTransaction {
            idx,
            txid: av(w.txid),
            r#type: av_or(w.r#type, WalletTransactionType::CreateUtxos),
        };
        self.wallet_transactions.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn update_transfer(
        &self,
        t: &mut DbTransferActMod,
    ) -> Result<DbTransfer, InternalError> {
        let idx = av(t.idx.clone());
        let mut transfers = self.transfers.borrow_mut();
        let pos = transfers
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut transfers[pos];
        if let ActiveValue::Set(v) = t.asset_transfer_idx {
            row.asset_transfer_idx = v;
        }
        if let ActiveValue::Set(v) = &t.requested_assignment {
            row.requested_assignment = v.clone();
        }
        if let ActiveValue::Set(v) = t.incoming {
            row.incoming = v;
        }
        if let ActiveValue::Set(v) = &t.recipient_type {
            row.recipient_type = v.clone();
        }
        if let ActiveValue::Set(v) = &t.recipient_id {
            row.recipient_id = v.clone();
        }
        if let ActiveValue::Set(v) = t.ack {
            row.ack = v;
        }
        if let ActiveValue::Set(v) = &t.invoice_string {
            row.invoice_string = v.clone();
        }
        Ok(row.clone())
    }

    pub(crate) fn update_asset(&self, a: &mut DbAssetActMod) -> Result<DbAsset, InternalError> {
        let idx = av(a.idx.clone());
        let mut assets = self.assets.borrow_mut();
        let pos = assets
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut assets[pos];
        if let ActiveValue::Set(v) = &a.media_idx {
            row.media_idx = *v;
        }
        if let ActiveValue::Set(v) = &a.details {
            row.details = v.clone();
        }
        if let ActiveValue::Set(v) = &a.initial_supply {
            row.initial_supply = v.clone();
        }
        if let ActiveValue::Set(v) = &a.name {
            row.name = v.clone();
        }
        if let ActiveValue::Set(v) = a.precision {
            row.precision = v;
        }
        if let ActiveValue::Set(v) = &a.ticker {
            row.ticker = v.clone();
        }
        if let ActiveValue::Set(v) = &a.max_supply {
            row.max_supply = v.clone();
        }
        if let ActiveValue::Set(v) = &a.known_circulating_supply {
            row.known_circulating_supply = v.clone();
        }
        if let ActiveValue::Set(v) = &a.reject_list_url {
            row.reject_list_url = v.clone();
        }
        Ok(row.clone())
    }

    pub(crate) fn update_asset_transfer(
        &self,
        a: &mut DbAssetTransferActMod,
    ) -> Result<DbAssetTransfer, InternalError> {
        let idx = av(a.idx.clone());
        let mut asset_transfers = self.asset_transfers.borrow_mut();
        let pos = asset_transfers
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut asset_transfers[pos];
        if let ActiveValue::Set(v) = a.user_driven {
            row.user_driven = v;
        }
        if let ActiveValue::Set(v) = a.batch_transfer_idx {
            row.batch_transfer_idx = v;
        }
        if let ActiveValue::Set(v) = &a.asset_id {
            row.asset_id = v.clone();
        }
        Ok(row.clone())
    }

    pub(crate) fn update_backup_info(
        &self,
        b: &mut DbBackupInfoActMod,
    ) -> Result<DbBackupInfo, InternalError> {
        if let Some(row) = &mut *self.backup_info.borrow_mut() {
            if let ActiveValue::Set(v) = &b.last_backup_timestamp {
                row.last_backup_timestamp = v.clone();
            }
            if let ActiveValue::Set(v) = &b.last_operation_timestamp {
                row.last_operation_timestamp = v.clone();
            }
            Ok(row.clone())
        } else {
            Err(InternalError::Unexpected)
        }
    }

    pub(crate) fn update_batch_transfer(
        &self,
        b: &mut DbBatchTransferActMod,
    ) -> Result<DbBatchTransfer, InternalError> {
        let idx = av(b.idx.clone());
        let mut batch_transfers = self.batch_transfers.borrow_mut();
        let pos = batch_transfers
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut batch_transfers[pos];
        if let ActiveValue::Set(v) = &b.txid {
            row.txid = v.clone();
        }
        if let ActiveValue::Set(v) = b.status {
            row.status = v;
        }
        if let ActiveValue::Set(v) = b.updated_at {
            row.updated_at = v;
        }
        if let ActiveValue::Set(v) = &b.expiration {
            row.expiration = *v;
        }
        Ok(row.clone())
    }

    pub(crate) fn update_transfer_transport_endpoint(
        &self,
        t: &mut DbTransferTransportEndpointActMod,
    ) -> Result<DbTransferTransportEndpoint, InternalError> {
        let idx = av(t.idx.clone());
        let mut tte = self.transfer_transport_endpoints.borrow_mut();
        let pos = tte
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut tte[pos];
        if let ActiveValue::Set(v) = t.used {
            row.used = v;
        }
        Ok(row.clone())
    }

    pub(crate) fn update_txo(&self, t: DbTxoActMod) -> Result<(), InternalError> {
        let idx = av(t.idx);
        let mut txos = self.txos.borrow_mut();
        let pos = txos
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Unexpected)?;
        let row = &mut txos[pos];
        if let ActiveValue::Set(v) = &t.btc_amount {
            row.btc_amount = v.clone();
        }
        if let ActiveValue::Set(v) = t.spent {
            row.spent = v;
        }
        if let ActiveValue::Set(v) = t.exists {
            row.exists = v;
        }
        if let ActiveValue::Set(v) = t.pending_witness {
            row.pending_witness = v;
        }
        Ok(())
    }

    pub(crate) fn del_backup_info(&self) -> Result<(), InternalError> {
        *self.backup_info.borrow_mut() = None;
        Ok(())
    }

    pub(crate) fn del_batch_transfer(
        &self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<(), InternalError> {
        // Cascade: BatchTransfer → AssetTransfer → Transfer → TransferTransportEndpoint
        let asset_transfer_idxs: Vec<i32> = self
            .asset_transfers
            .borrow()
            .iter()
            .filter(|at| at.batch_transfer_idx == batch_transfer.idx)
            .map(|at| at.idx)
            .collect();
        let transfer_idxs: Vec<i32> = self
            .transfers
            .borrow()
            .iter()
            .filter(|t| asset_transfer_idxs.contains(&t.asset_transfer_idx))
            .map(|t| t.idx)
            .collect();
        {
            let mut ttes = self.transfer_transport_endpoints.borrow_mut();
            ttes.retain(|tte| !transfer_idxs.contains(&tte.transfer_idx));
            ttes.shrink_to_fit();
        }
        {
            let mut transfers = self.transfers.borrow_mut();
            transfers.retain(|t| !asset_transfer_idxs.contains(&t.asset_transfer_idx));
            transfers.shrink_to_fit();
        }
        {
            let mut ats = self.asset_transfers.borrow_mut();
            ats.retain(|at| at.batch_transfer_idx != batch_transfer.idx);
            ats.shrink_to_fit();
        }
        {
            let mut bts = self.batch_transfers.borrow_mut();
            bts.retain(|bt| bt.idx != batch_transfer.idx);
            bts.shrink_to_fit();
        }
        Ok(())
    }

    pub(crate) fn del_coloring(&self, asset_transfer_idx: i32) -> Result<(), InternalError> {
        self.colorings
            .borrow_mut()
            .retain(|c| c.asset_transfer_idx != asset_transfer_idx);
        Ok(())
    }

    pub(crate) fn del_pending_witness_script(&self, script: String) -> Result<(), InternalError> {
        self.pending_witness_scripts
            .borrow_mut()
            .retain(|p| p.script != script);
        Ok(())
    }

    pub(crate) fn del_txo(&self, idx: i32) -> Result<(), InternalError> {
        {
            let mut colorings = self.colorings.borrow_mut();
            colorings.retain(|c| c.txo_idx != idx);
            colorings.shrink_to_fit();
        }
        {
            let mut txos = self.txos.borrow_mut();
            txos.retain(|t| t.idx != idx);
            txos.shrink_to_fit();
        }
        Ok(())
    }

    pub(crate) fn get_asset(&self, asset_id: String) -> Result<Option<DbAsset>, InternalError> {
        Ok(self
            .assets
            .borrow()
            .iter()
            .find(|a| a.id == asset_id)
            .cloned())
    }

    pub(crate) fn get_backup_info(&self) -> Result<Option<DbBackupInfo>, InternalError> {
        Ok(self.backup_info.borrow().clone())
    }

    pub(crate) fn get_media(&self, media_idx: i32) -> Result<Option<DbMedia>, InternalError> {
        Ok(self
            .media
            .borrow()
            .iter()
            .find(|m| m.idx == media_idx)
            .cloned())
    }

    pub(crate) fn get_media_by_digest(
        &self,
        digest: String,
    ) -> Result<Option<DbMedia>, InternalError> {
        Ok(self
            .media
            .borrow()
            .iter()
            .find(|m| m.digest == digest)
            .cloned())
    }

    pub(crate) fn get_transport_endpoint(
        &self,
        endpoint: String,
    ) -> Result<Option<DbTransportEndpoint>, InternalError> {
        Ok(self
            .transport_endpoints
            .borrow()
            .iter()
            .find(|t| t.endpoint == endpoint)
            .cloned())
    }

    pub(crate) fn get_txo(&self, outpoint: &Outpoint) -> Result<Option<DbTxo>, InternalError> {
        Ok(self
            .txos
            .borrow()
            .iter()
            .find(|t| t.txid == outpoint.txid && t.vout == outpoint.vout)
            .cloned())
    }

    pub(crate) fn iter_assets(&self) -> Result<Vec<DbAsset>, InternalError> {
        Ok(self.assets.borrow().clone())
    }

    pub(crate) fn iter_asset_transfers(&self) -> Result<Vec<DbAssetTransfer>, InternalError> {
        Ok(self.asset_transfers.borrow().clone())
    }

    pub(crate) fn iter_batch_transfers(&self) -> Result<Vec<DbBatchTransfer>, InternalError> {
        Ok(self.batch_transfers.borrow().clone())
    }

    pub(crate) fn iter_colorings(&self) -> Result<Vec<DbColoring>, InternalError> {
        Ok(self.colorings.borrow().clone())
    }

    pub(crate) fn iter_media(&self) -> Result<Vec<DbMedia>, InternalError> {
        Ok(self.media.borrow().clone())
    }

    pub(crate) fn iter_pending_witness_scripts(
        &self,
    ) -> Result<Vec<DbPendingWitnessScript>, InternalError> {
        Ok(self.pending_witness_scripts.borrow().clone())
    }

    pub(crate) fn iter_transfers(&self) -> Result<Vec<DbTransfer>, InternalError> {
        Ok(self.transfers.borrow().clone())
    }

    pub(crate) fn iter_txos(&self) -> Result<Vec<DbTxo>, InternalError> {
        Ok(self.txos.borrow().clone())
    }

    pub(crate) fn iter_wallet_transactions(
        &self,
    ) -> Result<Vec<DbWalletTransaction>, InternalError> {
        Ok(self.wallet_transactions.borrow().clone())
    }

    pub(crate) fn get_transfer_transport_endpoints_data(
        &self,
        transfer_idx: i32,
    ) -> Result<Vec<(DbTransferTransportEndpoint, DbTransportEndpoint)>, InternalError> {
        let mut out = Vec::new();
        for tte in self.transfer_transport_endpoints.borrow().iter() {
            if tte.transfer_idx != transfer_idx {
                continue;
            }
            let te = self
                .transport_endpoints
                .borrow()
                .iter()
                .find(|e| e.idx == tte.transport_endpoint_idx)
                .cloned()
                .ok_or(InternalError::Unexpected)?;
            out.push((tte.clone(), te));
        }
        out.sort_by_key(|(tte, _)| tte.idx);
        Ok(out)
    }

    pub(crate) fn get_db_data(
        &self,
        empty_transfers: bool,
    ) -> Result<super::DbData, InternalError> {
        let batch_transfers = self.iter_batch_transfers()?;
        let asset_transfers = self.iter_asset_transfers()?;
        let colorings = self.iter_colorings()?;
        let transfers = if empty_transfers {
            vec![]
        } else {
            self.iter_transfers()?
        };
        let txos = self.iter_txos()?;
        Ok(super::DbData {
            batch_transfers,
            asset_transfers,
            transfers,
            colorings,
            txos,
        })
    }

    pub(crate) fn get_unspent_txos(&self, txos: Vec<DbTxo>) -> Result<Vec<DbTxo>, InternalError> {
        let txos = if txos.is_empty() {
            self.iter_txos()?
        } else {
            txos
        };
        Ok(txos.into_iter().filter(|t| !t.spent).collect())
    }

    pub(crate) fn get_asset_balance(
        &self,
        asset_id: String,
        transfers: Option<Vec<DbTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        colorings: Option<Vec<DbColoring>>,
        txos: Option<Vec<DbTxo>>,
    ) -> Result<Balance, Error> {
        let batch_transfers =
            batch_transfers.unwrap_or_else(|| self.iter_batch_transfers().unwrap_or_default());
        let asset_transfers =
            asset_transfers.unwrap_or_else(|| self.iter_asset_transfers().unwrap_or_default());
        let transfers = transfers.unwrap_or_else(|| self.iter_transfers().unwrap_or_default());
        let colorings = colorings.unwrap_or_else(|| self.iter_colorings().unwrap_or_default());
        let txos = txos.unwrap_or_else(|| self.iter_txos().unwrap_or_default());

        let txos_allocations = self.get_rgb_allocations(
            txos,
            Some(colorings),
            Some(batch_transfers.clone()),
            Some(asset_transfers.clone()),
            Some(transfers.clone()),
        )?;

        let mut allocations: Vec<LocalRgbAllocation> = vec![];
        txos_allocations
            .iter()
            .for_each(|u| allocations.extend(u.rgb_allocations.clone()));
        let ass_allocations: Vec<LocalRgbAllocation> = allocations
            .into_iter()
            .filter(|a| a.asset_id == Some(asset_id.clone()))
            .collect();

        let settled: u64 = ass_allocations
            .iter()
            .filter(|a| a.settled())
            .map(|a| a.assignment.main_amount())
            .sum();

        let mut ass_pending_incoming: u64 = ass_allocations
            .iter()
            .filter(|a| !a.txo_spent && a.incoming && a.status.pending())
            .map(|a| a.assignment.main_amount())
            .sum();
        let witness_pending: u64 = transfers
            .iter()
            .filter(|t| {
                t.incoming && matches!(t.recipient_type, Some(RecipientTypeFull::Witness { .. }))
            })
            .filter_map(
                |t| match t.related_transfers(&asset_transfers, &batch_transfers) {
                    Ok((at, bt)) => {
                        if bt.status.waiting_confirmations() {
                            if at.asset_id.as_deref() != Some(asset_id.as_str()) {
                                return None;
                            }
                            Some(Ok(t
                                .requested_assignment
                                .as_ref()
                                .map(|a| a.main_amount())
                                .unwrap_or(0)))
                        } else {
                            None
                        }
                    }
                    Err(e) => Some(Err(e)),
                },
            )
            .collect::<Result<Vec<u64>, InternalError>>()?
            .iter()
            .sum();
        ass_pending_incoming += witness_pending;
        let ass_pending_outgoing: u64 = ass_allocations
            .iter()
            .filter(|a| !a.incoming && a.status.pending())
            .map(|a| a.assignment.main_amount())
            .sum();
        let ass_pending: i128 = ass_pending_incoming as i128 - ass_pending_outgoing as i128;

        let future = settled as i128 + ass_pending;

        let unspendable: u64 = txos_allocations
            .iter()
            .filter(|u| {
                let unspent_with_pending = !u.utxo.spent
                    && (u.rgb_allocations.iter().any(|a| {
                        (!a.incoming && !a.status.failed()) || (a.incoming && a.status.pending())
                    }) || u.pending_blinded > 0);
                let spent_waiting = u.utxo.spent
                    && u.rgb_allocations
                        .iter()
                        .any(|a| !a.incoming && a.status.waiting_confirmations());
                unspent_with_pending || spent_waiting
            })
            .map(|u| {
                u.rgb_allocations
                    .iter()
                    .filter(|a| a.asset_id == Some(asset_id.clone()) && a.settled())
                    .map(|a| a.assignment.main_amount())
                    .sum::<u64>()
            })
            .sum();

        let spendable = settled.saturating_sub(unspendable);

        Ok(Balance {
            settled,
            future: future as u64,
            spendable,
        })
    }

    pub(crate) fn get_asset_ids(&self) -> Result<Vec<String>, InternalError> {
        Ok(self.assets.borrow().iter().map(|a| a.id.clone()).collect())
    }

    pub(crate) fn check_asset_exists(&self, asset_id: String) -> Result<DbAsset, Error> {
        match self.get_asset(asset_id.clone())? {
            Some(a) => Ok(a),
            None => Err(Error::AssetNotFound { asset_id }),
        }
    }

    pub(crate) fn get_batch_transfer_or_fail(
        &self,
        idx: i32,
        batch_transfers: &[DbBatchTransfer],
    ) -> Result<DbBatchTransfer, Error> {
        batch_transfers
            .iter()
            .find(|t| t.idx == idx)
            .cloned()
            .ok_or(Error::BatchTransferNotFound { idx })
    }

    pub(crate) fn get_incoming_transfer(
        &self,
        batch_transfer_data: &DbBatchTransferData,
    ) -> Result<(DbAssetTransfer, DbTransfer), Error> {
        let ad = batch_transfer_data
            .asset_transfers_data
            .first()
            .ok_or(Error::Internal {
                details: "batch transfer has no asset transfers".to_string(),
            })?;
        let transfer = ad.transfers.first().ok_or(Error::Internal {
            details: "asset transfer has no transfers".to_string(),
        })?;
        Ok((ad.asset_transfer.clone(), transfer.clone()))
    }

    fn _get_utxo_allocations(
        &self,
        utxo: &DbTxo,
        colorings: Vec<DbColoring>,
        asset_transfers: Vec<DbAssetTransfer>,
        batch_transfers: Vec<DbBatchTransfer>,
    ) -> Result<Vec<LocalRgbAllocation>, Error> {
        let utxo_colorings: Vec<&DbColoring> =
            colorings.iter().filter(|c| c.txo_idx == utxo.idx).collect();

        let mut allocations = Vec::new();
        for c in utxo_colorings {
            let Some(asset_transfer) = asset_transfers
                .iter()
                .find(|t| t.idx == c.asset_transfer_idx)
            else {
                continue;
            };
            let Some(batch_transfer) = batch_transfers
                .iter()
                .find(|t| asset_transfer.batch_transfer_idx == t.idx)
            else {
                continue;
            };

            allocations.push(LocalRgbAllocation {
                asset_id: asset_transfer.asset_id.clone(),
                assignment: c.assignment.clone(),
                status: batch_transfer.status,
                incoming: c.incoming(),
                txo_spent: utxo.spent,
            });
        }
        Ok(allocations)
    }

    pub(crate) fn get_rgb_allocations(
        &self,
        utxos: Vec<DbTxo>,
        colorings: Option<Vec<DbColoring>>,
        batch_transfers: Option<Vec<DbBatchTransfer>>,
        asset_transfers: Option<Vec<DbAssetTransfer>>,
        transfers: Option<Vec<DbTransfer>>,
    ) -> Result<Vec<LocalUnspent>, Error> {
        let batch_transfers =
            batch_transfers.unwrap_or_else(|| self.iter_batch_transfers().unwrap_or_default());
        let asset_transfers =
            asset_transfers.unwrap_or_else(|| self.iter_asset_transfers().unwrap_or_default());
        let colorings = colorings.unwrap_or_else(|| self.iter_colorings().unwrap_or_default());
        let transfers = transfers.unwrap_or_else(|| self.iter_transfers().unwrap_or_default());

        let pending_blinded_utxos = transfers
            .iter()
            .filter_map(|t| match (&t.recipient_type, t.incoming) {
                (Some(RecipientTypeFull::Blind { unblinded_utxo }), true) => t
                    .related_transfers(&asset_transfers, &batch_transfers)
                    .ok()
                    .filter(|(_, bt)| bt.status.waiting_counterparty())
                    .map(|_| unblinded_utxo),
                _ => None,
            })
            .fold(HashMap::new(), |mut acc, utxo| {
                *acc.entry(utxo).or_insert(0) += 1;
                acc
            });

        utxos
            .iter()
            .map(|t| {
                Ok(LocalUnspent {
                    utxo: t.clone(),
                    rgb_allocations: self._get_utxo_allocations(
                        t,
                        colorings.clone(),
                        asset_transfers.clone(),
                        batch_transfers.clone(),
                    )?,
                    pending_blinded: *pending_blinded_utxos.get(&t.outpoint()).unwrap_or(&0),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> InMemoryDb {
        InMemoryDb::new()
    }

    fn make_asset_mod(id: &str) -> DbAssetActMod {
        DbAssetActMod {
            id: ActiveValue::Set(id.to_string()),
            schema: ActiveValue::Set(AssetSchema::Nia),
            added_at: ActiveValue::Set(1000),
            name: ActiveValue::Set("Test Asset".to_string()),
            precision: ActiveValue::Set(8),
            initial_supply: ActiveValue::Set("1000".to_string()),
            timestamp: ActiveValue::Set(1000),
            ..Default::default()
        }
    }

    fn make_batch_transfer_mod(status: TransferStatus) -> DbBatchTransferActMod {
        DbBatchTransferActMod {
            status: ActiveValue::Set(status),
            created_at: ActiveValue::Set(1000),
            updated_at: ActiveValue::Set(1000),
            min_confirmations: ActiveValue::Set(1),
            ..Default::default()
        }
    }

    fn make_txo_mod(txid: &str, vout: u32, amount: &str) -> DbTxoActMod {
        DbTxoActMod {
            txid: ActiveValue::Set(txid.to_string()),
            vout: ActiveValue::Set(vout),
            btc_amount: ActiveValue::Set(amount.to_string()),
            spent: ActiveValue::Set(false),
            exists: ActiveValue::Set(true),
            pending_witness: ActiveValue::Set(false),
            ..Default::default()
        }
    }

    fn make_asset_transfer_mod(batch_idx: i32, asset_id: Option<&str>) -> DbAssetTransferActMod {
        DbAssetTransferActMod {
            user_driven: ActiveValue::Set(true),
            batch_transfer_idx: ActiveValue::Set(batch_idx),
            asset_id: ActiveValue::Set(asset_id.map(|s| s.to_string())),
            ..Default::default()
        }
    }

    fn make_transfer_mod(asset_transfer_idx: i32, incoming: bool) -> DbTransferActMod {
        DbTransferActMod {
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            incoming: ActiveValue::Set(incoming),
            ..Default::default()
        }
    }

    fn make_coloring_mod(
        txo_idx: i32,
        asset_transfer_idx: i32,
        coloring_type: ColoringType,
        amount: u64,
    ) -> DbColoringActMod {
        DbColoringActMod {
            txo_idx: ActiveValue::Set(txo_idx),
            asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
            r#type: ActiveValue::Set(coloring_type),
            assignment: ActiveValue::Set(Assignment::Fungible(amount)),
            ..Default::default()
        }
    }

    // ── Asset ──────────────────────────────────────────────────────

    #[test]
    fn test_asset_set_and_get() {
        let db = make_db();
        let idx = db.set_asset(make_asset_mod("asset1")).unwrap();
        assert_eq!(idx, 1);

        let asset = db.get_asset("asset1".to_string()).unwrap().unwrap();
        assert_eq!(asset.idx, 1);
        assert_eq!(asset.id, "asset1");
        assert_eq!(asset.name, "Test Asset");
        assert_eq!(asset.precision, 8);
        assert_eq!(asset.schema, AssetSchema::Nia);
    }

    #[test]
    fn test_asset_get_nonexistent() {
        let db = make_db();
        let result = db.get_asset("nope".to_string()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_asset_update() {
        let db = make_db();
        db.set_asset(make_asset_mod("asset1")).unwrap();

        let mut upd: DbAssetActMod = db.get_asset("asset1".to_string()).unwrap().unwrap().into();
        upd.name = ActiveValue::Set("Updated".to_string());
        upd.precision = ActiveValue::Set(2);
        let updated = db.update_asset(&mut upd).unwrap();

        assert_eq!(updated.name, "Updated");
        assert_eq!(updated.precision, 2);
        // unchanged fields preserved
        assert_eq!(updated.id, "asset1");
    }

    #[test]
    fn test_asset_iter_and_ids() {
        let db = make_db();
        db.set_asset(make_asset_mod("a1")).unwrap();
        db.set_asset(make_asset_mod("a2")).unwrap();

        let assets = db.iter_assets().unwrap();
        assert_eq!(assets.len(), 2);

        let ids = db.get_asset_ids().unwrap();
        assert!(ids.contains(&"a1".to_string()));
        assert!(ids.contains(&"a2".to_string()));
    }

    #[test]
    fn test_asset_check_exists() {
        let db = make_db();
        db.set_asset(make_asset_mod("a1")).unwrap();

        let asset = db.check_asset_exists("a1".to_string()).unwrap();
        assert_eq!(asset.id, "a1");

        let err = db.check_asset_exists("missing".to_string()).unwrap_err();
        assert!(matches!(err, Error::AssetNotFound { asset_id } if asset_id == "missing"));
    }

    #[test]
    fn test_asset_autoincrement_idx() {
        let db = make_db();
        let idx1 = db.set_asset(make_asset_mod("a1")).unwrap();
        let idx2 = db.set_asset(make_asset_mod("a2")).unwrap();
        assert_eq!(idx1, 1);
        assert_eq!(idx2, 2);
    }

    // ── BatchTransfer ──────────────────────────────────────────────

    #[test]
    fn test_batch_transfer_set_and_iter() {
        let db = make_db();
        let idx = db
            .set_batch_transfer(make_batch_transfer_mod(TransferStatus::Settled))
            .unwrap();
        assert_eq!(idx, 1);

        let items = db.iter_batch_transfers().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, TransferStatus::Settled);
    }

    #[test]
    fn test_batch_transfer_update() {
        let db = make_db();
        db.set_batch_transfer(make_batch_transfer_mod(TransferStatus::WaitingCounterparty))
            .unwrap();

        let bt = &db.iter_batch_transfers().unwrap()[0];
        let mut upd: DbBatchTransferActMod = bt.clone().into();
        upd.status = ActiveValue::Set(TransferStatus::Settled);
        upd.txid = ActiveValue::Set(Some("abc123".to_string()));
        let updated = db.update_batch_transfer(&mut upd).unwrap();

        assert_eq!(updated.status, TransferStatus::Settled);
        assert_eq!(updated.txid, Some("abc123".to_string()));
    }

    #[test]
    fn test_batch_transfer_del_cascades() {
        let db = make_db();

        // Create two batch transfers so PKs diverge from child PKs
        let bt1_idx = db
            .set_batch_transfer(make_batch_transfer_mod(TransferStatus::Settled))
            .unwrap();
        let bt2_idx = db
            .set_batch_transfer(make_batch_transfer_mod(TransferStatus::WaitingCounterparty))
            .unwrap();

        // Create asset transfers for each batch
        let at1_idx = db
            .set_asset_transfer(make_asset_transfer_mod(bt1_idx, Some("asset1")))
            .unwrap();
        let at2_idx = db
            .set_asset_transfer(make_asset_transfer_mod(bt2_idx, Some("asset2")))
            .unwrap();

        // Create transfers for each asset_transfer
        let t1_idx = db.set_transfer(make_transfer_mod(at1_idx, true)).unwrap();
        let t2_idx = db.set_transfer(make_transfer_mod(at2_idx, false)).unwrap();

        // Create transport endpoints for the transfers
        let te_idx = db
            .set_transport_endpoint(DbTransportEndpointActMod {
                transport_type: ActiveValue::Set(TransportType::JsonRpc),
                endpoint: ActiveValue::Set("http://proxy:3000/json-rpc".to_string()),
                ..Default::default()
            })
            .unwrap();
        db.set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
            transfer_idx: ActiveValue::Set(t1_idx),
            transport_endpoint_idx: ActiveValue::Set(te_idx),
            ..Default::default()
        })
        .unwrap();
        db.set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
            transfer_idx: ActiveValue::Set(t2_idx),
            transport_endpoint_idx: ActiveValue::Set(te_idx),
            ..Default::default()
        })
        .unwrap();

        // Delete batch_transfer 1 — should cascade to at1, t1, and t1's TTEs
        let bt1 = db
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|b| b.idx == bt1_idx)
            .unwrap();
        db.del_batch_transfer(&bt1).unwrap();

        // bt1 gone, bt2 remains
        let bts = db.iter_batch_transfers().unwrap();
        assert_eq!(bts.len(), 1);
        assert_eq!(bts[0].idx, bt2_idx);

        // at1 gone, at2 remains
        let ats = db.iter_asset_transfers().unwrap();
        assert_eq!(ats.len(), 1);
        assert_eq!(ats[0].idx, at2_idx);

        // t1 gone, t2 remains
        let ts = db.iter_transfers().unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].idx, t2_idx);

        // Only t2's transport endpoint remains
        let t1_ttes = db.get_transfer_transport_endpoints_data(t1_idx).unwrap();
        assert!(t1_ttes.is_empty());
        let t2_ttes = db.get_transfer_transport_endpoints_data(t2_idx).unwrap();
        assert_eq!(t2_ttes.len(), 1);
    }

    // ── Txo ────────────────────────────────────────────────────────

    #[test]
    fn test_txo_set_and_get() {
        let db = make_db();
        let idx = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();
        assert_eq!(idx, 1);

        let outpoint = Outpoint {
            txid: "tx1".to_string(),
            vout: 0,
        };
        let txo = db.get_txo(&outpoint).unwrap().unwrap();
        assert_eq!(txo.btc_amount, "50000");
        assert!(!txo.spent);
        assert!(txo.exists);
    }

    #[test]
    fn test_txo_upsert_same_outpoint() {
        let db = make_db();
        let idx1 = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        // Second insert with same txid:vout should upsert, returning same idx
        let idx2 = db
            .set_txo(DbTxoActMod {
                txid: ActiveValue::Set("tx1".to_string()),
                vout: ActiveValue::Set(0),
                btc_amount: ActiveValue::Set("60000".to_string()),
                exists: ActiveValue::Set(true),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(idx1, idx2);
        let txos = db.iter_txos().unwrap();
        assert_eq!(txos.len(), 1);
        // btc_amount should be updated because it's not "0"
        assert_eq!(txos[0].btc_amount, "60000");
    }

    #[test]
    fn test_txo_upsert_zero_amount_not_updated() {
        let db = make_db();
        db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        // Upsert with "0" amount should NOT update btc_amount
        db.set_txo(DbTxoActMod {
            txid: ActiveValue::Set("tx1".to_string()),
            vout: ActiveValue::Set(0),
            btc_amount: ActiveValue::Set("0".to_string()),
            exists: ActiveValue::Set(true),
            ..Default::default()
        })
        .unwrap();

        let txos = db.iter_txos().unwrap();
        assert_eq!(txos[0].btc_amount, "50000");
    }

    #[test]
    fn test_txo_update() {
        let db = make_db();
        db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        let txo = db.iter_txos().unwrap()[0].clone();
        let mut upd: DbTxoActMod = txo.into();
        upd.spent = ActiveValue::Set(true);
        db.update_txo(upd).unwrap();

        let txos = db.iter_txos().unwrap();
        assert!(txos[0].spent);
    }

    #[test]
    fn test_txo_get_nonexistent() {
        let db = make_db();
        let outpoint = Outpoint {
            txid: "nope".to_string(),
            vout: 0,
        };
        assert!(db.get_txo(&outpoint).unwrap().is_none());
    }

    #[test]
    fn test_txo_get_unspent() {
        let db = make_db();
        db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();
        db.set_txo(make_txo_mod("tx2", 0, "30000")).unwrap();

        // Mark first as spent
        let txo = db.iter_txos().unwrap()[0].clone();
        let mut upd: DbTxoActMod = txo.into();
        upd.spent = ActiveValue::Set(true);
        db.update_txo(upd).unwrap();

        let unspent = db.get_unspent_txos(vec![]).unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(unspent[0].btc_amount, "30000");
    }

    #[test]
    fn test_txo_del_cascades() {
        let db = make_db();
        // Create two TXOs so PKs are 1 and 2
        let txo1_idx = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();
        let txo2_idx = db.set_txo(make_txo_mod("tx2", 0, "30000")).unwrap();

        // Create colorings: two for txo1, one for txo2
        // Use asset_transfer_idx values that differ from txo_idx to catch wrong-field bugs
        db.set_coloring(make_coloring_mod(txo1_idx, 10, ColoringType::Receive, 100))
            .unwrap();
        db.set_coloring(make_coloring_mod(txo1_idx, 11, ColoringType::Change, 50))
            .unwrap();
        db.set_coloring(make_coloring_mod(txo2_idx, 12, ColoringType::Receive, 200))
            .unwrap();

        // Delete txo1 — should remove txo1 row and its 2 colorings
        db.del_txo(txo1_idx).unwrap();

        // txo1 gone, txo2 remains
        let txos = db.iter_txos().unwrap();
        assert_eq!(txos.len(), 1);
        assert_eq!(txos[0].idx, txo2_idx);

        // Only txo2's coloring remains
        let colorings = db.iter_colorings().unwrap();
        assert_eq!(colorings.len(), 1);
        assert_eq!(colorings[0].txo_idx, txo2_idx);
    }

    #[test]
    fn test_txo_iter() {
        let db = make_db();
        db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();
        db.set_txo(make_txo_mod("tx2", 1, "30000")).unwrap();

        let txos = db.iter_txos().unwrap();
        assert_eq!(txos.len(), 2);
    }

    // ── Transfer ───────────────────────────────────────────────────

    #[test]
    fn test_transfer_set_and_iter() {
        let db = make_db();
        let idx = db.set_transfer(make_transfer_mod(1, true)).unwrap();
        assert_eq!(idx, 1);

        let transfers = db.iter_transfers().unwrap();
        assert_eq!(transfers.len(), 1);
        assert!(transfers[0].incoming);
    }

    #[test]
    fn test_transfer_update() {
        let db = make_db();
        db.set_transfer(make_transfer_mod(1, true)).unwrap();

        let t = &db.iter_transfers().unwrap()[0];
        let mut upd: DbTransferActMod = t.clone().into();
        upd.ack = ActiveValue::Set(Some(true));
        upd.recipient_id = ActiveValue::Set(Some("rid1".to_string()));
        let updated = db.update_transfer(&mut upd).unwrap();

        assert_eq!(updated.ack, Some(true));
        assert_eq!(updated.recipient_id, Some("rid1".to_string()));
        assert!(updated.incoming); // unchanged
    }

    // ── AssetTransfer ──────────────────────────────────────────────

    #[test]
    fn test_asset_transfer_set_and_iter() {
        let db = make_db();
        let idx = db
            .set_asset_transfer(make_asset_transfer_mod(1, Some("a1")))
            .unwrap();
        assert_eq!(idx, 1);

        let items = db.iter_asset_transfers().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].asset_id, Some("a1".to_string()));
    }

    #[test]
    fn test_asset_transfer_update() {
        let db = make_db();
        db.set_asset_transfer(make_asset_transfer_mod(1, Some("a1")))
            .unwrap();

        let at = &db.iter_asset_transfers().unwrap()[0];
        let mut upd: DbAssetTransferActMod = at.clone().into();
        upd.asset_id = ActiveValue::Set(Some("a2".to_string()));
        let updated = db.update_asset_transfer(&mut upd).unwrap();

        assert_eq!(updated.asset_id, Some("a2".to_string()));
    }

    // ── Coloring ───────────────────────────────────────────────────

    #[test]
    fn test_coloring_set_and_iter() {
        let db = make_db();
        let idx = db
            .set_coloring(make_coloring_mod(1, 1, ColoringType::Issue, 500))
            .unwrap();
        assert_eq!(idx, 1);

        let items = db.iter_colorings().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].r#type, ColoringType::Issue);
        assert_eq!(items[0].assignment, Assignment::Fungible(500));
    }

    #[test]
    fn test_coloring_del_by_asset_transfer_idx() {
        let db = make_db();
        db.set_coloring(make_coloring_mod(1, 10, ColoringType::Receive, 100))
            .unwrap();
        db.set_coloring(make_coloring_mod(2, 20, ColoringType::Issue, 200))
            .unwrap();

        db.del_coloring(10).unwrap();

        let items = db.iter_colorings().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].asset_transfer_idx, 20);
    }

    // ── Media ──────────────────────────────────────────────────────

    #[test]
    fn test_media_set_get_iter() {
        let db = make_db();
        let idx = db
            .set_media(DbMediaActMod {
                digest: ActiveValue::Set("sha256abc".to_string()),
                mime: ActiveValue::Set("image/png".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(idx, 1);

        let media = db.get_media(1).unwrap().unwrap();
        assert_eq!(media.digest, "sha256abc");
        assert_eq!(media.mime, "image/png");

        assert!(db.get_media(999).unwrap().is_none());

        let all = db.iter_media().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_media_get_by_digest() {
        let db = make_db();
        db.set_media(DbMediaActMod {
            digest: ActiveValue::Set("d1".to_string()),
            mime: ActiveValue::Set("text/plain".to_string()),
            ..Default::default()
        })
        .unwrap();

        let found = db.get_media_by_digest("d1".to_string()).unwrap().unwrap();
        assert_eq!(found.mime, "text/plain");

        assert!(
            db.get_media_by_digest("nope".to_string())
                .unwrap()
                .is_none()
        );
    }

    // ── TransportEndpoint ──────────────────────────────────────────

    #[test]
    fn test_transport_endpoint_set_and_get() {
        let db = make_db();
        let idx = db
            .set_transport_endpoint(DbTransportEndpointActMod {
                transport_type: ActiveValue::Set(TransportType::JsonRpc),
                endpoint: ActiveValue::Set("http://proxy.example.com".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(idx, 1);

        let te = db
            .get_transport_endpoint("http://proxy.example.com".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(te.transport_type, TransportType::JsonRpc);

        assert!(
            db.get_transport_endpoint("nope".to_string())
                .unwrap()
                .is_none()
        );
    }

    // ── TransferTransportEndpoint ──────────────────────────────────

    #[test]
    fn test_transfer_transport_endpoint_set_update_get_data() {
        let db = make_db();

        // Set up a transport endpoint first
        let te_idx = db
            .set_transport_endpoint(DbTransportEndpointActMod {
                transport_type: ActiveValue::Set(TransportType::JsonRpc),
                endpoint: ActiveValue::Set("http://proxy.example.com".to_string()),
                ..Default::default()
            })
            .unwrap();

        let tte_idx = db
            .set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
                transfer_idx: ActiveValue::Set(42),
                transport_endpoint_idx: ActiveValue::Set(te_idx),
                used: ActiveValue::Set(false),
                ..Default::default()
            })
            .unwrap();

        // Update to mark as used
        let mut upd = DbTransferTransportEndpointActMod {
            idx: ActiveValue::Unchanged(tte_idx),
            used: ActiveValue::Set(true),
            ..Default::default()
        };
        let updated = db.update_transfer_transport_endpoint(&mut upd).unwrap();
        assert!(updated.used);

        // get_transfer_transport_endpoints_data
        let data = db.get_transfer_transport_endpoints_data(42).unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].0.transfer_idx, 42);
        assert_eq!(data[0].1.endpoint, "http://proxy.example.com");
    }

    #[test]
    fn test_transfer_transport_endpoint_data_empty() {
        let db = make_db();
        let data = db.get_transfer_transport_endpoints_data(999).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn test_transfer_transport_endpoint_data_orphaned_fk_returns_error() {
        let db = make_db();
        // Create a TTE that references a non-existent transport endpoint
        db.set_transfer_transport_endpoint(DbTransferTransportEndpointActMod {
            transfer_idx: ActiveValue::Set(1),
            transport_endpoint_idx: ActiveValue::Set(999), // doesn't exist
            ..Default::default()
        })
        .unwrap();
        let result = db.get_transfer_transport_endpoints_data(1);
        assert!(result.is_err());
    }

    // ── PendingWitnessScript ───────────────────────────────────────

    #[test]
    fn test_pending_witness_script_set_del_iter() {
        let db = make_db();
        db.set_pending_witness_script(DbPendingWitnessScriptActMod {
            script: ActiveValue::Set("script_a".to_string()),
            ..Default::default()
        })
        .unwrap();
        db.set_pending_witness_script(DbPendingWitnessScriptActMod {
            script: ActiveValue::Set("script_b".to_string()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(db.iter_pending_witness_scripts().unwrap().len(), 2);

        db.del_pending_witness_script("script_a".to_string())
            .unwrap();
        let remaining = db.iter_pending_witness_scripts().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].script, "script_b");
    }

    // ── WalletTransaction ──────────────────────────────────────────

    #[test]
    fn test_wallet_transaction_set_and_iter() {
        let db = make_db();
        let idx = db
            .set_wallet_transaction(DbWalletTransactionActMod {
                txid: ActiveValue::Set("wtx1".to_string()),
                r#type: ActiveValue::Set(WalletTransactionType::CreateUtxos),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(idx, 1);

        db.set_wallet_transaction(DbWalletTransactionActMod {
            txid: ActiveValue::Set("wtx2".to_string()),
            r#type: ActiveValue::Set(WalletTransactionType::Drain),
            ..Default::default()
        })
        .unwrap();

        let items = db.iter_wallet_transactions().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].r#type, WalletTransactionType::CreateUtxos);
        assert_eq!(items[1].r#type, WalletTransactionType::Drain);
    }

    // ── BackupInfo ─────────────────────────────────────────────────

    #[test]
    fn test_backup_info_set_get_update_del() {
        let db = make_db();

        // Initially empty
        assert!(db.get_backup_info().unwrap().is_none());

        // Set
        db.set_backup_info(DbBackupInfoActMod {
            last_backup_timestamp: ActiveValue::Set("2026-01-01".to_string()),
            last_operation_timestamp: ActiveValue::Set("2026-01-01".to_string()),
            ..Default::default()
        })
        .unwrap();

        let info = db.get_backup_info().unwrap().unwrap();
        assert_eq!(info.last_backup_timestamp, "2026-01-01");

        // Update
        let mut upd: DbBackupInfoActMod = info.into();
        upd.last_backup_timestamp = ActiveValue::Set("2026-02-01".to_string());
        let updated = db.update_backup_info(&mut upd).unwrap();
        assert_eq!(updated.last_backup_timestamp, "2026-02-01");
        // Unchanged field preserved
        assert_eq!(updated.last_operation_timestamp, "2026-01-01");

        // Delete
        db.del_backup_info().unwrap();
        assert!(db.get_backup_info().unwrap().is_none());
    }

    #[test]
    fn test_backup_info_update_when_empty_fails() {
        let db = make_db();
        let mut upd = DbBackupInfoActMod {
            last_backup_timestamp: ActiveValue::Set("x".to_string()),
            ..Default::default()
        };
        let result = db.update_backup_info(&mut upd);
        assert!(result.is_err());
    }

    // ── get_db_data ────────────────────────────────────────────────

    #[test]
    fn test_get_db_data_with_transfers() {
        let db = make_db();
        db.set_batch_transfer(make_batch_transfer_mod(TransferStatus::Settled))
            .unwrap();
        db.set_asset_transfer(make_asset_transfer_mod(1, Some("a1")))
            .unwrap();
        db.set_transfer(make_transfer_mod(1, true)).unwrap();
        db.set_coloring(make_coloring_mod(1, 1, ColoringType::Issue, 100))
            .unwrap();
        db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        let data = db.get_db_data(false).unwrap();
        assert_eq!(data.batch_transfers.len(), 1);
        assert_eq!(data.asset_transfers.len(), 1);
        assert_eq!(data.transfers.len(), 1);
        assert_eq!(data.colorings.len(), 1);
        assert_eq!(data.txos.len(), 1);
    }

    #[test]
    fn test_get_db_data_empty_transfers_flag() {
        let db = make_db();
        db.set_transfer(make_transfer_mod(1, true)).unwrap();

        let data = db.get_db_data(true).unwrap();
        assert!(data.transfers.is_empty());
    }

    // ── get_asset_balance ──────────────────────────────────────────

    #[test]
    fn test_get_asset_balance_settled_issuance() {
        let db = make_db();

        // 1. Create a settled batch transfer
        let bt_idx = db
            .set_batch_transfer(make_batch_transfer_mod(TransferStatus::Settled))
            .unwrap();

        // 2. Asset transfer linked to asset
        let at_idx = db
            .set_asset_transfer(make_asset_transfer_mod(bt_idx, Some("a1")))
            .unwrap();

        // 3. Transfer (incoming)
        db.set_transfer(make_transfer_mod(at_idx, true)).unwrap();

        // 4. TXO (unspent)
        let txo_idx = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        // 5. Coloring: Issue type (incoming), 1000 fungible
        db.set_coloring(make_coloring_mod(
            txo_idx,
            at_idx,
            ColoringType::Issue,
            1000,
        ))
        .unwrap();

        let balance = db
            .get_asset_balance("a1".to_string(), None, None, None, None, None)
            .unwrap();

        assert_eq!(balance.settled, 1000);
        assert_eq!(balance.future, 1000);
        assert_eq!(balance.spendable, 1000);
    }

    #[test]
    fn test_get_asset_balance_empty() {
        let db = make_db();
        let balance = db
            .get_asset_balance("nonexistent".to_string(), None, None, None, None, None)
            .unwrap();
        assert_eq!(balance.settled, 0);
        assert_eq!(balance.future, 0);
        assert_eq!(balance.spendable, 0);
    }

    // ── Update with missing idx fails ──────────────────────────────

    #[test]
    fn test_update_nonexistent_transfer_fails() {
        let db = make_db();
        let mut upd = DbTransferActMod {
            idx: ActiveValue::Set(999),
            ..Default::default()
        };
        assert!(db.update_transfer(&mut upd).is_err());
    }

    #[test]
    fn test_update_nonexistent_asset_fails() {
        let db = make_db();
        let mut upd = DbAssetActMod {
            idx: ActiveValue::Set(999),
            ..Default::default()
        };
        assert!(db.update_asset(&mut upd).is_err());
    }

    #[test]
    fn test_update_nonexistent_batch_transfer_fails() {
        let db = make_db();
        let mut upd = DbBatchTransferActMod {
            idx: ActiveValue::Set(999),
            ..Default::default()
        };
        assert!(db.update_batch_transfer(&mut upd).is_err());
    }

    #[test]
    fn test_update_nonexistent_txo_fails() {
        let db = make_db();
        let upd = DbTxoActMod {
            idx: ActiveValue::Set(999),
            ..Default::default()
        };
        assert!(db.update_txo(upd).is_err());
    }

    // ── get_batch_transfer_or_fail ─────────────────────────────────

    #[test]
    fn test_get_batch_transfer_or_fail() {
        let db = make_db();
        let bt_idx = db
            .set_batch_transfer(make_batch_transfer_mod(TransferStatus::Settled))
            .unwrap();
        let batch_transfers = db.iter_batch_transfers().unwrap();

        let found = db
            .get_batch_transfer_or_fail(bt_idx, &batch_transfers)
            .unwrap();
        assert_eq!(found.idx, bt_idx);

        let err = db
            .get_batch_transfer_or_fail(999, &batch_transfers)
            .unwrap_err();
        assert!(matches!(err, Error::BatchTransferNotFound { idx: 999 }));
    }

    // ── get_unspent_txos with provided list ────────────────────────

    #[test]
    fn test_get_unspent_txos_with_provided_list() {
        let db = make_db();

        let txos = vec![
            DbTxo {
                idx: 1,
                txid: "tx1".to_string(),
                vout: 0,
                btc_amount: "1000".to_string(),
                spent: false,
                exists: true,
                pending_witness: false,
            },
            DbTxo {
                idx: 2,
                txid: "tx2".to_string(),
                vout: 0,
                btc_amount: "2000".to_string(),
                spent: true,
                exists: true,
                pending_witness: false,
            },
        ];

        let unspent = db.get_unspent_txos(txos).unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(unspent[0].txid, "tx1");
    }

    // ── Multiple entities interact correctly ───────────────────────

    #[test]
    fn test_multiple_colorings_per_txo() {
        let db = make_db();
        let txo_idx = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();

        db.set_coloring(make_coloring_mod(txo_idx, 1, ColoringType::Issue, 500))
            .unwrap();
        db.set_coloring(make_coloring_mod(txo_idx, 2, ColoringType::Receive, 300))
            .unwrap();

        let colorings = db.iter_colorings().unwrap();
        assert_eq!(colorings.len(), 2);
        assert_eq!(colorings[0].txo_idx, txo_idx);
        assert_eq!(colorings[1].txo_idx, txo_idx);
    }

    #[test]
    fn test_del_coloring_only_removes_matching() {
        let db = make_db();
        db.set_coloring(make_coloring_mod(1, 10, ColoringType::Issue, 100))
            .unwrap();
        db.set_coloring(make_coloring_mod(1, 10, ColoringType::Receive, 200))
            .unwrap();
        db.set_coloring(make_coloring_mod(2, 20, ColoringType::Issue, 300))
            .unwrap();

        // Delete all colorings with asset_transfer_idx == 10
        db.del_coloring(10).unwrap();
        let remaining = db.iter_colorings().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].asset_transfer_idx, 20);
    }

    #[test]
    fn test_empty_db_iters() {
        let db = make_db();
        assert!(db.iter_assets().unwrap().is_empty());
        assert!(db.iter_batch_transfers().unwrap().is_empty());
        assert!(db.iter_txos().unwrap().is_empty());
        assert!(db.iter_transfers().unwrap().is_empty());
        assert!(db.iter_asset_transfers().unwrap().is_empty());
        assert!(db.iter_colorings().unwrap().is_empty());
        assert!(db.iter_media().unwrap().is_empty());
        assert!(db.iter_pending_witness_scripts().unwrap().is_empty());
        assert!(db.iter_wallet_transactions().unwrap().is_empty());
    }

    // ── Fix #18: orphaned FK handling ─────────────────────────────

    #[test]
    fn test_utxo_allocations_skips_orphaned_colorings() {
        let db = make_db();

        // Create a TXO
        let txo_idx = db.set_txo(make_txo_mod("tx1", 0, "50000")).unwrap();
        let txo = db.iter_txos().unwrap()[0].clone();

        // Create a coloring that references a non-existent asset_transfer (idx 999)
        db.set_coloring(DbColoringActMod {
            txo_idx: ActiveValue::Set(txo_idx),
            asset_transfer_idx: ActiveValue::Set(999), // orphaned FK
            r#type: ActiveValue::Set(ColoringType::Receive),
            assignment: ActiveValue::Set(Assignment::Fungible(100)),
            ..Default::default()
        })
        .unwrap();

        // Should skip the orphaned coloring, not panic
        let allocations = db
            .get_rgb_allocations(vec![txo], None, None, None, None)
            .unwrap();
        assert_eq!(allocations.len(), 1);
        assert!(allocations[0].rgb_allocations.is_empty());
    }
}
