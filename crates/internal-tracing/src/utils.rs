use std::collections::HashMap;

use byteorder::{ByteOrder, LittleEndian};
use cairo_felt::Felt252;
use cairo_lang_sierra::extensions::gas::CostTokenType;
use cairo_lang_sierra_to_casm::{
    compiler::{CairoProgram, CairoProgramDebugInfo, SierraToCasmConfig},
    metadata::{calc_metadata, MetadataComputationConfig},
};
use cairo_lang_starknet_classes::{
    casm_contract_class::ENTRY_POINT_COST, contract_class::ContractClass,
    felt252_serde::sierra_from_felt252s,
};
use cairo_vm::{types::instruction::{Instruction, Op1Addr}, vm::{decoding::decoder::decode_instruction, trace::trace_entry::TraceEntry}};
use itertools::chain;

use crate::get_instruction_encoding;

pub fn compile_sierra_contract_class(
    contract_class: ContractClass,
    max_bytecode_size: usize,
) -> CairoProgram {
    let (sierra_version, _, program) =
        sierra_from_felt252s(&contract_class.sierra_program).unwrap();

    let entrypoint_function_indices = chain!(
        &contract_class.entry_points_by_type.constructor,
        &contract_class.entry_points_by_type.external,
        &contract_class.entry_points_by_type.l1_handler,
    )
    .map(|entrypoint| entrypoint.function_idx);

    let entrypoint_ids = entrypoint_function_indices.map(|idx| program.funcs[idx].id.clone());

    let no_eq_solver = sierra_version.minor >= 4;

    let metadata_computation_config = MetadataComputationConfig {
        function_set_costs: entrypoint_ids
            .map(|id| (id, [(CostTokenType::Const, ENTRY_POINT_COST)].into()))
            .collect(),
        linear_gas_solver: no_eq_solver,
        linear_ap_change_solver: no_eq_solver,
        skip_non_linear_solver_comparisons: false,
        compute_runtime_costs: false,
    };

    let metadata = calc_metadata(&program, metadata_computation_config).unwrap();

    cairo_lang_sierra_to_casm::compiler::compile(
        &program,
        &metadata,
        SierraToCasmConfig {
            gas_usage_check: true,
            max_bytecode_size,
        },
    )
    .unwrap()
}

pub fn make_casm_to_sierra_map(
    debug_info: &CairoProgramDebugInfo,
    casm_headers_len: usize,
) -> HashMap<usize, Vec<usize>> {
    let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
    let sierra_statement_info_len = debug_info.sierra_statement_info.len();
    for (i, sierra_info) in debug_info
        .sierra_statement_info
        .iter()
        .enumerate()
        .take(sierra_statement_info_len - 1)
    {
        let key = sierra_info.instruction_idx + casm_headers_len;
        map.entry(key).or_insert_with(Vec::new).push(i);
    }
    map
}

pub fn get_pc_mappings(
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
) -> (HashMap<usize, Instruction>, HashMap<usize, usize>) {
    let max_pc_entry = vm_trace.iter().max_by(|a, b| a.pc.cmp(&b.pc));

    let max_pc = match max_pc_entry {
        Some(max_entry) => max_entry.pc,
        None => {
            println!("No entries in the trace");
            0
        }
    };

    let mut pc_inst_map: HashMap<usize, Instruction> = HashMap::new();
    // let mut pc_inst_serialized_map: HashMap<usize, InstructionSerializable> = HashMap::new();
    let mut pc_to_inst_indexes_map: HashMap<usize, usize> = HashMap::new();

    let mut skip_next_pc = false;
    let mut casm_index: usize = 0;
    for pc in 1..=max_pc {
        if skip_next_pc {
            skip_next_pc = false;
            continue;
        }

        let (instruction_encoding_felt, _) = get_instruction_encoding(pc, &relocated_memory)
            .expect("Failed to get instruction encoding");
        let instruction_encoding_bytes_le = instruction_encoding_felt.to_le_bytes();
        let instruction_encoding_u64 = LittleEndian::read_u64(&instruction_encoding_bytes_le[..]);
        let instruction =
            decode_instruction(instruction_encoding_u64).expect("Failed to decode instruction");
        pc_inst_map.insert(pc, instruction.clone());
        if instruction.op1_addr == Op1Addr::Imm {
            skip_next_pc = true;
        }
        // pc_inst_serialized_map.insert(pc, InstructionSerializable(instruction));
        pc_to_inst_indexes_map.insert(pc, casm_index);
        casm_index += 1;
    }
    (pc_inst_map, pc_to_inst_indexes_map)
}
