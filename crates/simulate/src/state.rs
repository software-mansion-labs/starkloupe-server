use blockifier::execution::contract_class::{
    CompiledClassV0 as ContractClassV0, CompiledClassV1 as ContractClassV1,
    RunnableCompiledClass as ContractClassBlockifier,
};
use blockifier::state::cached_state::StorageEntry;
use blockifier::state::errors::StateError::{self, StateReadError};
use blockifier::state::state_api::{StateReader, StateResult};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_lang_utils::bigint::BigUintAsHex;
use cheatnet::state::BlockInfoReader;
use conversions::FromConv;
use flate2::read::GzDecoder;
use num_bigint::BigUint;
use runtime::starknet::context::SerializableGasPrices;
use sqlx::Pool;
use sqlx::Postgres;
use starknet::core::types::{
    BlockId, BlockTag, ConfirmedBlockId, ContractClass as ContractClassStarknet,
    ContractStorageDiffItem, DeclaredClassItem, DeployedContractItem, EntryPointsByType, Felt,
    FlattenedSierraClass, MaybePreConfirmedBlockWithTxHashes, SierraEntryPoint, StarknetError,
    TransactionTrace,
};
use starknet::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider, ProviderError, Url,
};
use starknet_api::block::BlockInfo;
use starknet_api::block::{BlockNumber, BlockTimestamp};
use starknet_api::contract_class::{EntryPointType, SierraVersion};
use starknet_api::core::{ClassHash, CompiledClassHash, ContractAddress, Nonce};
use starknet_api::deprecated_contract_class::{
    ContractClass as DeprecatedContractClass, EntryPointV0,
};
use starknet_api::state::StorageKey;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use tracing::error;
use universal_sierra_compiler_api::{compile_sierra, SierraType};
use verification::s3::fetch_verified_class_hash_with_contract_class_data;
use walnut_shared::create_rpc_client_from_url;

#[derive(Debug, Default)]
pub struct InMemoryForkCache {
    pub storage_view: HashMap<StorageEntry, Felt>,
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
        value: Felt,
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

    pub fn get_storage_view(&self) -> HashMap<StorageEntry, Felt> {
        self.storage_view.clone()
    }
}

impl StateReader for InMemoryForkCache {
    fn get_storage_at(
        &self,
        contract_address: ContractAddress,
        key: StorageKey,
    ) -> StateResult<Felt> {
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

    fn get_compiled_class(&self, class_hash: ClassHash) -> StateResult<ContractClassBlockifier> {
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
    only_non_inlined_class: bool,
    pub in_memory_fork_cache: RefCell<InMemoryForkCache>, // Wrap in RefCell
    db_pool: Pool<Postgres>,
    s3_client: aws_sdk_s3::Client,
}

impl ForkStateReader {
    pub fn new(
        url: Url,
        block_number: u64,
        transaction_index: usize,
        tx_number_in_block: usize,
        only_non_inlined_class: bool,
        db_pool: &Pool<Postgres>,
        s3_client: &aws_sdk_s3::Client,
    ) -> anyhow::Result<Self> {
        let block_id = BlockId::Number(block_number);
        let adjusted_block_number = block_number - 1;
        let client = create_rpc_client_from_url(url.clone());
        let mut fork_state_reader = ForkStateReader {
            client,
            block_number,
            adjusted_block_number,
            only_non_inlined_class,
            in_memory_fork_cache: RefCell::new(InMemoryForkCache::default()), // Wrap in RefCell
            db_pool: db_pool.clone(),
            s3_client: s3_client.clone(),
        };

        if tx_number_in_block > 1 {
            fork_state_reader
                .prepare_storage_view(block_id, transaction_index)
                .map_err(|_| {
                    anyhow::anyhow!("Unable to get trace block transactions from node provider",)
                })?;
        }
        Ok(fork_state_reader)
    }

    fn block_id(&self) -> BlockId {
        BlockId::Number(self.block_number)
    }

    fn adjusted_block_id(&self) -> BlockId {
        BlockId::Number(self.adjusted_block_number)
    }

    pub fn block_number(&self) -> u64 {
        self.block_number
    }

    pub fn adjusted_block_number(&self) -> u64 {
        self.adjusted_block_number
    }

    pub fn prepare_storage_view(
        &mut self,
        block_id: BlockId,
        transaction_index: usize,
    ) -> Result<(), StateError> {
        let results = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.client.trace_block_transactions(
                match block_id {
                    BlockId::Hash(hash) => ConfirmedBlockId::Hash(hash),
                    BlockId::Number(number) => ConfirmedBlockId::Number(number),
                    BlockId::Tag(tag) => match tag {
                        BlockTag::Latest => ConfirmedBlockId::Latest,
                        BlockTag::PreConfirmed => ConfirmedBlockId::Latest, // Pre-confirmed not supported, use Latest
                        _ => ConfirmedBlockId::Latest, // Pre-confirmed not supported, use Latest
                    },
                },
            ))
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
                        let declared = &state_diff.declared_classes;
                        let deployed = &state_diff.deployed_contracts;
                        self.collect_storage_diffs(&contract_storage_diff.to_vec());
                        self.collect_deploy_contract(&deployed.to_vec());
                        self.collect_declared_classes(&declared.to_vec());
                    }
                }
                TransactionTrace::Declare(declare_trace) => {
                    if let Some(state_diff) = &declare_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        let declared = &state_diff.declared_classes;
                        let deployed = &state_diff.deployed_contracts;
                        self.collect_storage_diffs(&contract_storage_diff.to_vec());
                        self.collect_deploy_contract(&deployed.to_vec());
                        self.collect_declared_classes(&declared.to_vec());
                    }
                }
                TransactionTrace::DeployAccount(deploy_trace) => {
                    if let Some(state_diff) = &deploy_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        let declared = &state_diff.declared_classes;
                        let deployed = &state_diff.deployed_contracts;
                        self.collect_storage_diffs(&contract_storage_diff.to_vec());
                        self.collect_deploy_contract(&deployed.to_vec());
                        self.collect_declared_classes(&declared.to_vec());
                    }
                }
                TransactionTrace::L1Handler(l1handler_trace) => {
                    if let Some(state_diff) = &l1handler_trace.state_diff {
                        let contract_storage_diff = &state_diff.storage_diffs;
                        let declared = &state_diff.declared_classes;
                        let deployed = &state_diff.deployed_contracts;
                        self.collect_storage_diffs(&contract_storage_diff.to_vec());
                        self.collect_deploy_contract(&deployed.to_vec());
                        self.collect_declared_classes(&declared.to_vec());
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
                ContractAddress::try_from(Felt::from(storage_diff.address)).unwrap();
            for storage_entry in storage_diff.storage_entries.iter() {
                let key = StorageKey::try_from(Felt::from(storage_entry.key)).unwrap();
                let value: Felt = storage_entry.value;
                cache.cache_storage_at(contract_address, key, value);
            }
        }
    }

    fn collect_deploy_contract(&mut self, deploy_contract: &[DeployedContractItem]) {
        let mut cache = self.in_memory_fork_cache.borrow_mut();
        for contract in deploy_contract.iter() {
            let contract_address: ContractAddress =
                ContractAddress::try_from(Felt::from(contract.address)).unwrap();
            let class_hash = ClassHash(contract.class_hash.into());
            cache.cache_class_hash_at(contract_address, class_hash);
        }
    }

    fn collect_declared_classes(&mut self, declare_class: &[DeclaredClassItem]) {
        for class in declare_class.iter() {
            let class_hash = ClassHash(class.class_hash.into());
            let compiled_class_hash = CompiledClassHash(class.compiled_class_hash.into());
            {
                let mut in_memory_fork_cache = self.in_memory_fork_cache.borrow_mut();
                in_memory_fork_cache.cache_compiled_class_hash(class_hash, compiled_class_hash);
            }
            if let Err(err) = self.fetch_and_compile_contract_class(class_hash, self.block_id()) {
                error!(
                    "Error in state when try to fetch and compile contract class: {:?}",
                    err
                );
                continue;
            }
        }
    }

    fn fetch_and_compile_verified_contract_class(
        &self,
        class_hash: ClassHash,
        contract_class: ContractClass,
    ) -> StateResult<()> {
        let mut in_memory_fork_cache = self.in_memory_fork_cache.borrow_mut();

        let converted_sierra_program: Vec<Felt> = contract_class
            .sierra_program
            .iter()
            .map(|element| Felt::from(element.value.clone()))
            .collect();

        let sierra_program_version =
            SierraVersion::extract_from_program(&converted_sierra_program)?;
        let entry_points = &contract_class.entry_points_by_type;
        let entry_points_converted = EntryPointsByType {
            constructor: entry_points
                .constructor
                .iter()
                .map(|entry| SierraEntryPoint {
                    selector: Felt::from_dec_str(&entry.selector.to_str_radix(10))
                        .expect("Invalid Felt conversion"),
                    function_idx: entry.function_idx as u64,
                })
                .collect(),
            external: entry_points
                .external
                .iter()
                .map(|entry| SierraEntryPoint {
                    selector: Felt::from_dec_str(&entry.selector.to_str_radix(10))
                        .expect("Invalid Felt conversion"),
                    function_idx: entry.function_idx as u64,
                })
                .collect(),
            l1_handler: entry_points
                .l1_handler
                .iter()
                .map(|entry| SierraEntryPoint {
                    selector: Felt::from_dec_str(&entry.selector.to_str_radix(10))
                        .expect("Invalid Felt conversion"),
                    function_idx: entry.function_idx as u64,
                })
                .collect(),
        };

        let contract_class_starknet = ContractClassStarknet::Sierra(FlattenedSierraClass {
            sierra_program: converted_sierra_program,
            contract_class_version: contract_class.contract_class_version.clone(),
            entry_points_by_type: entry_points_converted,
            abi: contract_class.abi.as_ref().unwrap().json(),
        });

        in_memory_fork_cache.cache_contract_class(class_hash, contract_class_starknet);

        let sierra_contract_class = serde_json::json!({
            "sierra_program": contract_class.sierra_program,
            "contract_class_version": contract_class.contract_class_version,
            "entry_points_by_type": contract_class.entry_points_by_type,
            "abi": contract_class.abi
        });

        let casm_contract_class_raw =
            compile_sierra::<String>(&sierra_contract_class, &SierraType::Contract)
                .map_err(|err| StateError::StateReadError(err.to_string()))?;

        let compiled_contract_class = ContractClassBlockifier::V1(
            ContractClassV1::try_from_json_string(&casm_contract_class_raw, sierra_program_version)
                .map_err(|e| {
                    StateError::StateReadError(format!(
                        "Unable to create ContractClassV1 from CasmContractClass: {}",
                        e
                    ))
                })?,
        );

        in_memory_fork_cache.cache_compiled_contract_class(class_hash, compiled_contract_class);

        Ok(())
    }

    fn fetch_and_compile_contract_class(
        &self,
        class_hash: ClassHash,
        block_id: BlockId,
    ) -> StateResult<()> {
        let mut in_memory_fork_cache = self.in_memory_fork_cache.borrow_mut();

        let contract_class = match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.client.get_class(block_id, Felt::from_(class_hash)))
        }) {
            Ok(contract_class_json) => {
                let contract_class: ContractClassStarknet = serde_json::from_value(
                        serde_json::to_value(&contract_class_json)
                            .map_err(|e| StateError::StateReadError(format!(
                                "Failed to serialize class from blockchain to JSON value: {}", e
                            )))?,
                    )
                    .map_err(|e| StateError::StateReadError(format!(
                        "Failed to deserialize class from JSON value back to ContractClassStarknet: {}", e
                    )))?;

                in_memory_fork_cache.cache_contract_class(class_hash, contract_class.clone());

                Ok(contract_class)
            }
            Err(ProviderError::StarknetError(StarknetError::ClassHashNotFound)) => {
                Err(StateError::UndeclaredClassHash(class_hash))
            }
            Err(ProviderError::Other(boxed)) => other_provider_error(boxed),
            Err(x) => Err(StateError::StateReadError(format!(
                "Unable to get compiled class at {class_hash} from fork ({x})"
            ))),
        }?;

        let compiled_contract_class = match contract_class {
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

                let casm_contract_class_raw =
                    compile_sierra::<String>(&sierra_contract_class, &SierraType::Contract)
                        .map_err(|err| StateError::StateReadError(err.to_string()))?;

                let sierra_program_version =
                    SierraVersion::extract_from_program(&flattened_class.sierra_program)?;
                ContractClassBlockifier::V1(
                    ContractClassV1::try_from_json_string(
                        &casm_contract_class_raw,
                        sierra_program_version,
                    )
                    .map_err(|e| {
                        StateError::StateReadError(format!(
                            "Unable to create ContractClassV1 from CasmContractClass: {}",
                            e
                        ))
                    })?,
                )
            }
            ContractClassStarknet::Legacy(legacy_class) => {
                let converted_entry_points: HashMap<EntryPointType, Vec<EntryPointV0>> =
                    serde_json::from_str(
                        &serde_json::to_string(&legacy_class.entry_points_by_type).map_err(
                            |e| {
                                StateError::StateReadError(format!(
                                    "Failed to serialize entry_points_by_type: {}",
                                    e
                                ))
                            },
                        )?,
                    )
                    .map_err(|e| {
                        StateError::StateReadError(format!(
                            "Failed to deserialize entry_points_by_type: {}",
                            e
                        ))
                    })?;

                let mut decoder = GzDecoder::new(&legacy_class.program[..]);
                let mut converted_program = String::new();
                decoder
                    .read_to_string(&mut converted_program)
                    .map_err(|e| {
                        StateError::StateReadError(format!("Failed to decompress program: {}", e))
                    })?;

                let deprecated_contract_class = DeprecatedContractClass {
                    abi: None,
                    program: serde_json::from_str(&converted_program).map_err(|e| {
                        StateError::StateReadError(format!("Failed to parse program JSON: {}", e))
                    })?,
                    entry_points_by_type: converted_entry_points,
                };

                ContractClassBlockifier::V0(
                    ContractClassV0::try_from(deprecated_contract_class).map_err(|e| {
                        StateError::StateReadError(format!(
                            "Failed to create ContractClassV0: {}",
                            e
                        ))
                    })?,
                )
            }
        };

        in_memory_fork_cache.cache_compiled_contract_class(class_hash, compiled_contract_class);

        Ok(())
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
    fn get_block_info(&mut self) -> Result<BlockInfo, StateError> {
        if let Some(block_info) = self.in_memory_fork_cache.borrow().get_block_info() {
            return Ok(block_info);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.client.get_block_with_tx_hashes(self.block_id()))
        }) {
            Ok(MaybePreConfirmedBlockWithTxHashes::Block(block)) => {
                let block_info = BlockInfo {
                    block_number: BlockNumber(block.block_number),
                    sequencer_address: ContractAddress::try_from(block.sequencer_address)
                        .unwrap_or_default(),
                    block_timestamp: BlockTimestamp(block.timestamp),
                    gas_prices: SerializableGasPrices::default().into(),
                    use_kzg_da: true,
                };

                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_block_info(block_info.clone());

                Ok(block_info)
            }
            Ok(MaybePreConfirmedBlockWithTxHashes::PreConfirmedBlock(_)) => {
                unreachable!("Pre-confirmed block is not be allowed at the configuration level")
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
    ) -> StateResult<Felt> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_storage_at(contract_address, key)
        {
            return Ok(cache_hit);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.client.get_storage_at(
                Felt::from_(contract_address),
                Felt::from_(*key.0.key()),
                self.adjusted_block_id(),
            ))
        }) {
            Ok(value) => {
                self.in_memory_fork_cache
                    .borrow_mut()
                    .cache_storage_at(contract_address, key, value);
                Ok(value)
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
            tokio::runtime::Handle::current().block_on(
                self.client
                    .get_nonce(self.adjusted_block_id(), Felt::from_(contract_address)),
            )
        }) {
            Ok(nonce) => {
                let nonce = Nonce(nonce);
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
            tokio::runtime::Handle::current().block_on(
                self.client
                    .get_class_hash_at(self.adjusted_block_id(), Felt::from_(contract_address)),
            )
        }) {
            Ok(class_hash) => {
                let class_hash = ClassHash(class_hash);
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

    fn get_compiled_class(&self, class_hash: ClassHash) -> StateResult<ContractClassBlockifier> {
        if let Ok(cache_hit) = self
            .in_memory_fork_cache
            .borrow()
            .get_compiled_class(class_hash)
        {
            return Ok(cache_hit);
        }

        match tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                fetch_verified_class_hash_with_contract_class_data(
                    &self.db_pool,
                    &self.s3_client,
                    &class_hash.to_fixed_hex_string(),
                ),
            )
        }) {
            Ok((class_hash, Some(contract_class))) => {
                let class_hash_felt = Felt::from_hex(&class_hash).unwrap();

                if !self.only_non_inlined_class {
                    self.fetch_and_compile_verified_contract_class(
                        ClassHash(class_hash_felt),
                        contract_class,
                    )?;
                } else {
                    self.fetch_and_compile_contract_class(
                        ClassHash(class_hash_felt),
                        self.adjusted_block_id(),
                    )?;
                }
                self.in_memory_fork_cache
                    .borrow()
                    .get_compiled_class(ClassHash(class_hash_felt))
                    .map_err(|_| {
                        StateError::StateReadError(format!(
                            "Failed to retrieve compiled contract class for class_hash: {}",
                            class_hash
                        ))
                    })
            }
            Ok((class_hash, None)) => {
                let class_hash_felt = Felt::from_hex(&class_hash).unwrap();

                self.fetch_and_compile_contract_class(
                    ClassHash(class_hash_felt),
                    self.adjusted_block_id(),
                )?;
                self.in_memory_fork_cache
                    .borrow()
                    .get_compiled_class(ClassHash(class_hash_felt))
                    .map_err(|_| {
                        StateError::StateReadError(format!(
                            "Failed to retrieve compiled contract class for class_hash: {}",
                            class_hash
                        ))
                    })
            }
            Err(e) => Err(StateError::StateReadError(format!("{}", e))),
        }
    }

    fn get_compiled_class_hash(&self, _class_hash: ClassHash) -> StateResult<CompiledClassHash> {
        Err(StateReadError(
            "Unable to get compiled class hash from the fork".to_string(),
        ))
    }
}
