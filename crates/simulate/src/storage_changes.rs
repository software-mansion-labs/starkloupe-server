use blockifier::state::cached_state::StorageEntry;
use cairo_lang_sierra_to_casm::compiler::CairoProgram;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use internal_tracing::{
    call_trace::ESysCall,
    utils::{get_pc_sys_call_mappings, get_system_call_at_trace_step},
};
use starknet::core::types::{BlockId, Felt};
use starknet::providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use starknet_api::{core::ContractAddress, state::StorageKey};
use std::collections::HashMap;

use crate::contract_calls_map::ContractCallsMap;

pub fn get_storage_changes(
    casm_program: &CairoProgram,
    vm_trace: &[RelocatedTraceEntry],
    vm_memory: &[Option<Felt>],
    storage_view: &HashMap<StorageEntry, Felt>,
    contract_address: ContractAddress,
) -> Option<HashMap<String, (Option<String>, String)>> {
    let pc_to_ptr_sys_calls = get_pc_sys_call_mappings(vm_trace, &casm_program.instructions);
    let mut storage_changes: HashMap<String, (Option<String>, String)> = HashMap::new();
    for trace_entry in vm_trace {
        let syscall =
            get_system_call_at_trace_step(&pc_to_ptr_sys_calls, vm_memory, trace_entry, None, None);
        match syscall {
            Some(ESysCall::StorageWrite(storage_write)) => {
                let before = storage_view
                    .get(&(
                        contract_address,
                        StorageKey(storage_write.address.try_into().unwrap()),
                    ))
                    .cloned();
                storage_changes.insert(
                    storage_write.address.to_hex_string(),
                    (
                        before.map(|before| before.to_hex_string()),
                        storage_write.value.to_hex_string(),
                    ),
                );
            }
            _ => {}
        }
    }

    Some(storage_changes)
}

pub async fn fetch_before_storage_changes(
    mut storage_changes: HashMap<u32, HashMap<String, (Option<String>, String)>>,
    contract_calls_map: &ContractCallsMap,
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> HashMap<u32, HashMap<String, (String, String)>> {
    let mut filtered_storage_changes = HashMap::new();
    for (call_id, changes) in storage_changes.iter_mut() {
        for (address, (before, _after)) in changes.iter_mut() {
            if before.is_none() {
                let contract_address_felt = Felt::from(
                    contract_calls_map
                        .0
                        .get(call_id)
                        .unwrap()
                        .entry_point
                        .storage_address,
                );
                let before_from_provider = provider_client
                    .get_storage_at(
                        contract_address_felt,
                        Felt::from_hex(address).unwrap(),
                        BlockId::Number(block_number),
                    )
                    .await
                    .ok();
                *before = before_from_provider.map(|before| before.to_hex_string());
            }
        }
        let mut filtered_contract_storage_changes = HashMap::new();
        for (address, (before, after)) in changes.iter() {
            if let Some(before) = before {
                if before != after {
                    filtered_contract_storage_changes
                        .insert(address.clone(), (before.clone(), after.clone()));
                }
            }
        }
        if !filtered_contract_storage_changes.is_empty() {
            filtered_storage_changes.insert(*call_id, filtered_contract_storage_changes);
        }
    }
    filtered_storage_changes
}
