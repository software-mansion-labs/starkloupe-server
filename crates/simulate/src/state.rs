use anyhow::{Context, Result};
use blockifier::blockifier::block::BlockInfo;
use blockifier::execution::contract_class::{
    ContractClass as ContractClassBlockifier, ContractClassV0, ContractClassV1,
};
use blockifier::state::cached_state::StorageEntry;
use blockifier::state::errors::StateError::{self, StateReadError, UndeclaredClassHash};
use blockifier::state::state_api::{StateReader, StateResult};
use cairo_lang_starknet_classes::casm_contract_class::CasmContractClass;
use cairo_lang_utils::bigint::BigUintAsHex;
use cheatnet::state::BlockInfoReader;
use conversions::{FromConv, IntoConv};
use flate2::read::GzDecoder;
use num_bigint::BigUint;
use runtime::starknet::context::SerializableGasPrices;
use starknet::core::types::{
    BlockId, ContractClass as ContractClassStarknet, ContractStorageDiffItem, FieldElement,
    MaybePendingBlockWithTxHashes, StarknetError, TransactionTrace,
};
use starknet::providers::jsonrpc::HttpTransport;
use starknet::providers::{JsonRpcClient, Provider, ProviderError};
use starknet_api::block::{BlockNumber, BlockTimestamp};
use starknet_api::core::{ClassHash, CompiledClassHash, ContractAddress, Nonce};
use starknet_api::deprecated_contract_class::{
    ContractClass as DeprecatedContractClass, EntryPoint, EntryPointType,
};
use starknet_api::hash::StarkFelt;
use starknet_api::state::StorageKey;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use universal_sierra_compiler_api::{compile_sierra, SierraType};
use url::Url;

#[derive(Debug, Default)]
pub struct InMemoryForkCache {
    pub storage_view: HashMap<StorageEntry, StarkFelt>,
    pub address_to_nonce: HashMap<ContractAddress, Nonce>,
    pub address_to_class_hash: HashMap<ContractAddress, ClassHash>,
    pub class_hash_to_compiled_class: HashMap<ClassHash, ContractClassBlockifier>,
    pub class_hash_to_class: HashMap<ClassHash, ContractClassStarknet>,
    pub class_hash_to_compiled_class_hash: HashMap<ClassHash, CompiledClassHash>,
    pub block_info: Option<BlockInfo>,
}

impl InMemoryForkCache {
    pub fn cache_storage_at(
        &mut self,
        contract_address: ContractAddress,
        key: StorageKey,
        value: StarkFelt,
    ) {
        self.storage_view.insert((contract_address, key), value);
    }

    pub fn cache_nonce_at(&mut self, contract_address: ContractAddress, nonce: Nonce) {
        self.address_to_nonce.insert(contract_address, nonce);
    }

    pub fn cache_class_hash_at(
        &mut self,
        contract_address: ContractAddress,
        class_hash: ClassHash,
    ) {
        self.address_to_class_hash
            .insert(contract_address, class_hash);
    }

    pub fn cache_compiled_class_hash(
        &mut self,
        class_hash: ClassHash,
        compiled_class_hash: CompiledClassHash,
    ) {
        self.class_hash_to_compiled_class_hash
            .insert(class_hash, compiled_class_hash);
    }

    pub fn cache_block_info(&mut self, block_info: BlockInfo) {
        self.block_info = Some(block_info);
    }

    pub fn get_block_info(&self) -> Option<BlockInfo> {
        self.block_info.clone()
    }

    pub fn cache_compiled_contract_class(
        &mut self,
        class_hash: ClassHash,
        contract_class: ContractClassBlockifier,
    ) {
        self.class_hash_to_compiled_class
            .insert(class_hash, contract_class);
    }

    pub fn cache_contract_class(
        &mut self,
        class_hash: ClassHash,
        contract_class: ContractClassStarknet,
    ) {
        self.class_hash_to_class.insert(class_hash, contract_class);
    }

    pub fn get_contract_class(&self, class_hash: ClassHash) -> StateResult<ContractClassStarknet> {
        let contract_class = self.class_hash_to_class.get(&class_hash).cloned();
        match contract_class {
            Some(contract_class) => Ok(contract_class),
            _ => Err(StateError::UndeclaredClassHash(class_hash)),
        }
    }
}

impl StateReader for InMemoryForkCache {
    fn get_storage_at(
        &self,
        contract_address: ContractAddress,
        key: StorageKey,
    ) -> StateResult<StarkFelt> {
        self.storage_view
            .get(&(contract_address, key))
            .copied()
            .ok_or(StateError::StateReadError(format!(
                "Unable to get storage at address: {contract_address:?} and key: {key:?} form DictStateReader"
            )))
    }

    fn get_nonce_at(&self, contract_address: ContractAddress) -> StateResult<Nonce> {
        self.address_to_nonce
            .get(&contract_address)
            .copied()
            .ok_or(StateError::StateReadError(format!(
                "Unable to get nonce at {contract_address:?} from DictStateReader"
            )))
    }

    fn get_class_hash_at(&self, contract_address: ContractAddress) -> StateResult<ClassHash> {
        self.address_to_class_hash
            .get(&contract_address)
            .copied()
            .ok_or(StateError::UnavailableContractAddress(contract_address))
    }

    fn get_compiled_contract_class(
        &self,
        class_hash: ClassHash,
    ) -> StateResult<ContractClassBlockifier> {
        let contract_class = self.class_hash_to_compiled_class.get(&class_hash).cloned();
        match contract_class {
            Some(contract_class) => Ok(contract_class),
            _ => Err(StateError::UndeclaredClassHash(class_hash)),
        }
    }

    fn get_compiled_class_hash(&self, class_hash: ClassHash) -> StateResult<CompiledClassHash> {
        let compiled_class_hash = self
            .class_hash_to_compiled_class_hash
            .get(&class_hash)
            .copied()
            .unwrap_or_default();
        Ok(compiled_class_hash)
    }
}

#[derive(Debug)]
pub struct ForkStateReader {
    client: JsonRpcClient<HttpTransport>,
    block_number: u64,
    adjusted_block_number: u64,
    pub in_memory_fork_cache: RefCell<InMemoryForkCache>, // Wrap in RefCell
}

impl ForkStateReader {
    pub fn new(url: Url, block_number: u64, transaction_index: usize) -> Result<Self> {
        let block_id = BlockId::Number(block_number);
        let adjusted_block_number = block_number - 1;

        let mut fork_state_reader = ForkStateReader {
            client: JsonRpcClient::new(HttpTransport::new(url.clone())),
            block_number,
            adjusted_block_number,
            in_memory_fork_cache: RefCell::new(InMemoryForkCache::default()), // Wrap in RefCell
        };

        let tx_number_in_block = fork_state_reader
            .get_block_transaction_count(block_id)
            .context("Unable to get block transactions count from node provider")?;
        if tx_number_in_block > 1 {
            //Get over all transaction till transaction_index and store new storage values in
            //storage_diff hash map
            fork_state_reader
                .prepare_storage_view(block_id, transaction_index)
                .context("Unable to get trace block transactions from node provider")?;
        }
        // Return the initialized and state updated ForkStateReader
        Ok(fork_state_reader)
    }

    fn block_id(&self) -> BlockId {
        BlockId::Number(self.block_number)
    }

    fn adjusted_block_id(&self) -> BlockId {
        BlockId::Number(self.adjusted_block_number)
    }

    pub fn get_block_transaction_count(&self, block_id: BlockId) -> Result<u64, StateError> {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.client.get_block_transaction_count(block_id))
        })
        .map_err(|err| {
            StateError::StateReadError(format!(
                "Unable to get block transactions count from fork ({err})"
            ))
        })?;

        Ok(result)
    }

    pub fn prepare_storage_view(
        &mut self,
        block_id: BlockId,
        transaction_index: usize,
    ) -> Result<(), StateError> {
        let results = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.client.trace_block_transactions(block_id))
        })
        .map_err(|err| {
            StateError::StateReadError(format!(
                "Unable to get trace block transactions from fork ({err})"
            ))
        })?;

        for (index, result) in results.into_iter().enumerate() {
            if index == transaction_index {
                break;
            }
            match &result.trace_root {
                TransactionTrace::Invoke(invoke_trace) => {
                    if let Some(state_diff) = &invoke_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        self.collect_storage_diffs(contract_storage_diff);
                    }
                }
                TransactionTrace::Declare(declare_trace) => {
                    if let Some(state_diff) = &declare_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        self.collect_storage_diffs(contract_storage_diff);
                    }
                }
                TransactionTrace::DeployAccount(deploy_trace) => {
                    if let Some(state_diff) = &deploy_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        self.collect_storage_diffs(contract_storage_diff);
                    }
                }
                TransactionTrace::L1Handler(l1handler_trace) => {
                    if let Some(state_diff) = &l1handler_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        self.collect_storage_diffs(contract_storage_diff);
                    }
                }
            }
        }

        Ok(())
    }

    fn collect_storage_diffs(&mut self, storage_diffs: &[ContractStorageDiffItem]) {
        let mut cache = self.in_memory_fork_cache.borrow_mut();
        for storage_diff in storage_diffs.iter() {
            let contract_address: ContractAddress =
                ContractAddress::try_from(StarkFelt::from(storage_diff.address)).unwrap();
            for storage_entry in storage_diff.storage_entries.iter() {
                let key = StorageKey::try_from(StarkFelt::from(storage_entry.key)).unwrap();
                let value: StarkFelt = storage_entry.value.into();
                cache.cache_storage_at(contract_address, key, value);
            }
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn other_provider_error<T>(boxed: impl ToString) -> Result<T, StateError> {
    let err_str = boxed.to_string();

    Err(StateError::StateReadError(
        if err_str.contains("error sending request for url") {
            "Unable to reach the node. Check your internet connection and node url".to_string()
        } else {
            format!("JsonRpc provider error: {err_str}")
        },
    ))
}

impl BlockInfoReader for ForkStateReader {
    // TODO: check usage
    fn get_block_info(&mut self) -> StateResult<BlockInfo> {
        if let Some(block_info) = self.in_memory_fork_cache.borrow().get_block_info() {
            return Ok(block_info);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.client.get_block_with_tx_hashes(self.block_id()))
        }) {
            Ok(MaybePendingBlockWithTxHashes::Block(block)) => {
                let block_info = BlockInfo {
                    block_number: BlockNumber(block.block_number),
                    sequencer_address: block.sequencer_address.into_(),
                    block_timestamp: BlockTimestamp(block.timestamp),
                    gas_prices: SerializableGasPrices::default().into(),
                    use_kzg_da: true,
                };

                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_block_info(block_info.clone());

                Ok(block_info)
            }
            Ok(MaybePendingBlockWithTxHashes::PendingBlock(_)) => {
                unreachable!("Pending block is not be allowed at the configuration level")
            }
            Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
            Err(err) => Err(StateReadError(format!(
                "Unable to get block with tx hashes from fork ({err})"
            ))),
        }
    }
}

impl StateReader for ForkStateReader {
    fn get_storage_at(
        &self,
        contract_address: ContractAddress,
        key: StorageKey,
    ) -> StateResult<StarkFelt> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_storage_at(contract_address, key)
        {
            return Ok(cache_hit);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.client.get_storage_at(
                FieldElement::from_(contract_address),
                FieldElement::from_(*key.0.key()),
                self.adjusted_block_id(),
            ))
        }) {
            Ok(value) => {
                let value_sf = value.into_();
                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_storage_at(contract_address, key, value_sf);
                Ok(value_sf)
            }
            Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
            Err(ProviderError::StarknetError(StarknetError::ContractNotFound)) => Ok(Default::default()),
            Err(x) => Err(StateReadError(format!(
                "Unable to get storage at address: {contract_address:?} and key: {key:?} from fork ({x})"
            ))),
        }
    }

    fn get_nonce_at(&self, contract_address: ContractAddress) -> StateResult<Nonce> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_nonce_at(contract_address)
        {
            return Ok(cache_hit);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.client.get_nonce(
                self.adjusted_block_id(),
                FieldElement::from_(contract_address),
            ))
        }) {
            Ok(nonce) => {
                let nonce = nonce.into_();
                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_nonce_at(contract_address, nonce);
                Ok(nonce)
            }
            Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
            Err(ProviderError::StarknetError(StarknetError::ContractNotFound)) => {
                Ok(Default::default())
            }
            Err(x) => Err(StateReadError(format!(
                "Unable to get nonce at {contract_address:?} from fork ({x})"
            ))),
        }
    }

    // TODO: check the case where the contract was upgraded and the class hash changed
    fn get_class_hash_at(&self, contract_address: ContractAddress) -> StateResult<ClassHash> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_class_hash_at(contract_address)
        {
            return Ok(cache_hit);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.client.get_class_hash_at(
                self.adjusted_block_id(),
                FieldElement::from_(contract_address),
            ))
        }) {
            Ok(class_hash) => {
                let class_hash = class_hash.into_();
                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_class_hash_at(contract_address, class_hash);
                Ok(class_hash)
            }
            Err(ProviderError::StarknetError(StarknetError::ContractNotFound)) => {
                Ok(Default::default())
            }
            Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
            Err(x) => Err(StateReadError(format!(
                "Unable to get class hash at {contract_address:?} from fork ({x})"
            ))),
        }
    }

    fn get_compiled_contract_class(
        &self,
        class_hash: ClassHash,
    ) -> StateResult<ContractClassBlockifier> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_compiled_contract_class(class_hash)
        {
            return Ok(cache_hit);
        }

        let mut in_memory_fork_cache = self.in_memory_fork_cache.borrow_mut();

        let contract_class = {
            if let Ok(contract_class) = in_memory_fork_cache.get_contract_class(class_hash) {
                Ok(contract_class)
            } else {
                match tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(
                        self.client
                            .get_class(self.adjusted_block_id(), FieldElement::from_(class_hash)),
                    )
                }) {
                    Ok(contract_class) => {
                        in_memory_fork_cache
                            .cache_contract_class(class_hash, contract_class.clone());
                        Ok(contract_class)
                    }
                    Err(ProviderError::StarknetError(StarknetError::ClassHashNotFound)) => {
                        Err(UndeclaredClassHash(class_hash))
                    }
                    Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
                    Err(x) => Err(StateReadError(format!(
                        "Unable to get compiled class at {class_hash} from fork ({x})"
                    ))),
                }
            }
        };

        match contract_class? {
            ContractClassStarknet::Sierra(flattened_class) => {
                let converted_sierra_program: Vec<BigUintAsHex> = flattened_class
                    .sierra_program
                    .iter()
                    .map(|field_element| BigUintAsHex {
                        value: BigUint::from_bytes_be(&field_element.to_bytes_be()),
                    })
                    .collect();

                let sierra_contract_class = serde_json::json!({
                    "sierra_program": converted_sierra_program,
                    "contract_class_version": "",
                    "entry_points_by_type": flattened_class.entry_points_by_type
                });

                match compile_sierra(&sierra_contract_class, None, &SierraType::Contract) {
                    Ok(casm_contract_class_raw) => {
                        let casm_contract_class: CasmContractClass =
                            serde_json::from_str(&casm_contract_class_raw)
                                .expect("Unable to deserialize CasmContractClass");

                        let compiled_contract_class = ContractClassBlockifier::V1(
                            ContractClassV1::try_from(casm_contract_class)
                                .expect("Unable to create ContractClassV1 from CasmContractClass"),
                        );

                        in_memory_fork_cache.cache_compiled_contract_class(
                            class_hash,
                            compiled_contract_class.clone(),
                        );

                        Ok(compiled_contract_class)
                    }
                    Err(err) => Err(StateReadError(err.to_string())),
                }
            }
            ContractClassStarknet::Legacy(legacy_class) => {
                let converted_entry_points: HashMap<EntryPointType, Vec<EntryPoint>> =
                    serde_json::from_str(
                        &serde_json::to_string(&legacy_class.entry_points_by_type).unwrap(),
                    )
                    .unwrap();

                let mut decoder = GzDecoder::new(&legacy_class.program[..]);
                let mut converted_program = String::new();
                decoder.read_to_string(&mut converted_program).unwrap();

                let compiled_contract_class = ContractClassBlockifier::V0(
                    ContractClassV0::try_from(DeprecatedContractClass {
                        abi: None,
                        program: serde_json::from_str(&converted_program).unwrap(),
                        entry_points_by_type: converted_entry_points,
                    })
                    .unwrap(),
                );

                in_memory_fork_cache
                    .cache_compiled_contract_class(class_hash, compiled_contract_class.clone());

                Ok(compiled_contract_class)
            }
        }
    }

    fn get_compiled_class_hash(&self, _class_hash: ClassHash) -> StateResult<CompiledClassHash> {
        Err(StateReadError(
            "Unable to get compiled class hash from the fork".to_string(),
        ))
    }
}
