//! RGB Rust-only methods module
//!
//! This module defines additional utility methods that are not exposed via FFI

use super::*;

/// RGB asset-specific information to color a transaction
#[derive(Clone, Debug)]
pub struct AssetColoringInfo {
    /// Map of vouts and asset amounts to color the transaction outputs
    pub output_map: HashMap<u32, u64>,
    /// Static blinding to keep the transaction construction deterministic
    pub static_blinding: Option<u64>,
}

/// RGB information to color a transaction
#[derive(Clone, Debug)]
pub struct ColoringInfo {
    /// Asset-specific information
    pub asset_info_map: HashMap<ContractId, AssetColoringInfo>,
    /// Static blinding to keep the transaction construction deterministic
    pub static_blinding: Option<u64>,
    /// Nonce for offchain TXs ordering
    pub nonce: Option<u64>,
}

/// Map of contract ID and list of its beneficiaries
pub type AssetBeneficiariesMap = BTreeMap<ContractId, Vec<BuilderSeal<GraphSeal>>>;

/// Indexer protocol
#[derive(Clone, Debug)]
pub enum IndexerProtocol {
    /// An indexer implementing the esplora protocol
    Esplora,
}

impl fmt::Display for IndexerProtocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Result of consignment validation (offchain or indexer-based).
#[cfg(feature = "esplora")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateConsignmentResult {
    /// Whether the consignment is valid.
    pub valid: bool,
    /// Warnings from validation (when valid).
    pub warnings: Option<Vec<String>>,
    /// Error category (when invalid): "invalid" or "resolver".
    pub error: Option<String>,
    /// Detailed error/failure description (when invalid).
    pub details: Option<String>,
}

impl Wallet {
    /// Color a PSBT.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn color_psbt(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
    ) -> Result<(Fascia, AssetBeneficiariesMap), Error> {
        info!(self.logger, "Coloring PSBT...");
        let mut transaction = match psbt.clone().extract_tx() {
            Ok(tx) => tx,
            Err(ExtractTxError::MissingInputValue { tx }) => tx, // required for non-standard TXs
            Err(e) => return Err(InternalError::from(e).into()),
        };
        let mut opreturn_first = false;
        if transaction.output.iter().any(|o| o.script_pubkey.is_p2tr()) {
            opreturn_first = true;
        }

        if !transaction
            .output
            .iter()
            .any(|o| o.script_pubkey.is_op_return())
        {
            let opreturn_output = TxOut {
                value: BdkAmount::ZERO,
                script_pubkey: ScriptBuf::new_op_return([]),
            };
            if opreturn_first {
                transaction.output.insert(0, opreturn_output);
            } else {
                transaction.output.push(opreturn_output);
            }
            *psbt = Psbt::from_unsigned_tx(transaction).unwrap();
        }

        let runtime = self.rgb_runtime()?;

        let prev_outputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<HashSet<OutPoint>>();

        let mut all_transitions: HashMap<ContractId, Transition> = HashMap::new();
        let mut asset_beneficiaries: AssetBeneficiariesMap = bmap![];
        let assignment_name = FieldName::from(RGB_STATE_ASSET_OWNER);

        for (contract_id, asset_coloring_info) in coloring_info.asset_info_map.clone() {
            let schema = AssetSchema::get_from_contract_id(contract_id, &runtime)?;

            let mut asset_transition_builder =
                runtime.transition_builder(contract_id, "transfer")?;

            let mut asset_available_amt = 0;
            for (_, opout_state_map) in
                runtime.contract_assignments_for(contract_id, prev_outputs.iter().copied())?
            {
                for (opout, state) in opout_state_map {
                    if let AllocatedState::Amount(amt) = &state {
                        asset_available_amt += amt.as_u64();
                    }
                    asset_transition_builder = asset_transition_builder.add_input(opout, state)?;
                }
            }

            let mut beneficiaries = vec![];
            let mut sending_amt = 0;
            for (mut vout, amount) in asset_coloring_info.output_map {
                if amount == 0 {
                    continue;
                }
                if opreturn_first {
                    vout += 1;
                }
                sending_amt += amount;
                if vout as usize > psbt.outputs.len() {
                    return Err(Error::InvalidColoringInfo {
                        details: s!("invalid vout in output_map, does not exist in the given PSBT"),
                    });
                }
                let graph_seal = if let Some(blinding) = asset_coloring_info.static_blinding {
                    GraphSeal::with_blinded_vout(vout, blinding)
                } else {
                    GraphSeal::new_random_vout(vout)
                };
                let seal = BuilderSeal::Revealed(graph_seal);
                beneficiaries.push(seal);

                match schema {
                    AssetSchema::Nia | AssetSchema::Ifa => {
                        asset_transition_builder = asset_transition_builder.add_fungible_state(
                            assignment_name.clone(),
                            seal,
                            amount,
                        )?;
                    }
                }
            }
            if sending_amt > asset_available_amt {
                return Err(Error::InvalidColoringInfo {
                    details: format!(
                        "total amount in output_map ({sending_amt}) greater than available ({asset_available_amt})"
                    ),
                });
            }

            if let Some(nonce) = coloring_info.nonce {
                asset_transition_builder = asset_transition_builder.set_nonce(nonce);
            }

            let transition = asset_transition_builder.complete_transition()?;
            all_transitions.insert(contract_id, transition);
            asset_beneficiaries.insert(contract_id, beneficiaries);
        }

        let opreturn_index = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey.is_op_return())
            .expect("psbt should have an op_return output")
            .0;
        let opreturn_output = psbt.outputs.get_mut(opreturn_index).unwrap();
        opreturn_output.set_opret_host();
        if let Some(blinding) = coloring_info.static_blinding {
            opreturn_output
                .set_mpc_entropy(blinding)
                .map_err(InternalError::from)?;
        }

        for (contract_id, transition) in all_transitions {
            for opout in transition.inputs() {
                psbt.set_rgb_contract_consumer(contract_id, opout, transition.id())
                    .map_err(InternalError::from)?;
            }
            psbt.push_rgb_transition(transition)
                .map_err(InternalError::from)?;
        }

        psbt.set_rgb_close_method(CloseMethod::OpretFirst);
        psbt.set_as_unmodifiable();
        let fascia = psbt.rgb_commit().map_err(|e| Error::Internal {
            details: e.to_string(),
        })?;

        info!(self.logger, "Color PSBT completed");
        Ok((fascia, asset_beneficiaries))
    }

    /// Color a PSBT, consume the RGB fascia and return the related consignment.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn color_psbt_and_consume(
        &self,
        psbt: &mut Psbt,
        coloring_info: ColoringInfo,
    ) -> Result<Vec<RgbTransfer>, Error> {
        info!(self.logger, "Coloring PSBT and consuming...");
        let (fascia, asset_beneficiaries) = self.color_psbt(psbt, coloring_info.clone())?;

        let witness_txid = psbt.get_txid();

        let mut runtime = self.rgb_runtime()?;
        runtime.consume_fascia(fascia, None)?;

        let mut transfers = vec![];
        for (contract_id, beneficiaries) in asset_beneficiaries {
            let mut beneficiaries_witness = vec![];
            let mut beneficiaries_blinded = vec![];
            for builder_seal in beneficiaries {
                match builder_seal {
                    BuilderSeal::Revealed(seal) => {
                        let explicit_seal = ExplicitSeal::with(witness_txid, seal.vout);
                        beneficiaries_witness.push(explicit_seal);
                    }
                    BuilderSeal::Concealed(secret_seal) => {
                        beneficiaries_blinded.push(secret_seal);
                    }
                };
            }
            transfers.push(runtime.transfer(
                contract_id,
                beneficiaries_witness,
                beneficiaries_blinded,
                Some(witness_txid),
            )?);
        }

        info!(self.logger, "Color PSBT and consume completed");
        Ok(transfers)
    }

    /// Consume an RGB fascia.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn consume_fascia(
        &self,
        fascia: Fascia,
        witness_ord: Option<WitnessOrd>,
    ) -> Result<(), Error> {
        info!(self.logger, "Consuming fascia...");
        self.rgb_runtime()?
            .consume_fascia(fascia.clone(), witness_ord)?;
        info!(self.logger, "Consume fascia completed");
        Ok(())
    }

    /// Manually set the [`WitnessOrd`] of a witness TX.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub fn upsert_witness(
        &self,
        witness_id: RgbTxid,
        witness_ord: WitnessOrd,
    ) -> Result<(), Error> {
        let mut runtime = self.rgb_runtime()?;
        runtime.upsert_witness(witness_id, witness_ord)?;
        Ok(())
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn save_new_asset_internal(
        &self,
        runtime: &RgbRuntime,
        contract_id: ContractId,
        asset_schema: AssetSchema,
        valid_contract: ValidContract,
        valid_transfer: ValidTransfer,
    ) -> Result<(), Error> {
        let timestamp = valid_contract.genesis.timestamp;
        let local_asset_data = match &asset_schema {
            AssetSchema::Nia => {
                let contract = runtime.contract_wrapper::<NonInflatableAsset>(contract_id)?;
                let spec = contract.spec();
                let ticker = spec.ticker().to_string();
                let name = spec.name().to_string();
                let details = spec.details().map(|d| d.to_string());
                let precision = spec.precision.into();
                let initial_supply = contract.total_issued_supply().into();
                let media_idx = if let Some(attachment) = contract.contract_terms().media {
                    Some(self.get_or_insert_media(
                        hex::encode(attachment.digest),
                        attachment.ty.to_string(),
                    )?)
                } else {
                    None
                };
                LocalAssetData {
                    name,
                    precision,
                    ticker: Some(ticker),
                    details,
                    media_idx,
                    initial_supply,
                    max_supply: None,
                    known_circulating_supply: None,
                    reject_list_url: None,
                }
            }
            AssetSchema::Ifa => {
                let contract = runtime.contract_wrapper::<InflatableFungibleAsset>(contract_id)?;
                let spec = contract.spec();
                let ticker = spec.ticker().to_string();
                let name = spec.name().to_string();
                let details = spec.details().map(|d| d.to_string());
                let precision = spec.precision.into();
                let media_idx = if let Some(attachment) = contract.contract_terms().media {
                    Some(self.get_or_insert_media(
                        hex::encode(attachment.digest),
                        attachment.ty.to_string(),
                    )?)
                } else {
                    None
                };
                let initial_supply = contract.total_issued_supply().into();
                let max_supply = contract.max_supply().into();
                let known_circulating_supply = IfaWrapper::with(valid_transfer.contract_data())
                    .total_issued_supply()
                    .into();
                let reject_list_url = contract.reject_list_url().map(|u| u.to_string());
                LocalAssetData {
                    name,
                    precision,
                    ticker: Some(ticker),
                    details,
                    media_idx,
                    initial_supply,
                    max_supply: Some(max_supply),
                    known_circulating_supply: Some(known_circulating_supply),
                    reject_list_url,
                }
            }
        };

        self.add_asset_to_db(
            contract_id.to_string(),
            &asset_schema,
            None,
            timestamp,
            local_asset_data,
        )?;

        Ok(())
    }

    /// Return the consignment file path for a send transfer of an asset.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    pub fn get_send_consignment_path(&self, asset_id: &str, transfer_id: &str) -> PathBuf {
        let transfer_dir = self.get_transfer_dir(transfer_id);
        let asset_transfer_dir = self.get_asset_transfer_dir(transfer_dir, asset_id);
        asset_transfer_dir.join(CONSIGNMENT_FILE)
    }

    /// Post a consignment to the proxy server.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn post_consignment(
        &self,
        proxy_url: &str,
        recipient_id: String,
        consignment_bytes: &[u8],
        txid: String,
        vout: Option<u32>,
    ) -> Result<(), Error> {
        info!(self.logger, "Posting consignment...");
        let consignment_res = self
            .wasm_proxy_client
            .post_consignment(
                proxy_url,
                recipient_id.clone(),
                consignment_bytes,
                txid.clone(),
                vout,
            )
            .await?;
        debug!(
            self.logger,
            "Consignment POST response: {:?}", consignment_res
        );

        if let Some(err) = consignment_res.error {
            if err.code == -101 {
                return Err(Error::RecipientIDAlreadyUsed);
            }
            return Err(Error::InvalidTransportEndpoint {
                details: format!("proxy error: {}", err.message),
            });
        }
        if consignment_res.result.is_none() {
            return Err(Error::InvalidTransportEndpoint {
                details: s!("invalid result"),
            });
        }

        info!(self.logger, "Post consignment completed");
        Ok(())
    }

    /// Get the height at which a transaction was mined.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn get_tx_height(&self, online: Online, txid: String) -> Result<Option<u32>, Error> {
        info!(self.logger, "Getting TX height...");
        self.check_online(online)?;
        let _ = RgbTxid::from_str(&txid).map_err(|_| Error::InvalidTxid)?;
        let height = self.indexer().get_tx_height(&txid).await?;
        info!(self.logger, "Get TX height completed");
        Ok(height)
    }

    /// Accept an RGB transfer by retrieving and validating its consignment from a proxy server.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn accept_transfer(
        &mut self,
        online: Online,
        txid: String,
        vout: u32,
        consignment_endpoint: RgbTransport,
        blinding: u64,
    ) -> Result<(RgbTransfer, Vec<Assignment>), Error> {
        info!(self.logger, "Accepting transfer...");
        self.check_online(online)?;
        let witness_id = RgbTxid::from_str(&txid).map_err(|_| Error::InvalidTxid)?;
        let proxy_url = TransportEndpoint::try_from(consignment_endpoint)?.endpoint;

        let consignment_res = self
            ._get_consignment_async(&proxy_url, txid.clone())
            .await?;
        let consignment_bytes = general_purpose::STANDARD
            .decode(consignment_res.consignment)
            .map_err(InternalError::from)?;
        let consignment = RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;

        let schema_id = consignment.schema_id().to_string();
        let asset_schema: AssetSchema = schema_id.try_into()?;
        self.check_schema_support(&asset_schema)?;
        debug!(
            self.logger,
            "Got consignment for asset with {} schema", asset_schema
        );

        let mut runtime = self.rgb_runtime()?;

        let graph_seal = GraphSeal::with_blinded_vout(vout, blinding);
        runtime.store_secret_seal(graph_seal)?;

        let wasm_resolver =
            crate::utils::WasmResolver::from_consignment(&consignment, self.chain_net());
        let resolver = crate::utils::OffchainResolverWasm {
            witness_id,
            consignment: &consignment,
            fallback: &wasm_resolver,
        };

        debug!(self.logger, "Validating consignment...");
        let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
        let trusted_typesystem = asset_schema.types();
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            trusted_typesystem,
            ..Default::default()
        };
        let valid_consignment = match consignment.clone().validate(&resolver, &validation_config) {
            Ok(consignment) => consignment,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return Err(Error::InvalidConsignment);
            }
            Err(ValidationError::ResolverError(e)) => {
                warn!(self.logger, "Network error during consignment validation");
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
        };
        let validity = valid_consignment.validation_status().validity();
        debug!(self.logger, "Consignment validity: {:?}", validity);

        let valid_contract = valid_consignment.clone().into_valid_contract();
        runtime
            .import_contract(valid_contract, &resolver)
            .expect("failure importing validated contract");

        let received_rgb_assignments =
            self.extract_received_assignments(&consignment, witness_id, Some(vout), None);

        let _status = runtime.accept_transfer(valid_consignment, &resolver)?;

        info!(self.logger, "Accept transfer completed");
        Ok((
            consignment,
            received_rgb_assignments.into_values().collect(),
        ))
    }

    /// Update RGB witnesses.
    ///
    /// Pre-fetches witness data from esplora before calling the sync RGB stock method.
    ///
    /// <div class="warning">This method is meant for special usage and is normally not needed, use
    /// it only if you know what you're doing</div>
    #[cfg(feature = "esplora")]
    pub async fn update_witnesses(
        &mut self,
        online: Online,
        after_height: u32,
        force_witnesses: Vec<RgbTxid>,
    ) -> Result<UpdateRes, Error> {
        info!(self.logger, "Updating witnesses...");
        self.check_online(online)?;

        let mut cache = HashMap::new();
        for witness_id in &force_witnesses {
            let txid_str = witness_id.to_string();
            let txid = Txid::from_str(&txid_str).map_err(|_| Error::InvalidTxid)?;
            if let Some((tx, block_height, block_time)) =
                self.indexer().get_tx_with_status(&txid).await?
            {
                let witness_ord = match block_height.and_then(|h| block_time.map(|t| (h, t))) {
                    Some((h, t)) => {
                        if let Some(height) = NonZeroU32::new(h) {
                            if let Some(pos) = WitnessPos::bitcoin(height, t as i64) {
                                WitnessOrd::Mined(pos)
                            } else {
                                WitnessOrd::Tentative
                            }
                        } else {
                            WitnessOrd::Tentative
                        }
                    }
                    None => WitnessOrd::Tentative,
                };
                cache.insert(*witness_id, WitnessStatus::Resolved(tx, witness_ord));
            }
        }

        let resolver = crate::utils::PreFetchResolver::new(cache, self.chain_net());
        let update_res =
            self.rgb_runtime()?
                .update_witnesses(&resolver, after_height, force_witnesses)?;

        info!(self.logger, "Update witnesses completed");
        Ok(update_res)
    }
}

/// Check whether the provided URL points to a valid proxy.
///
/// An error is raised if the provided proxy URL is invalid or if the service is running an
/// unsupported protocol version.
#[cfg(feature = "esplora")]
pub async fn check_proxy_url(proxy_url: &str) -> Result<(), Error> {
    crate::utils::check_proxy_async(proxy_url).await
}

/// Validate a consignment using the witness bundled in the consignment (offchain).
///
/// This works before the witness transaction is broadcast. The consignment bytes are
/// the raw strict-encoded consignment (not base64). The `txid` is the witness
/// transaction ID. The fallback resolver uses witness data from the consignment itself.
///
/// Returns a `ValidateConsignmentResult` with validity status, warnings, and error details.
#[cfg(feature = "esplora")]
pub fn validate_consignment_offchain(
    consignment_bytes: &[u8],
    txid: &str,
    bitcoin_network: BitcoinNetwork,
) -> Result<ValidateConsignmentResult, Error> {
    let consignment = RgbTransfer::load(consignment_bytes).map_err(|e| Error::Internal {
        details: format!("Failed to load consignment: {e}"),
    })?;

    let witness_id = RgbTxid::from_str(txid).map_err(|_| Error::InvalidTxid)?;
    let chain_net: ChainNet = bitcoin_network.into();
    let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
    let trusted_typesystem = asset_schema.types();

    let wasm_resolver = crate::utils::WasmResolver::from_consignment(&consignment, chain_net);
    let resolver = crate::utils::OffchainResolverWasm {
        witness_id,
        consignment: &consignment,
        fallback: &wasm_resolver,
    };

    let validation_config = ValidationConfig {
        chain_net,
        trusted_typesystem,
        ..Default::default()
    };

    match consignment.clone().validate(&resolver, &validation_config) {
        Ok(valid_consignment) => {
            let status = valid_consignment.validation_status();
            Ok(ValidateConsignmentResult {
                valid: true,
                warnings: Some(
                    status
                        .warnings
                        .iter()
                        .map(|w| w.to_string())
                        .collect::<Vec<_>>(),
                ),
                error: None,
                details: None,
            })
        }
        Err(ValidationError::InvalidConsignment(failure)) => Ok(ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("invalid".to_string()),
            details: Some(failure.to_string()),
        }),
        Err(ValidationError::ResolverError(e)) => Ok(ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("resolver".to_string()),
            details: Some(e.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_consignment_offchain_invalid_bytes() {
        let result = validate_consignment_offchain(
            b"not a valid consignment",
            "0000000000000000000000000000000000000000000000000000000000000000",
            BitcoinNetwork::Regtest,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::Internal { details: ref d } if d.contains("Failed to load consignment")),
            "expected Internal error with load failure, got: {err:?}"
        );
    }

    #[test]
    fn validate_consignment_offchain_invalid_txid() {
        let result = validate_consignment_offchain(
            b"not a valid consignment",
            "not-a-txid",
            BitcoinNetwork::Regtest,
        );
        // Will fail on consignment load before txid parsing
        assert!(result.is_err());
    }

    #[test]
    fn validate_consignment_result_serde_roundtrip() {
        let result = ValidateConsignmentResult {
            valid: true,
            warnings: Some(vec!["warn1".to_string()]),
            error: None,
            details: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ValidateConsignmentResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.valid);
        assert_eq!(deserialized.warnings.unwrap(), vec!["warn1".to_string()]);
        assert!(deserialized.error.is_none());

        let result_invalid = ValidateConsignmentResult {
            valid: false,
            warnings: None,
            error: Some("invalid".to_string()),
            details: Some("schema mismatch".to_string()),
        };
        let json = serde_json::to_string(&result_invalid).unwrap();
        let deserialized: ValidateConsignmentResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.valid);
        assert_eq!(deserialized.error.unwrap(), "invalid");
    }

    #[test]
    fn indexer_protocol_display() {
        assert_eq!(IndexerProtocol::Esplora.to_string(), "Esplora");
    }
}
