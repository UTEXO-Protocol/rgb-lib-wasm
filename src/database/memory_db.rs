//! In-memory wallet DB for wasm32: same API as RgbLibDatabase, backed by Vec/HashMap.
//! No SQLite; used when data_dir == ":memory:" in the browser.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;

use super::*;
use crate::database::entities::{
    asset::{ActiveModel as DbAssetActMod, Model as DbAsset},
    asset_transfer::{ActiveModel as DbAssetTransferActMod, Model as DbAssetTransfer},
    backup_info::{ActiveModel as DbBackupInfoActMod, Model as DbBackupInfo},
    batch_transfer::{ActiveModel as DbBatchTransferActMod, Model as DbBatchTransfer},
    coloring::{ActiveModel as DbColoringActMod, Model as DbColoring},
    media::{ActiveModel as DbMediaActMod, Model as DbMedia},
    pending_witness_script::{ActiveModel as DbPendingWitnessScriptActMod, Model as DbPendingWitnessScript},
    token::{ActiveModel as DbTokenActMod, Model as DbToken},
    token_media::{ActiveModel as DbTokenMediaActMod, Model as DbTokenMedia},
    transfer::{ActiveModel as DbTransferActMod, Model as DbTransfer},
    transfer_transport_endpoint::{
        ActiveModel as DbTransferTransportEndpointActMod, Model as DbTransferTransportEndpoint,
    },
    transport_endpoint::{ActiveModel as DbTransportEndpointActMod, Model as DbTransportEndpoint},
    txo::{ActiveModel as DbTxoActMod, Model as DbTxo},
    wallet_transaction::{ActiveModel as DbWalletTransactionActMod, Model as DbWalletTransaction},
};
use sea_orm::{ActiveValue, Value};

use crate::database::enums::{
    AssetSchema, Assignment, ColoringType, TransportType, TransferStatus, WalletTransactionType,
};

/// Extract value from ActiveValue; use default for NotSet.
fn av<T: Default + Into<Value>>(v: ActiveValue<T>) -> T {
    match v {
        ActiveValue::Set(x) | ActiveValue::Unchanged(x) => x,
        ActiveValue::NotSet => T::default(),
    }
}

/// Extract value from ActiveValue; use given default for NotSet.
fn av_or<T: Into<Value>>(v: ActiveValue<T>, default: T) -> T {
    match v {
        ActiveValue::Set(x) | ActiveValue::Unchanged(x) => x,
        ActiveValue::NotSet => default,
    }
}

/// In-memory store: one Vec per entity, next_id per table for insert idx.
/// RefCell for interior mutability so methods can take &self (Arc<Backend>).
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
    tokens: RefCell<Vec<DbToken>>,
    next_token_idx: RefCell<i32>,
    token_medias: RefCell<Vec<DbTokenMedia>>,
    next_token_media_idx: RefCell<i32>,
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
            tokens: RefCell::new(Vec::new()),
            next_token_idx: RefCell::new(1),
            token_medias: RefCell::new(Vec::new()),
            next_token_media_idx: RefCell::new(1),
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

    pub(crate) fn set_token(&self, t: DbTokenActMod) -> Result<i32, InternalError> {
        let idx = *self.next_token_idx.borrow();
        *self.next_token_idx.borrow_mut() += 1;
        let row = DbToken {
            idx,
            asset_idx: av(t.asset_idx),
            index: av(t.index),
            ticker: av(t.ticker),
            name: av(t.name),
            details: av(t.details),
            embedded_media: av(t.embedded_media),
            reserves: av(t.reserves),
        };
        self.tokens.borrow_mut().push(row);
        Ok(idx)
    }

    pub(crate) fn set_token_media(
        &self,
        t: DbTokenMediaActMod,
    ) -> Result<i32, InternalError> {
        let idx = *self.next_token_media_idx.borrow();
        *self.next_token_media_idx.borrow_mut() += 1;
        let row = DbTokenMedia {
            idx,
            token_idx: av(t.token_idx),
            media_idx: av(t.media_idx),
            attachment_id: av(t.attachment_id),
        };
        self.token_medias.borrow_mut().push(row);
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
        if let Some(pos) = self.txos.borrow().iter().position(|r| r.txid == txid && r.vout == vout) {
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
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("transfer".into())))?;
        let row = &mut transfers[pos];
        if let ActiveValue::Set(v) = t.asset_transfer_idx { row.asset_transfer_idx = v; }
        if let ActiveValue::Set(v) = &t.requested_assignment { row.requested_assignment = v.clone(); }
        if let ActiveValue::Set(v) = t.incoming { row.incoming = v; }
        if let ActiveValue::Set(v) = &t.recipient_type { row.recipient_type = v.clone(); }
        if let ActiveValue::Set(v) = &t.recipient_id { row.recipient_id = v.clone(); }
        if let ActiveValue::Set(v) = t.ack { row.ack = v; }
        if let ActiveValue::Set(v) = &t.invoice_string { row.invoice_string = v.clone(); }
        Ok(row.clone())
    }

    pub(crate) fn update_asset(&self, a: &mut DbAssetActMod) -> Result<DbAsset, InternalError> {
        let idx = av(a.idx.clone());
        let mut assets = self.assets.borrow_mut();
        let pos = assets
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("asset".into())))?;
        let row = &mut assets[pos];
        if let ActiveValue::Set(v) = &a.media_idx { row.media_idx = v.clone(); }
        if let ActiveValue::Set(v) = &a.details { row.details = v.clone(); }
        if let ActiveValue::Set(v) = &a.initial_supply { row.initial_supply = v.clone(); }
        if let ActiveValue::Set(v) = &a.name { row.name = v.clone(); }
        if let ActiveValue::Set(v) = a.precision { row.precision = v; }
        if let ActiveValue::Set(v) = &a.ticker { row.ticker = v.clone(); }
        if let ActiveValue::Set(v) = &a.max_supply { row.max_supply = v.clone(); }
        if let ActiveValue::Set(v) = &a.known_circulating_supply { row.known_circulating_supply = v.clone(); }
        if let ActiveValue::Set(v) = &a.reject_list_url { row.reject_list_url = v.clone(); }
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
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("asset_transfer".into())))?;
        let row = &mut asset_transfers[pos];
        if let ActiveValue::Set(v) = a.user_driven { row.user_driven = v; }
        if let ActiveValue::Set(v) = a.batch_transfer_idx { row.batch_transfer_idx = v; }
        if let ActiveValue::Set(v) = &a.asset_id { row.asset_id = v.clone(); }
        Ok(row.clone())
    }

    pub(crate) fn update_backup_info(
        &self,
        b: &mut DbBackupInfoActMod,
    ) -> Result<DbBackupInfo, InternalError> {
        if let Some(row) = &mut *self.backup_info.borrow_mut() {
            if let ActiveValue::Set(v) = &b.last_backup_timestamp { row.last_backup_timestamp = v.clone(); }
            if let ActiveValue::Set(v) = &b.last_operation_timestamp { row.last_operation_timestamp = v.clone(); }
            Ok(row.clone())
        } else {
            Err(InternalError::Database(sea_orm::DbErr::RecordNotFound("backup_info".into())))
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
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("batch_transfer".into())))?;
        let row = &mut batch_transfers[pos];
        if let ActiveValue::Set(v) = &b.txid { row.txid = v.clone(); }
        if let ActiveValue::Set(v) = b.status { row.status = v; }
        if let ActiveValue::Set(v) = b.updated_at { row.updated_at = v; }
        if let ActiveValue::Set(v) = &b.expiration { row.expiration = v.clone(); }
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
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("tte".into())))?;
        let row = &mut tte[pos];
        if let ActiveValue::Set(v) = t.used { row.used = v; }
        Ok(row.clone())
    }

    pub(crate) fn update_txo(&self, t: DbTxoActMod) -> Result<(), InternalError> {
        let idx = av(t.idx);
        let mut txos = self.txos.borrow_mut();
        let pos = txos
            .iter()
            .position(|r| r.idx == idx)
            .ok_or_else(|| InternalError::Database(sea_orm::DbErr::RecordNotFound("txo".into())))?;
        let row = &mut txos[pos];
        if let ActiveValue::Set(v) = &t.btc_amount { row.btc_amount = v.clone(); }
        if let ActiveValue::Set(v) = t.spent { row.spent = v; }
        if let ActiveValue::Set(v) = t.exists { row.exists = v; }
        if let ActiveValue::Set(v) = t.pending_witness { row.pending_witness = v; }
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
        self.transfers.borrow_mut().retain(|r| r.idx != batch_transfer.idx);
        Ok(())
    }

    pub(crate) fn del_coloring(&self, asset_transfer_idx: i32) -> Result<(), InternalError> {
        self.colorings.borrow_mut().retain(|c| c.asset_transfer_idx != asset_transfer_idx);
        Ok(())
    }

    pub(crate) fn del_pending_witness_script(&self, script: String) -> Result<(), InternalError> {
        self.pending_witness_scripts.borrow_mut().retain(|p| p.script != script);
        Ok(())
    }

    pub(crate) fn del_txo(&self, idx: i32) -> Result<(), InternalError> {
        self.colorings.borrow_mut().retain(|c| c.idx != idx);
        Ok(())
    }

    pub(crate) fn get_asset(&self, asset_id: String) -> Result<Option<DbAsset>, InternalError> {
        Ok(self.assets.borrow().iter().find(|a| a.id == asset_id).cloned())
    }

    pub(crate) fn get_backup_info(&self) -> Result<Option<DbBackupInfo>, InternalError> {
        Ok(self.backup_info.borrow().clone())
    }

    pub(crate) fn get_media(&self, media_idx: i32) -> Result<Option<DbMedia>, InternalError> {
        Ok(self.media.borrow().iter().find(|m| m.idx == media_idx).cloned())
    }

    pub(crate) fn get_media_by_digest(&self, digest: String) -> Result<Option<DbMedia>, InternalError> {
        Ok(self.media.borrow().iter().find(|m| m.digest == digest).cloned())
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

    pub(crate) fn iter_token_medias(&self) -> Result<Vec<DbTokenMedia>, InternalError> {
        Ok(self.token_medias.borrow().clone())
    }

    pub(crate) fn iter_tokens(&self) -> Result<Vec<DbToken>, InternalError> {
        Ok(self.tokens.borrow().clone())
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
                .expect("fk");
            out.push((tte.clone(), te));
        }
        out.sort_by_key(|(tte, _)| tte.idx);
        Ok(out)
    }

    pub(crate) fn get_db_data(&self, empty_transfers: bool) -> Result<DbData, InternalError> {
        let batch_transfers = self.iter_batch_transfers()?;
        let asset_transfers = self.iter_asset_transfers()?;
        let colorings = self.iter_colorings()?;
        let transfers = if empty_transfers {
            vec![]
        } else {
            self.iter_transfers()?
        };
        let txos = self.iter_txos()?;
        Ok(DbData {
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
        let batch_transfers = batch_transfers.unwrap_or_else(|| self.iter_batch_transfers().unwrap_or_default());
        let asset_transfers = asset_transfers.unwrap_or_else(|| self.iter_asset_transfers().unwrap_or_default());
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
                            if at.asset_id.as_ref().map(|id| id.as_str()) != Some(asset_id.as_str()) {
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
            .expect("asset transfer");
        let transfer = ad.transfers.first().expect("transfer");
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
            let asset_transfer = asset_transfers
                .iter()
                .find(|t| t.idx == c.asset_transfer_idx)
                .expect("coloring -> asset_transfer");
            let batch_transfer = batch_transfers
                .iter()
                .find(|t| asset_transfer.batch_transfer_idx == t.idx)
                .expect("asset_transfer -> batch_transfer");

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
        let batch_transfers = batch_transfers.unwrap_or_else(|| self.iter_batch_transfers().unwrap_or_default());
        let asset_transfers = asset_transfers.unwrap_or_else(|| self.iter_asset_transfers().unwrap_or_default());
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
