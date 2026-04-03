//! RGB utilities
//!
//! This module defines some utility methods.

use super::*;

const TIMESTAMP_FORMAT: &[time::format_description::BorrowedFormatItem] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour repr:24]:[minute]:[second].[subsecond digits:3]+00"
);

pub(crate) const RGB_RUNTIME_DIR: &str = "rgb";
pub(crate) const LOG_FILE: &str = "log";

pub(crate) const PURPOSE: u8 = 86;
pub(crate) const COIN_RGB_MAINNET: u32 = 827166;
pub(crate) const COIN_RGB_TESTNET: u32 = 827167;
pub(crate) const ACCOUNT: u8 = 0;
pub(crate) const KEYCHAIN_RGB: u8 = 0;
pub(crate) const KEYCHAIN_BTC: u8 = 0;

#[cfg(feature = "esplora")]
pub(crate) const INDEXER_STOP_GAP: usize = 20;
#[cfg(feature = "esplora")]
pub(crate) const INDEXER_TIMEOUT: u8 = 10;
#[cfg(feature = "esplora")]
pub(crate) const INDEXER_RETRIES: u8 = 3;
#[cfg(feature = "esplora")]
pub(crate) const INDEXER_PARALLEL_REQUESTS: usize = 5;

#[cfg(feature = "esplora")]
#[derive(Clone)]
pub(crate) struct WasmSleeper;

#[cfg(feature = "esplora")]
impl esplora_client::Sleeper for WasmSleeper {
    type Sleep = core::future::Ready<()>;

    fn sleep(_duration: std::time::Duration) -> Self::Sleep {
        core::future::ready(())
    }
}

#[cfg(feature = "esplora")]
const PROXY_PROTOCOL_VERSION: &str = "0.2";

/// Supported Bitcoin networks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum BitcoinNetwork {
    /// Bitcoin's mainnet
    Mainnet,
    /// Bitcoin's testnet3
    Testnet,
    /// Bitcoin's testnet4
    Testnet4,
    /// Bitcoin's default signet
    Signet,
    /// Bitcoin's regtest
    Regtest,
    /// Bitcoin's custom signet
    SignetCustom,
}

impl fmt::Display for BitcoinNetwork {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl FromStr for BitcoinNetwork {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        Ok(match s.as_str() {
            "mainnet" | "bitcoin" => BitcoinNetwork::Mainnet,
            "testnet" | "testnet3" => BitcoinNetwork::Testnet,
            "testnet4" => BitcoinNetwork::Testnet4,
            "regtest" => BitcoinNetwork::Regtest,
            "signet" => BitcoinNetwork::Signet,
            "signetcustom" => BitcoinNetwork::SignetCustom,
            _ => {
                return Err(Error::InvalidBitcoinNetwork {
                    network: s.to_string(),
                });
            }
        })
    }
}

impl TryFrom<ChainNet> for BitcoinNetwork {
    type Error = Error;

    fn try_from(x: ChainNet) -> Result<Self, Self::Error> {
        match x {
            ChainNet::BitcoinMainnet => Ok(BitcoinNetwork::Mainnet),
            ChainNet::BitcoinTestnet3 => Ok(BitcoinNetwork::Testnet),
            ChainNet::BitcoinTestnet4 => Ok(BitcoinNetwork::Testnet4),
            ChainNet::BitcoinSignet => Ok(BitcoinNetwork::Signet),
            ChainNet::BitcoinRegtest => Ok(BitcoinNetwork::Regtest),
            ChainNet::BitcoinSignetCustom => Ok(BitcoinNetwork::SignetCustom),
            _ => Err(Error::UnsupportedLayer1 {
                layer_1: x.layer1().to_string(),
            }),
        }
    }
}

impl From<BitcoinNetwork> for bitcoin::Network {
    fn from(x: BitcoinNetwork) -> bitcoin::Network {
        match x {
            BitcoinNetwork::Mainnet => bitcoin::Network::Bitcoin,
            BitcoinNetwork::Testnet => bitcoin::Network::Testnet,
            BitcoinNetwork::Testnet4 => bitcoin::Network::Testnet4,
            BitcoinNetwork::Signet => bitcoin::Network::Signet,
            BitcoinNetwork::Regtest => bitcoin::Network::Regtest,
            BitcoinNetwork::SignetCustom => bitcoin::Network::Signet,
        }
    }
}

impl From<BitcoinNetwork> for NetworkKind {
    fn from(x: BitcoinNetwork) -> Self {
        match x {
            BitcoinNetwork::Mainnet => Self::Main,
            _ => Self::Test,
        }
    }
}

impl From<BitcoinNetwork> for ChainNet {
    fn from(x: BitcoinNetwork) -> ChainNet {
        match x {
            BitcoinNetwork::Mainnet => ChainNet::BitcoinMainnet,
            BitcoinNetwork::Testnet => ChainNet::BitcoinTestnet3,
            BitcoinNetwork::Testnet4 => ChainNet::BitcoinTestnet4,
            BitcoinNetwork::Signet => ChainNet::BitcoinSignet,
            BitcoinNetwork::Regtest => ChainNet::BitcoinRegtest,
            BitcoinNetwork::SignetCustom => ChainNet::BitcoinSignetCustom,
        }
    }
}

pub(crate) fn adjust_canonicalization<P: AsRef<Path>>(p: P) -> String {
    p.as_ref().display().to_string()
}

fn deserialize_str_or_number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Copy,
    T::Err: fmt::Display,
{
    struct StringOrNumberVisitor<T>(std::marker::PhantomData<T>);

    impl<T> Visitor<'_> for StringOrNumberVisitor<T>
    where
        T: FromStr + Copy,
        T::Err: fmt::Display,
    {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string, a number, or null")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            T::from_str(&value.to_string())
                .map(Some)
                .map_err(de::Error::custom)
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            T::from_str(&value.to_string())
                .map(Some)
                .map_err(de::Error::custom)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.parse::<T>().map(Some).map_err(|e| {
                de::Error::invalid_value(Unexpected::Str(value), &e.to_string().as_str())
            })
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }
    }

    deserializer.deserialize_any(StringOrNumberVisitor(std::marker::PhantomData))
}

pub(crate) fn from_str_or_number_mandatory<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Copy,
    T::Err: fmt::Display,
{
    match deserialize_str_or_number(deserializer)? {
        Some(val) => Ok(val),
        None => Err(de::Error::custom("expected a number but got null")),
    }
}

pub(crate) fn from_str_or_number_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Copy,
    T::Err: fmt::Display,
{
    deserialize_str_or_number(deserializer)
}

pub(crate) fn str_to_xpub(xpub: &str, bdk_network: BdkNetwork) -> Result<Xpub, Error> {
    let pubkey_btc = Xpub::from_str(xpub)?;
    let extended_key_btc: ExtendedKey = ExtendedKey::from(pubkey_btc);
    Ok(extended_key_btc.into_xpub(bdk_network, &Secp256k1::new()))
}

pub(crate) fn get_coin_type(bitcoin_network: &BitcoinNetwork, rgb: bool) -> u32 {
    match (bitcoin_network, rgb) {
        (BitcoinNetwork::Mainnet, true) => COIN_RGB_MAINNET,
        (_, true) => COIN_RGB_TESTNET,
        (_, false) => u32::from(*bitcoin_network != BitcoinNetwork::Mainnet),
    }
}

pub(crate) fn get_account_derivation_children(coin_type: u32) -> Vec<ChildNumber> {
    vec![
        ChildNumber::from_hardened_idx(PURPOSE as u32).unwrap(),
        ChildNumber::from_hardened_idx(coin_type).unwrap(),
        ChildNumber::from_hardened_idx(ACCOUNT as u32).unwrap(),
    ]
}

fn derive_account_xprv_from_mnemonic(
    bitcoin_network: BitcoinNetwork,
    mnemonic: &str,
    rgb: bool,
) -> Result<(Xpriv, Fingerprint), Error> {
    let coin_type = get_coin_type(&bitcoin_network, rgb);
    let account_derivation_children = get_account_derivation_children(coin_type);
    let mnemonic = Mnemonic::parse_in(Language::English, mnemonic.to_string())?;
    let master_xprv = Xpriv::new_master(bitcoin_network, &mnemonic.to_seed("")).unwrap();
    let master_xpub = Xpub::from_priv(&Secp256k1::new(), &master_xprv);
    let master_fingerprint = master_xpub.fingerprint();
    let account_xprv = master_xprv.derive_priv(&Secp256k1::new(), &account_derivation_children)?;
    Ok((account_xprv, master_fingerprint))
}

fn get_xpub_from_xprv(xprv: &Xpriv) -> Xpub {
    Xpub::from_priv(&Secp256k1::new(), xprv)
}

/// Get the account-level xPriv and xPub for the given mnemonic and Bitcoin network based on the
/// requested wallet side (colored or vanilla)
pub fn get_account_data(
    bitcoin_network: BitcoinNetwork,
    mnemonic: &str,
    rgb: bool,
) -> Result<(Xpriv, Xpub, Fingerprint), Error> {
    let (account_xprv, master_fingerprint) =
        derive_account_xprv_from_mnemonic(bitcoin_network, mnemonic, rgb)?;
    let account_xpub = get_xpub_from_xprv(&account_xprv);
    Ok((account_xprv, account_xpub, master_fingerprint))
}

pub(crate) fn get_account_xpubs(
    bitcoin_network: BitcoinNetwork,
    mnemonic: &str,
) -> Result<(Xpub, Xpub), Error> {
    let (_, account_xpub_vanilla, _) = get_account_data(bitcoin_network, mnemonic, false)?;
    let (_, account_xpub_colored, _) = get_account_data(bitcoin_network, mnemonic, true)?;
    Ok((account_xpub_vanilla, account_xpub_colored))
}

fn derive_descriptor(
    bitcoin_network: BitcoinNetwork,
    mnemonic: &str,
    rgb: bool,
    keychain: u8,
    expected_xpub: Xpub,
) -> Result<String, Error> {
    let (account_xprv, account_xpub, master_fingerprint) =
        get_account_data(bitcoin_network, mnemonic, rgb)?;
    if account_xpub != expected_xpub {
        return Err(Error::InvalidBitcoinKeys);
    }
    let coin_type = get_coin_type(&bitcoin_network, rgb);
    calculate_descriptor_from_xprv(&master_fingerprint, coin_type, account_xprv, keychain)
}

pub(crate) fn get_descriptors(
    bitcoin_network: BitcoinNetwork,
    mnemonic: &str,
    vanilla_keychain: Option<u8>,
    expected_xpub_btc: Xpub,
    expected_xpub_rgb: Xpub,
) -> Result<(String, String), Error> {
    let descriptor_colored = derive_descriptor(
        bitcoin_network,
        mnemonic,
        true,
        KEYCHAIN_RGB,
        expected_xpub_rgb,
    )?;
    let descriptor_vanilla = derive_descriptor(
        bitcoin_network,
        mnemonic,
        false,
        vanilla_keychain.unwrap_or(KEYCHAIN_BTC),
        expected_xpub_btc,
    )?;
    Ok((descriptor_colored, descriptor_vanilla))
}

pub(crate) fn get_descriptors_from_xpubs(
    bitcoin_network: BitcoinNetwork,
    master_fingerprint: &str,
    xpub_rgb: Xpub,
    xpub_btc: Xpub,
    vanilla_keychain: Option<u8>,
) -> Result<(String, String), Error> {
    let master_fingerprint =
        Fingerprint::from_str(master_fingerprint).map_err(|_| Error::InvalidFingerprint)?;
    let descriptor_colored = calculate_descriptor_from_xpub(
        &master_fingerprint,
        get_coin_type(&bitcoin_network, true),
        xpub_rgb,
        KEYCHAIN_RGB,
    )?;
    let descriptor_vanilla = calculate_descriptor_from_xpub(
        &master_fingerprint,
        get_coin_type(&bitcoin_network, false),
        xpub_btc,
        vanilla_keychain.unwrap_or(KEYCHAIN_BTC),
    )?;
    Ok((descriptor_colored, descriptor_vanilla))
}

pub(crate) fn parse_address_str(
    address: &str,
    bitcoin_network: BitcoinNetwork,
) -> Result<BdkAddress, Error> {
    BdkAddress::from_str(address)
        .map_err(|e| Error::InvalidAddress {
            details: e.to_string(),
        })?
        .require_network(bitcoin_network.into())
        .map_err(|_| Error::InvalidAddress {
            details: s!("belongs to another network"),
        })
}

/// Extract the witness script if recipient is a Witness one
pub fn script_buf_from_recipient_id(recipient_id: String) -> Result<Option<ScriptBuf>, Error> {
    let xchainnet_beneficiary =
        XChainNet::<Beneficiary>::from_str(&recipient_id).map_err(|_| Error::InvalidRecipientID)?;
    match xchainnet_beneficiary.into_inner() {
        Beneficiary::WitnessVout(pay_2_vout, _) => {
            let script_buf = pay_2_vout.to_script();
            Ok(Some(script_buf))
        }
        Beneficiary::BlindedSeal(_) => Ok(None),
    }
}

pub(crate) fn beneficiary_from_script_buf(script_buf: ScriptBuf) -> Beneficiary {
    let address_payload = AddressPayload::from_script(&script_buf).unwrap();
    Beneficiary::WitnessVout(Pay2Vout::new(address_payload), None)
}

/// Return the recipient ID for a specific script buf
pub fn recipient_id_from_script_buf(
    script_buf: ScriptBuf,
    bitcoin_network: BitcoinNetwork,
) -> String {
    let beneficiary = beneficiary_from_script_buf(script_buf);
    XChainNet::with(bitcoin_network.into(), beneficiary).to_string()
}

fn get_derivation_path(keychain: u8) -> DerivationPath {
    let derivation_path = vec![ChildNumber::from_normal_idx(keychain as u32).unwrap()];
    DerivationPath::from_iter(derivation_path.clone())
}

pub(crate) fn get_extended_derivation_path(
    mut account_derivation_children: Vec<ChildNumber>,
    keychain: u8,
) -> DerivationPath {
    let keychain_child = ChildNumber::from_normal_idx(keychain as u32).unwrap();
    account_derivation_children.push(keychain_child);
    DerivationPath::from_iter(account_derivation_children.clone())
}

pub(crate) fn calculate_descriptor_from_xprv(
    master_fingerprint: &Fingerprint,
    coin_type: u32,
    xprv: Xpriv,
    keychain: u8,
) -> Result<String, Error> {
    let path = get_derivation_path(keychain);
    let der_xprv = &xprv
        .derive_priv(&Secp256k1::new(), &path)
        .expect("provided path should be derivable in an xprv");
    let account_derivation_children = get_account_derivation_children(coin_type);
    let full_path = get_extended_derivation_path(account_derivation_children, keychain);
    let origin_prv: KeySource = (*master_fingerprint, full_path.clone());
    let der_xprv_desc_key: DescriptorKey<Segwitv0> = der_xprv
        .into_descriptor_key(Some(origin_prv), DerivationPath::default())
        .expect("should be able to convert xprv in a descriptor key");
    let key = if let Secret(key, _, _) = der_xprv_desc_key {
        key
    } else {
        return Err(InternalError::Unexpected)?;
    };
    Ok(format!("tr({key})"))
}

pub(crate) fn calculate_descriptor_from_xpub(
    master_fingerprint: &Fingerprint,
    coin_type: u32,
    xpub: Xpub,
    keychain: u8,
) -> Result<String, Error> {
    let path = get_derivation_path(keychain);
    let der_xpub = &xpub
        .derive_pub(&Secp256k1::new(), &path)
        .expect("provided path should be derivable in an xpub");
    let account_derivation_children = get_account_derivation_children(coin_type);
    let full_path = get_extended_derivation_path(account_derivation_children, keychain);
    let origin_pub: KeySource = (*master_fingerprint, full_path);
    let der_xpub_desc_key: DescriptorKey<Segwitv0> = der_xpub
        .into_descriptor_key(Some(origin_pub), DerivationPath::default())
        .expect("should be able to convert xpub in a descriptor key");
    let key = if let Public(key, _, _) = der_xpub_desc_key {
        key
    } else {
        return Err(InternalError::Unexpected)?;
    };
    Ok(format!("tr({key})"))
}

#[cfg(feature = "esplora")]
pub(crate) async fn check_proxy_async(proxy_url: &str) -> Result<(), Error> {
    use crate::api::proxy::WasmProxyClient;
    let client = WasmProxyClient::new()?;
    let mut err_details = s!("unable to connect to proxy");
    if let Ok(server_info) = client.get_info(proxy_url).await {
        if let Some(info) = server_info.result {
            if info.protocol_version == *PROXY_PROTOCOL_VERSION {
                return Ok(());
            } else {
                return Err(Error::InvalidProxyProtocol {
                    version: info.protocol_version,
                });
            }
        }
        if let Some(err) = server_info.error {
            err_details = err.message;
        }
    }
    Err(Error::Proxy {
        details: err_details,
    })
}

#[cfg(feature = "esplora")]
pub(crate) fn build_indexer(indexer_url: &str) -> Option<Indexer> {
    use crate::wallet::online::Indexer;
    let opts = esplora_client::Builder::new(indexer_url).timeout(INDEXER_TIMEOUT.into());
    let client = opts.build_async_with_sleeper::<WasmSleeper>().ok()?;
    Some(Indexer::EsploraAsync(Box::new(client)))
}

fn convert_time_fmt_error(cause: time::error::Format) -> io::Error {
    io::Error::other(cause)
}

fn log_timestamp(io: &mut dyn io::Write) -> io::Result<()> {
    let now: time::OffsetDateTime = now();
    write!(
        io,
        "{}",
        now.format(TIMESTAMP_FORMAT)
            .map_err(convert_time_fmt_error)?
    )
}

pub(crate) fn setup_logger<P: AsRef<Path>>(
    _log_path: P,
    _log_name: Option<&str>,
) -> Result<(Logger, ()), Error> {
    let drain = slog::Discard;
    let logger = Logger::root(drain, o!());
    Ok((logger, ()))
}

pub(crate) fn now() -> OffsetDateTime {
    let ms = js_sys::Date::now();
    let secs = (ms / 1000.0).floor() as i64;
    OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

pub(crate) struct DumbResolver;

impl ResolveWitness for DumbResolver {
    fn resolve_witness(&self, _: RgbTxid) -> Result<WitnessStatus, WitnessResolverError> {
        unreachable!()
    }

    fn check_chain_net(&self, _: ChainNet) -> Result<(), WitnessResolverError> {
        Ok(())
    }
}

/// Wrapper for the RGB stock.
/// On drop, the stock is saved back to the shared RefCell for persistence across calls.
pub(crate) struct RgbRuntime {
    /// The RGB stock
    pub(crate) stock: Stock,
    /// The wallet directory
    wallet_dir: PathBuf,
    /// Shared storage for persisting the stock back on drop (WASM in-memory persistence)
    shared_stock: Option<std::rc::Rc<std::cell::RefCell<Option<Stock>>>>,
}

impl RgbRuntime {
    pub(crate) fn with_shared_stock(
        stock: Stock,
        wallet_dir: PathBuf,
        shared: std::rc::Rc<std::cell::RefCell<Option<Stock>>>,
    ) -> Self {
        Self {
            stock,
            wallet_dir,
            shared_stock: Some(shared),
        }
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn accept_transfer<R: ResolveWitness>(
        &mut self,
        contract: ValidTransfer,
        resolver: &R,
    ) -> Result<Status, InternalError> {
        self.stock
            .accept_transfer(contract, resolver)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn consume_fascia(
        &mut self,
        fascia: Fascia,
        witness_ord: Option<WitnessOrd>,
    ) -> Result<(), InternalError> {
        struct FasciaResolver {
            witness_id: RgbTxid,
            witness_ord: WitnessOrd,
        }
        impl WitnessOrdProvider for FasciaResolver {
            fn witness_ord(&self, witness_id: RgbTxid) -> Result<WitnessOrd, WitnessResolverError> {
                debug_assert_eq!(witness_id, self.witness_id);
                Ok(self.witness_ord)
            }
        }

        let resolver = FasciaResolver {
            witness_id: fascia.witness_id(),
            witness_ord: witness_ord.unwrap_or(WitnessOrd::Tentative),
        };

        self.stock
            .consume_fascia(fascia, resolver)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn contracts(&self) -> Result<Vec<ContractInfo>, InternalError> {
        Ok(self
            .stock
            .contracts()
            .map_err(InternalError::from)?
            .collect())
    }

    pub(crate) fn contract_wrapper<C: IssuerWrapper>(
        &self,
        contract_id: ContractId,
    ) -> Result<C::Wrapper<MemContract<&MemContractState>>, InternalError> {
        self.stock
            .contract_wrapper::<C>(contract_id)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn contracts_assigning(
        &self,
        outputs: impl IntoIterator<Item = impl Into<OutPoint>>,
    ) -> Result<BTreeSet<ContractId>, InternalError> {
        Ok(FromIterator::from_iter(
            self.stock
                .contracts_assigning(outputs)
                .map_err(InternalError::from)?,
        ))
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn genesis(&self, contract_id: ContractId) -> Result<&Genesis, InternalError> {
        self.stock
            .as_stash_provider()
            .genesis(contract_id)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn import_contract<R: ResolveWitness>(
        &mut self,
        contract: ValidContract,
        resolver: &R,
    ) -> Result<Status, InternalError> {
        self.stock
            .import_contract(contract, resolver)
            .map_err(InternalError::from)
    }

    pub(crate) fn import_kit(&mut self, kit: ValidKit) -> Result<Status, InternalError> {
        self.stock.import_kit(kit).map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn contract_assignments_for(
        &self,
        contract_id: ContractId,
        outpoints: impl IntoIterator<Item = impl Into<OutPoint>>,
    ) -> Result<HashMap<OutputSeal, HashMap<Opout, AllocatedState>>, InternalError> {
        self.stock
            .contract_assignments_for(contract_id, outpoints)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn contract_schema(
        &self,
        contract_id: ContractId,
    ) -> Result<&Schema, InternalError> {
        self.stock
            .as_stash_provider()
            .contract_schema(contract_id)
            .map_err(InternalError::from)
    }

    pub(crate) fn schemata(&self) -> Result<Vec<SchemaInfo>, InternalError> {
        Ok(self
            .stock
            .schemata()
            .map_err(InternalError::from)?
            .collect())
    }

    pub(crate) fn store_secret_seal(&mut self, seal: GraphSeal) -> Result<bool, InternalError> {
        self.stock
            .store_secret_seal(seal)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn transfer(
        &self,
        contract_id: ContractId,
        outputs: impl AsRef<[OutputSeal]>,
        secret_seals: impl AsRef<[SecretSeal]>,
        witness_id: Option<RgbTxid>,
    ) -> Result<RgbTransfer, InternalError> {
        self.stock
            .transfer(contract_id, outputs, secret_seals, [], witness_id)
            .map_err(InternalError::from)
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn transfer_with_dag(
        &self,
        contract_id: ContractId,
        outputs: impl AsRef<[OutputSeal]>,
        secret_seals: impl AsRef<[SecretSeal]>,
        witness_id: Option<RgbTxid>,
    ) -> Result<(RgbTransfer, OpoutsDagData), InternalError> {
        self.stock
            .transfer_with_dag(contract_id, outputs, secret_seals, [], witness_id)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn transfer_from_fascia(
        &self,
        contract_id: ContractId,
        outputs: impl AsRef<[OutputSeal]>,
        secret_seals: impl AsRef<[SecretSeal]>,
        fascia: &Fascia,
    ) -> Result<RgbTransfer, InternalError> {
        self.stock
            .transfer_from_fascia(contract_id, outputs, secret_seals, [], fascia)
            .map_err(InternalError::from)
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn transfer_from_fascia_with_dag(
        &self,
        contract_id: ContractId,
        outputs: impl AsRef<[OutputSeal]>,
        secret_seals: impl AsRef<[SecretSeal]>,
        fascia: &Fascia,
    ) -> Result<(RgbTransfer, OpoutsDagData), InternalError> {
        self.stock
            .transfer_from_fascia_with_dag(contract_id, outputs, secret_seals, [], fascia)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn transition_builder(
        &self,
        contract_id: ContractId,
        transition_name: impl Into<FieldName>,
    ) -> Result<TransitionBuilder, InternalError> {
        self.stock
            .transition_builder(contract_id, transition_name)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn transition_builder_raw(
        &self,
        contract_id: ContractId,
        transition_type: TransitionType,
    ) -> Result<TransitionBuilder, InternalError> {
        self.stock
            .transition_builder_raw(contract_id, transition_type)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn update_witnesses<R: ResolveWitness>(
        &mut self,
        resolver: &R,
        after_height: u32,
        force_witnesses: Vec<RgbTxid>,
    ) -> Result<UpdateRes, InternalError> {
        self.stock
            .update_witnesses(resolver, after_height, force_witnesses)
            .map_err(InternalError::from)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn upsert_witness(
        &mut self,
        witness_id: RgbTxid,
        witness_ord: WitnessOrd,
    ) -> Result<(), InternalError> {
        self.stock.upsert_witness(witness_id, witness_ord)?;
        Ok(())
    }
}

impl Drop for RgbRuntime {
    fn drop(&mut self) {
        if let Some(shared) = self.shared_stock.take() {
            let stock = std::mem::replace(&mut self.stock, Stock::in_memory());
            *shared.borrow_mut() = Some(stock);
        }
    }
}

pub(crate) fn load_rgb_runtime(_wallet_dir: PathBuf) -> Result<RgbRuntime, Error> {
    let stock = Stock::in_memory();
    Ok(RgbRuntime {
        stock,
        wallet_dir: PathBuf::new(),
        shared_stock: None,
    })
}

#[cfg(feature = "esplora")]
pub(crate) async fn fetch_esplora_broadcast_async(
    indexer_url: &str,
    tx_hex: &str,
) -> Result<(), Error> {
    use gloo_net::http::Request;
    let base = indexer_url.trim_end_matches('/');
    let url = format!("{}/tx", base);
    let resp = Request::post(&url)
        .body(tx_hex)
        .map_err(|e| Error::FailedBroadcast {
            details: e.to_string(),
        })?
        .send()
        .await
        .map_err(|e| Error::FailedBroadcast {
            details: e.to_string(),
        })?;
    if !resp.ok() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::FailedBroadcast {
            details: format!("Esplora POST /tx returned {}: {}", status, text),
        });
    }
    Ok(())
}

/// Pre-fetched witness resolver for wasm32.
///
/// Resolves witness transactions from a cache populated from consignment bundles.
#[cfg(feature = "esplora")]
pub(crate) struct WasmResolver {
    witness_cache: HashMap<RgbTxid, WitnessStatus>,
    chain_net: ChainNet,
}

#[cfg(feature = "esplora")]
impl WasmResolver {
    /// Build resolver by pre-fetching all witness IDs from a consignment.
    pub(crate) fn from_consignment<const TRANSFER: bool>(
        consignment: &Consignment<TRANSFER>,
        chain_net: ChainNet,
    ) -> Self {
        let mut cache = HashMap::new();
        for bw in consignment.bundled_witnesses() {
            let wid = bw.witness_id();
            if let Some(tx) = bw.pub_witness.tx().cloned() {
                cache.insert(wid, WitnessStatus::Resolved(tx, WitnessOrd::Tentative));
            }
        }
        Self {
            witness_cache: cache,
            chain_net,
        }
    }
}

#[cfg(feature = "esplora")]
impl ResolveWitness for WasmResolver {
    fn resolve_witness(&self, witness_id: RgbTxid) -> Result<WitnessStatus, WitnessResolverError> {
        self.witness_cache
            .get(&witness_id)
            .cloned()
            .ok_or(WitnessResolverError::ResolverIssue(
                Some(witness_id),
                s!("witness not found in cache"),
            ))
    }
    fn check_chain_net(&self, chain_net: ChainNet) -> Result<(), WitnessResolverError> {
        if self.chain_net == chain_net {
            Ok(())
        } else {
            Err(WitnessResolverError::WrongChainNet)
        }
    }
}

/// Pre-fetched witness resolver for update_witnesses in wasm32.
///
/// Resolves witness transactions from a cache pre-populated via async esplora queries.
#[cfg(feature = "esplora")]
pub(crate) struct PreFetchResolver {
    witness_cache: HashMap<RgbTxid, WitnessStatus>,
    chain_net: ChainNet,
}

#[cfg(feature = "esplora")]
impl PreFetchResolver {
    pub(crate) fn new(cache: HashMap<RgbTxid, WitnessStatus>, chain_net: ChainNet) -> Self {
        Self {
            witness_cache: cache,
            chain_net,
        }
    }
}

#[cfg(feature = "esplora")]
impl ResolveWitness for PreFetchResolver {
    fn resolve_witness(&self, witness_id: RgbTxid) -> Result<WitnessStatus, WitnessResolverError> {
        self.witness_cache
            .get(&witness_id)
            .cloned()
            .ok_or(WitnessResolverError::ResolverIssue(
                Some(witness_id),
                s!("witness not pre-fetched"),
            ))
    }
    fn check_chain_net(&self, chain_net: ChainNet) -> Result<(), WitnessResolverError> {
        if self.chain_net == chain_net {
            Ok(())
        } else {
            Err(WitnessResolverError::WrongChainNet)
        }
    }
}

/// Offchain resolver variant for wasm32.
///
/// Checks the consignment first for the specific witness ID, then falls back to WasmResolver.
#[cfg(feature = "esplora")]
pub(crate) struct OffchainResolverWasm<'a, 'cons, const TRANSFER: bool> {
    pub(crate) witness_id: RgbTxid,
    pub(crate) consignment: &'cons Consignment<TRANSFER>,
    pub(crate) fallback: &'a WasmResolver,
}

#[cfg(feature = "esplora")]
impl<const TRANSFER: bool> ResolveWitness for OffchainResolverWasm<'_, '_, TRANSFER> {
    fn resolve_witness(&self, witness_id: RgbTxid) -> Result<WitnessStatus, WitnessResolverError> {
        if witness_id != self.witness_id {
            return self.fallback.resolve_witness(witness_id);
        }
        self.consignment
            .bundled_witnesses()
            .find(|bw| bw.witness_id() == witness_id)
            .and_then(|p| p.pub_witness.tx().cloned())
            .map_or_else(
                || self.fallback.resolve_witness(witness_id),
                |tx| Ok(WitnessStatus::Resolved(tx, WitnessOrd::Tentative)),
            )
    }
    fn check_chain_net(&self, chain_net: ChainNet) -> Result<(), WitnessResolverError> {
        self.fallback.check_chain_net(chain_net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn fresh_keys() -> crate::keys::Keys {
        crate::keys::generate_keys(BitcoinNetwork::Regtest)
    }

    #[test]
    fn display_mainnet() {
        assert_eq!(BitcoinNetwork::Mainnet.to_string(), "Mainnet");
    }

    #[test]
    fn display_testnet() {
        assert_eq!(BitcoinNetwork::Testnet.to_string(), "Testnet");
    }

    #[test]
    fn display_regtest() {
        assert_eq!(BitcoinNetwork::Regtest.to_string(), "Regtest");
    }

    #[test]
    fn display_signet() {
        assert_eq!(BitcoinNetwork::Signet.to_string(), "Signet");
    }

    #[test]
    fn from_str_valid_variants() {
        // Each accepted string maps to the right variant
        assert_eq!(
            BitcoinNetwork::from_str("mainnet").unwrap(),
            BitcoinNetwork::Mainnet
        );
        assert_eq!(
            BitcoinNetwork::from_str("bitcoin").unwrap(),
            BitcoinNetwork::Mainnet
        );
        assert_eq!(
            BitcoinNetwork::from_str("testnet").unwrap(),
            BitcoinNetwork::Testnet
        );
        assert_eq!(
            BitcoinNetwork::from_str("testnet3").unwrap(),
            BitcoinNetwork::Testnet
        );
        assert_eq!(
            BitcoinNetwork::from_str("testnet4").unwrap(),
            BitcoinNetwork::Testnet4
        );
        assert_eq!(
            BitcoinNetwork::from_str("regtest").unwrap(),
            BitcoinNetwork::Regtest
        );
        assert_eq!(
            BitcoinNetwork::from_str("signet").unwrap(),
            BitcoinNetwork::Signet
        );
        // Case-insensitive
        assert_eq!(
            BitcoinNetwork::from_str("MAINNET").unwrap(),
            BitcoinNetwork::Mainnet
        );
        assert_eq!(
            BitcoinNetwork::from_str("Regtest").unwrap(),
            BitcoinNetwork::Regtest
        );
    }

    #[test]
    fn from_str_invalid_returns_error() {
        let result = BitcoinNetwork::from_str("invalidnet");
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InvalidBitcoinNetwork { network } => {
                assert_eq!(network, "invalidnet");
            }
            other => panic!("expected InvalidBitcoinNetwork, got: {other:?}"),
        }
    }

    #[test]
    fn into_bitcoin_network_all_variants() {
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::Mainnet),
            bitcoin::Network::Bitcoin
        );
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::Testnet),
            bitcoin::Network::Testnet
        );
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::Testnet4),
            bitcoin::Network::Testnet4
        );
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::Signet),
            bitcoin::Network::Signet
        );
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::Regtest),
            bitcoin::Network::Regtest
        );
        // SignetCustom maps to Signet
        assert_eq!(
            bitcoin::Network::from(BitcoinNetwork::SignetCustom),
            bitcoin::Network::Signet
        );
    }

    #[test]
    fn into_chainnet_all_variants() {
        assert_eq!(
            ChainNet::from(BitcoinNetwork::Mainnet),
            ChainNet::BitcoinMainnet
        );
        assert_eq!(
            ChainNet::from(BitcoinNetwork::Testnet),
            ChainNet::BitcoinTestnet3
        );
        assert_eq!(
            ChainNet::from(BitcoinNetwork::Testnet4),
            ChainNet::BitcoinTestnet4
        );
        assert_eq!(
            ChainNet::from(BitcoinNetwork::Signet),
            ChainNet::BitcoinSignet
        );
        assert_eq!(
            ChainNet::from(BitcoinNetwork::Regtest),
            ChainNet::BitcoinRegtest
        );
    }

    #[test]
    fn chainnet_round_trip() {
        let networks = [
            BitcoinNetwork::Mainnet,
            BitcoinNetwork::Testnet,
            BitcoinNetwork::Testnet4,
            BitcoinNetwork::Signet,
            BitcoinNetwork::Regtest,
        ];
        for net in networks {
            let chain_net = ChainNet::from(net);
            let back = BitcoinNetwork::try_from(chain_net).unwrap();
            assert_eq!(back, net, "round-trip failed for {net:?}");
        }
    }

    #[test]
    fn into_network_kind() {
        assert_eq!(
            NetworkKind::from(BitcoinNetwork::Mainnet),
            NetworkKind::Main
        );
        assert_eq!(
            NetworkKind::from(BitcoinNetwork::Testnet),
            NetworkKind::Test
        );
        assert_eq!(
            NetworkKind::from(BitcoinNetwork::Regtest),
            NetworkKind::Test
        );
        assert_eq!(NetworkKind::from(BitcoinNetwork::Signet), NetworkKind::Test);
    }

    #[test]
    fn coin_type_rgb_mainnet() {
        assert_eq!(
            get_coin_type(&BitcoinNetwork::Mainnet, true),
            COIN_RGB_MAINNET
        );
    }

    #[test]
    fn coin_type_rgb_testnet() {
        // All non-mainnet networks use the testnet RGB coin type
        assert_eq!(
            get_coin_type(&BitcoinNetwork::Testnet, true),
            COIN_RGB_TESTNET
        );
        assert_eq!(
            get_coin_type(&BitcoinNetwork::Regtest, true),
            COIN_RGB_TESTNET
        );
        assert_eq!(
            get_coin_type(&BitcoinNetwork::Signet, true),
            COIN_RGB_TESTNET
        );
    }

    #[test]
    fn coin_type_btc() {
        // BTC side: mainnet = 0, non-mainnet = 1
        assert_eq!(get_coin_type(&BitcoinNetwork::Mainnet, false), 0);
        assert_eq!(get_coin_type(&BitcoinNetwork::Testnet, false), 1);
        assert_eq!(get_coin_type(&BitcoinNetwork::Regtest, false), 1);
    }

    #[test]
    fn derive_account_xprv_produces_valid_key() {
        let (xprv, fingerprint) =
            derive_account_xprv_from_mnemonic(BitcoinNetwork::Regtest, TEST_MNEMONIC, true)
                .expect("derivation should succeed");
        // The xprv should be a valid extended private key
        let xpub = get_xpub_from_xprv(&xprv);
        assert_ne!(xpub.to_string(), "");
        // Fingerprint should be non-zero
        assert_ne!(fingerprint.to_string(), "00000000");
    }

    #[test]
    fn get_xpub_from_xprv_produces_valid_xpub() {
        let (xprv, _) =
            derive_account_xprv_from_mnemonic(BitcoinNetwork::Regtest, TEST_MNEMONIC, true)
                .unwrap();
        let xpub = get_xpub_from_xprv(&xprv);
        // xpub string should start with "xpub" or "tpub" depending on network
        let xpub_str = xpub.to_string();
        assert!(
            xpub_str.starts_with("tpub") || xpub_str.starts_with("xpub"),
            "unexpected xpub prefix: {xpub_str}"
        );
    }

    #[test]
    fn get_account_xpubs_returns_two_different_keys() {
        let (vanilla, colored) = get_account_xpubs(BitcoinNetwork::Regtest, TEST_MNEMONIC).unwrap();
        // The vanilla and colored xpubs must differ (different derivation paths)
        assert_ne!(vanilla, colored);
        // Both should be valid xpub strings
        assert!(!vanilla.to_string().is_empty());
        assert!(!colored.to_string().is_empty());
    }

    #[test]
    fn get_descriptors_produce_valid_strings() {
        let (vanilla_xpub, colored_xpub) =
            get_account_xpubs(BitcoinNetwork::Regtest, TEST_MNEMONIC).unwrap();
        let (desc_colored, desc_vanilla) = get_descriptors(
            BitcoinNetwork::Regtest,
            TEST_MNEMONIC,
            None,
            vanilla_xpub,
            colored_xpub,
        )
        .unwrap();
        // Both descriptors should start with "tr(" (taproot)
        assert!(
            desc_colored.starts_with("tr("),
            "colored descriptor: {desc_colored}"
        );
        assert!(
            desc_vanilla.starts_with("tr("),
            "vanilla descriptor: {desc_vanilla}"
        );
        // They should contain different xpub material since they derive from different coin types
        assert_ne!(desc_colored, desc_vanilla);
    }

    #[test]
    fn get_descriptors_from_xpubs_produces_valid_descriptors() {
        let keys =
            crate::keys::restore_keys(BitcoinNetwork::Regtest, TEST_MNEMONIC.to_string()).unwrap();
        let vanilla_xpub = Xpub::from_str(&keys.account_xpub_vanilla).unwrap();
        let colored_xpub = Xpub::from_str(&keys.account_xpub_colored).unwrap();

        let (desc_colored, desc_vanilla) = get_descriptors_from_xpubs(
            BitcoinNetwork::Regtest,
            &keys.master_fingerprint,
            colored_xpub,
            vanilla_xpub,
            None,
        )
        .unwrap();
        assert!(
            desc_colored.starts_with("tr("),
            "colored descriptor: {desc_colored}"
        );
        assert!(
            desc_vanilla.starts_with("tr("),
            "vanilla descriptor: {desc_vanilla}"
        );
    }

    /// Helper: derive a valid regtest address from the test mnemonic's descriptor
    fn regtest_address() -> BdkAddress {
        let (vanilla_xpub, colored_xpub) =
            get_account_xpubs(BitcoinNetwork::Regtest, TEST_MNEMONIC).unwrap();
        let (_, desc_vanilla) = get_descriptors(
            BitcoinNetwork::Regtest,
            TEST_MNEMONIC,
            None,
            vanilla_xpub,
            colored_xpub,
        )
        .unwrap();
        // Use BDK to derive address 0 from the vanilla descriptor
        let wallet = BdkWallet::create_single(desc_vanilla)
            .network(bitcoin::Network::Regtest)
            .create_wallet_no_persist()
            .expect("wallet creation should succeed");
        wallet.peek_address(KeychainKind::External, 0).address
    }

    #[test]
    fn parse_valid_regtest_address() {
        let addr = regtest_address();
        let addr_str = addr.to_string();
        let parsed = parse_address_str(&addr_str, BitcoinNetwork::Regtest);
        assert!(parsed.is_ok(), "expected valid address, got: {parsed:?}");
    }

    #[test]
    fn parse_invalid_address_returns_error() {
        let result = parse_address_str("not-an-address", BitcoinNetwork::Regtest);
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InvalidAddress { details } => {
                assert!(!details.is_empty());
            }
            other => panic!("expected InvalidAddress, got: {other:?}"),
        }
    }

    #[test]
    fn parse_address_wrong_network_returns_error() {
        // Generate a regtest address and try to parse it as mainnet
        let addr = regtest_address();
        let addr_str = addr.to_string();
        let result = parse_address_str(&addr_str, BitcoinNetwork::Mainnet);
        assert!(result.is_err());
    }

    #[test]
    fn script_buf_and_recipient_id_round_trip() {
        let addr = regtest_address();
        let script_buf = addr.script_pubkey();

        // Compute recipient_id from the script
        let recipient_id =
            recipient_id_from_script_buf(script_buf.clone(), BitcoinNetwork::Regtest);
        assert!(!recipient_id.is_empty());

        // Round-trip: parse the recipient_id back to a script
        let recovered = script_buf_from_recipient_id(recipient_id).unwrap();
        assert_eq!(recovered, Some(script_buf));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct MandatoryField {
        #[serde(deserialize_with = "from_str_or_number_mandatory")]
        value: u64,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct OptionalField {
        #[serde(deserialize_with = "from_str_or_number_optional")]
        value: Option<u64>,
    }

    #[test]
    fn mandatory_from_string_number() {
        let json = r#"{"value": "42"}"#;
        let parsed: MandatoryField = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.value, 42);
    }

    #[test]
    fn mandatory_from_integer() {
        let json = r#"{"value": 99}"#;
        let parsed: MandatoryField = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.value, 99);
    }

    #[test]
    fn mandatory_null_returns_error() {
        let json = r#"{"value": null}"#;
        let result = serde_json::from_str::<MandatoryField>(json);
        assert!(result.is_err());
    }

    #[test]
    fn optional_from_string_number() {
        let json = r#"{"value": "123"}"#;
        let parsed: OptionalField = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.value, Some(123));
    }

    #[test]
    fn optional_null_deserializes_to_none() {
        let json = r#"{"value": null}"#;
        let parsed: OptionalField = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.value, None);
    }

    #[test]
    fn adjust_canonicalization_preserves_path() {
        let p = PathBuf::from("/tmp/foo/bar");
        assert_eq!(adjust_canonicalization(&p), "/tmp/foo/bar");
    }

    #[test]
    fn get_account_derivation_children_has_three_levels() {
        let children = get_account_derivation_children(COIN_RGB_TESTNET);
        assert_eq!(children.len(), 3);
        // First child is PURPOSE (86')
        assert_eq!(
            children[0],
            ChildNumber::from_hardened_idx(PURPOSE as u32).unwrap()
        );
    }

    #[test]
    fn generate_and_restore_keys_round_trip() {
        let keys = fresh_keys();
        let restored =
            crate::keys::restore_keys(BitcoinNetwork::Regtest, keys.mnemonic.clone()).unwrap();
        assert_eq!(keys.xpub, restored.xpub);
        assert_eq!(keys.account_xpub_vanilla, restored.account_xpub_vanilla);
        assert_eq!(keys.account_xpub_colored, restored.account_xpub_colored);
        assert_eq!(keys.master_fingerprint, restored.master_fingerprint);
    }

    #[test]
    fn str_to_xpub_valid() {
        let keys = fresh_keys();
        let result = str_to_xpub(&keys.xpub, BdkNetwork::Testnet);
        assert!(result.is_ok());
    }
}
