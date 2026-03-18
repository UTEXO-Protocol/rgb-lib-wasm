use super::*;

/// The schema of an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum AssetSchema {
    /// NIA schema
    Nia = 1,
    /// IFA schema
    Ifa = 4,
}

impl fmt::Display for AssetSchema {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl TryFrom<String> for AssetSchema {
    type Error = Error;

    fn try_from(schema_id: String) -> Result<Self, Self::Error> {
        Ok(match &schema_id[..] {
            SCHEMA_ID_NIA => AssetSchema::Nia,
            SCHEMA_ID_IFA => AssetSchema::Ifa,
            _ => return Err(Error::UnknownRgbSchema { schema_id }),
        })
    }
}

impl TryFrom<SchemaId> for AssetSchema {
    type Error = Error;

    fn try_from(schema_id: SchemaId) -> Result<Self, Self::Error> {
        schema_id.to_string().try_into()
    }
}

impl AssetSchema {
    pub(crate) const VALUES: [Self; NUM_KNOWN_SCHEMAS] = [Self::Nia, Self::Ifa];

    fn from_schema_id_str(schema_id: String) -> Result<Self, Error> {
        Ok(match &schema_id[..] {
            SCHEMA_ID_NIA => AssetSchema::Nia,
            SCHEMA_ID_IFA => AssetSchema::Ifa,
            _ => return Err(Error::UnknownRgbSchema { schema_id }),
        })
    }

    /// Get [`AssetSchema`] from [`SchemaId`].
    pub fn from_schema_id(schema_id: SchemaId) -> Result<Self, Error> {
        Self::from_schema_id_str(schema_id.to_string())
    }

    pub(crate) fn get_from_contract_id(
        contract_id: ContractId,
        runtime: &RgbRuntime,
    ) -> Result<Self, Error> {
        let schema_id = runtime.genesis(contract_id)?.schema_id;
        Self::from_schema_id(schema_id)
    }

    fn schema(&self) -> Schema {
        match self {
            Self::Nia => NonInflatableAsset::schema(),
            Self::Ifa => InflatableFungibleAsset::schema(),
        }
    }

    fn scripts(&self) -> Scripts {
        match self {
            Self::Nia => NonInflatableAsset::scripts(),
            Self::Ifa => InflatableFungibleAsset::scripts(),
        }
    }

    /// Returns the type system for this asset schema.
    pub fn types(&self) -> TypeSystem {
        match self {
            Self::Nia => NonInflatableAsset::types(),
            Self::Ifa => InflatableFungibleAsset::types(),
        }
    }

    pub(crate) fn import_kit(&self, runtime: &mut RgbRuntime) -> Result<(), Error> {
        let schema = self.schema();
        let lib = self.scripts();
        let types = self.types();
        let mut kit = Kit::default();
        kit.schemata.push(schema).unwrap();
        kit.scripts.extend(lib.into_values()).unwrap();
        kit.types = types;
        let valid_kit = kit.validate().map_err(|_| InternalError::Unexpected)?;
        runtime.import_kit(valid_kit)?;
        Ok(())
    }
}

impl From<AssetSchema> for SchemaId {
    fn from(asset_schema: AssetSchema) -> Self {
        match asset_schema {
            AssetSchema::Ifa => SchemaId::from_str(SCHEMA_ID_IFA).unwrap(),
            AssetSchema::Nia => SchemaId::from_str(SCHEMA_ID_NIA).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ColoringType {
    Receive = 1,
    Issue = 2,
    Input = 3,
    Change = 4,
}

/// The type of an RGB recipient
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RecipientTypeFull {
    /// Receive via blinded UTXO
    Blind { unblinded_utxo: Outpoint },
    /// Receive via witness TX
    Witness { vout: Option<u32> },
}

/// The type of an RGB transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransportType {
    /// HTTP(s) JSON-RPC ([specification](https://github.com/RGB-Tools/rgb-http-json-rpc))
    JsonRpc = 1,
}

/// The status of a [`crate::wallet::Transfer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Deserialize, Serialize)]
pub enum TransferStatus {
    /// Waiting for the counterparty to take action
    WaitingCounterparty = 1,
    /// Waiting for the transfer transaction to reach the required number of confirmations
    WaitingConfirmations = 2,
    /// Settled transfer, this status is final
    Settled = 3,
    /// Failed transfer, this status is final
    Failed = 4,
}

impl TransferStatus {
    pub(crate) fn failed(&self) -> bool {
        self == &TransferStatus::Failed
    }

    pub(crate) fn pending(&self) -> bool {
        [
            TransferStatus::WaitingCounterparty,
            TransferStatus::WaitingConfirmations,
        ]
        .contains(self)
    }

    pub(crate) fn settled(&self) -> bool {
        self == &TransferStatus::Settled
    }

    pub(crate) fn waiting_confirmations(&self) -> bool {
        self == &TransferStatus::WaitingConfirmations
    }

    pub(crate) fn waiting_counterparty(&self) -> bool {
        self == &TransferStatus::WaitingCounterparty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum WalletTransactionType {
    CreateUtxos = 1,
    Drain = 2,
}

/// An RGB assignment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
pub enum Assignment {
    /// Fungible value in RGB units (not considering precision)
    Fungible(u64),
    /// Non-fungible value
    NonFungible,
    /// Inflation right
    InflationRight(u64),
    /// Replace right
    ReplaceRight,
    /// Any assignment
    Any,
}

impl Assignment {
    #[cfg(feature = "esplora")]
    pub(crate) fn from_opout_and_state(opout: Opout, state: &AllocatedState) -> Self {
        match state {
            AllocatedState::Amount(amt) if opout.ty == OS_ASSET => Self::Fungible(amt.as_u64()),
            AllocatedState::Amount(amt) if opout.ty == OS_INFLATION => {
                Self::InflationRight(amt.as_u64())
            }
            AllocatedState::Data(_) => Self::NonFungible,
            AllocatedState::Void if opout.ty == OS_REPLACE => Self::ReplaceRight,
            _ => unreachable!(),
        }
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn add_to_assignments(&self, assignments: &mut AssignmentsCollection) {
        match self {
            Self::Fungible(amt) => assignments.fungible += amt,
            Self::NonFungible => assignments.non_fungible = true,
            Self::InflationRight(amt) => assignments.inflation += amt,
            Self::ReplaceRight => assignments.replace += 1,
            _ => unreachable!("when using this method we should know the assignment type"),
        }
    }

    pub(crate) fn main_amount(&self) -> u64 {
        if let Self::Fungible(amt) = self {
            *amt
        } else if let Self::NonFungible = self {
            1
        } else {
            0
        }
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn inflation_amount(&self) -> u64 {
        if let Self::InflationRight(amt) = self {
            *amt
        } else {
            0
        }
    }
}
