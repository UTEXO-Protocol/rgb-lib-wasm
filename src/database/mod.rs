pub(crate) mod memory_db;

use super::*;

pub(crate) use crate::database::memory_db::{
    ActiveValue, DbAsset, DbAssetActMod, DbAssetTransfer, DbAssetTransferActMod, DbBackupInfo,
    DbBackupInfoActMod, DbBatchTransfer, DbBatchTransferActMod, DbColoring, DbColoringActMod,
    DbMedia, DbMediaActMod, DbPendingWitnessScript, DbPendingWitnessScriptActMod, DbTransfer,
    DbTransferActMod, DbTransferTransportEndpoint, DbTransferTransportEndpointActMod,
    DbTransportEndpoint, DbTransportEndpointActMod, DbTxo, DbTxoActMod, DbWalletTransaction,
    DbWalletTransactionActMod,
};

#[derive(Clone, Debug)]
pub(crate) struct DbAssetTransferData {
    pub(crate) asset_transfer: DbAssetTransfer,
    pub(crate) transfers: Vec<DbTransfer>,
}

impl DbBatchTransfer {
    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn incoming(
        &self,
        asset_transfers: &[DbAssetTransfer],
        transfers: &[DbTransfer],
    ) -> Result<bool, Error> {
        let asset_transfer_ids: Vec<i32> = asset_transfers
            .iter()
            .filter(|t| t.batch_transfer_idx == self.idx)
            .map(|t| t.idx)
            .collect();
        Ok(transfers
            .iter()
            .filter(|t| asset_transfer_ids.contains(&t.asset_transfer_idx))
            .all(|t| t.incoming))
    }

    pub(crate) fn get_asset_transfers(
        &self,
        asset_transfers: &[DbAssetTransfer],
    ) -> Result<Vec<DbAssetTransfer>, InternalError> {
        Ok(asset_transfers
            .iter()
            .filter(|&t| t.batch_transfer_idx == self.idx)
            .cloned()
            .collect())
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn get_transfers(
        &self,
        asset_transfers: &[DbAssetTransfer],
        transfers: &[DbTransfer],
    ) -> Result<DbBatchTransferData, InternalError> {
        let asset_transfers = self.get_asset_transfers(asset_transfers)?;
        let mut asset_transfers_data = vec![];
        for asset_transfer in asset_transfers {
            let transfers: Vec<DbTransfer> = transfers
                .iter()
                .filter(|&t| asset_transfer.idx == t.asset_transfer_idx)
                .cloned()
                .collect();
            asset_transfers_data.push(DbAssetTransferData {
                asset_transfer,
                transfers,
            })
        }
        Ok(DbBatchTransferData {
            asset_transfers_data,
        })
    }

    pub(crate) fn failed(&self) -> bool {
        self.status.failed()
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn pending(&self) -> bool {
        self.status.pending()
    }

    pub(crate) fn waiting_confirmations(&self) -> bool {
        self.status.waiting_confirmations()
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn waiting_counterparty(&self) -> bool {
        self.status.waiting_counterparty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DbBatchTransferData {
    pub(crate) asset_transfers_data: Vec<DbAssetTransferData>,
}

impl DbColoring {
    pub(crate) fn incoming(&self) -> bool {
        [
            ColoringType::Receive,
            ColoringType::Change,
            ColoringType::Issue,
        ]
        .contains(&self.r#type)
    }
}

pub(crate) struct DbData {
    pub(crate) batch_transfers: Vec<DbBatchTransfer>,
    pub(crate) asset_transfers: Vec<DbAssetTransfer>,
    pub(crate) transfers: Vec<DbTransfer>,
    pub(crate) colorings: Vec<DbColoring>,
    pub(crate) txos: Vec<DbTxo>,
}

impl DbTransfer {
    pub(crate) fn related_transfers(
        &self,
        asset_transfers: &[DbAssetTransfer],
        batch_transfers: &[DbBatchTransfer],
    ) -> Result<(DbAssetTransfer, DbBatchTransfer), InternalError> {
        let asset_transfer = asset_transfers
            .iter()
            .find(|t| t.idx == self.asset_transfer_idx)
            .expect("transfer should be connected to an asset transfer");
        let batch_transfer = batch_transfers
            .iter()
            .find(|t| t.idx == asset_transfer.batch_transfer_idx)
            .expect("asset transfer should be connected to a batch transfer");

        Ok((asset_transfer.clone(), batch_transfer.clone()))
    }
}

impl DbTxo {
    pub(crate) fn outpoint(&self) -> Outpoint {
        Outpoint {
            txid: self.txid.to_string(),
            vout: self.vout,
        }
    }
}

impl From<DbTxo> for BdkOutPoint {
    fn from(x: DbTxo) -> BdkOutPoint {
        BdkOutPoint::from_str(&x.outpoint().to_string())
            .expect("DB should contain a valid outpoint")
    }
}

impl From<LocalOutput> for DbTxoActMod {
    fn from(x: LocalOutput) -> DbTxoActMod {
        DbTxoActMod {
            idx: ActiveValue::NotSet,
            txid: ActiveValue::Set(x.outpoint.txid.to_string()),
            vout: ActiveValue::Set(x.outpoint.vout),
            btc_amount: ActiveValue::Set(x.txout.value.to_sat().to_string()),
            spent: ActiveValue::Set(false),
            exists: ActiveValue::Set(true),
            pending_witness: ActiveValue::Set(false),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LocalTransportEndpoint {
    pub transport_type: TransportType,
    pub endpoint: String,
    pub used: bool,
    pub usable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalUnspent {
    /// Database UTXO
    pub utxo: DbTxo,
    /// RGB allocations on the UTXO
    pub rgb_allocations: Vec<LocalRgbAllocation>,
    /// Number of pending blind receive operations
    pub pending_blinded: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LocalWitnessData {
    pub amount_sat: u64,
    pub blinding: Option<u64>,
    pub vout: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum LocalRecipientData {
    Blind(SecretSeal),
    Witness(LocalWitnessData),
}

impl LocalRecipientData {
    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn vout(&self) -> Option<u32> {
        match &self {
            LocalRecipientData::Blind(_) => None,
            LocalRecipientData::Witness(d) => Some(d.vout),
        }
    }
}

#[cfg(feature = "esplora")]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LocalRecipient {
    pub recipient_id: String,
    pub local_recipient_data: LocalRecipientData,
    pub assignment: Assignment,
    pub transport_endpoints: Vec<LocalTransportEndpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct LocalRgbAllocation {
    /// Asset ID
    pub asset_id: Option<String>,
    /// RGB assignment
    pub assignment: Assignment,
    /// The status of the transfer that produced the RGB allocation
    pub status: TransferStatus,
    /// Defines if the allocation is incoming
    pub incoming: bool,
    /// Defines if the allocation is on a spent TXO
    pub txo_spent: bool,
}

impl LocalRgbAllocation {
    pub(crate) fn settled(&self) -> bool {
        !self.status.failed()
            && ((!self.txo_spent && self.incoming && self.status.settled())
                || (self.txo_spent && !self.incoming && self.status.waiting_confirmations()))
    }

    pub(crate) fn future(&self) -> bool {
        !self.txo_spent && self.incoming && !self.status.failed() && !self.settled()
    }
}

#[derive(Debug)]
pub(crate) struct TransferData {
    pub(crate) kind: TransferKind,
    pub(crate) status: TransferStatus,
    pub(crate) batch_transfer_idx: i32,
    pub(crate) assignments: Vec<Assignment>,
    pub(crate) txid: Option<String>,
    pub(crate) receive_utxo: Option<Outpoint>,
    pub(crate) change_utxo: Option<Outpoint>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) expiration: Option<i64>,
    pub(crate) consignment_path: Option<String>,
}

pub(crate) mod enums;
