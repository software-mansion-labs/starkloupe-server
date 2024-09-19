use anyhow::{Error, Result};
use byteorder::{ByteOrder, LittleEndian};
use cairo_felt::{Felt252, PRIME_STR};
use cairo_lang_casm::{
    cell_expression::CellExpression,
    hints::{Hint, StarknetHint},
    instructions::Instruction as CasmInstruction,
};
use cairo_lang_sierra::{extensions::gas::CostTokenType, program::Program};
use cairo_lang_sierra_to_casm::{
    compiler::{CairoProgram, CairoProgramDebugInfo, SierraToCasmConfig},
    metadata::{calc_metadata, MetadataComputationConfig},
};
use cairo_lang_starknet_classes::{
    casm_contract_class::ENTRY_POINT_COST, contract_class::ContractClass,
    felt252_serde::sierra_from_felt252s,
};
use cairo_vm::{
    types::instruction::{Instruction, Op1Addr},
    vm::{decoding::decoder::decode_instruction, trace::trace_entry::TraceEntry},
};
use itertools::chain;
use num_bigint::BigUint;
use serde::Serialize;
use starknet_api::hash::StarkFelt;
use std::collections::HashMap;

pub fn compile_sierra_contract_class(
    contract_class: ContractClass,
    max_bytecode_size: usize,
) -> Result<CairoProgram> {
    let (sierra_version, _, program) = sierra_from_felt252s(&contract_class.sierra_program)
        .map_err(|e| anyhow::anyhow!("Failed to parse Sierra program: {:?}", e))?;

    let entrypoint_function_indices = chain!(
        &contract_class.entry_points_by_type.constructor,
        &contract_class.entry_points_by_type.external,
        &contract_class.entry_points_by_type.l1_handler,
    )
    .map(|entrypoint| entrypoint.function_idx);

    let entrypoint_ids: Vec<_> = entrypoint_function_indices
        .map(|idx| program.funcs[idx].id.clone())
        .collect();

    let no_eq_solver = sierra_version.minor >= 4;

    let metadata_computation_config = MetadataComputationConfig {
        function_set_costs: entrypoint_ids
            .into_iter()
            .map(|id| (id, [(CostTokenType::Const, ENTRY_POINT_COST)].into()))
            .collect(),
        linear_gas_solver: no_eq_solver,
        linear_ap_change_solver: no_eq_solver,
        skip_non_linear_solver_comparisons: false,
        compute_runtime_costs: false,
    };

    let metadata = calc_metadata(&program, metadata_computation_config)
        .map_err(|e| anyhow::anyhow!("Failed to calculate metadata: {:?}", e))?;

    let compiled_program = cairo_lang_sierra_to_casm::compiler::compile(
        &program,
        &metadata,
        SierraToCasmConfig {
            gas_usage_check: true,
            max_bytecode_size,
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to compile Sierra to Casm: {:?}", e))?;

    Ok(compiled_program)
}

pub fn make_casm_to_sierra_map(debug_info: &CairoProgramDebugInfo) -> HashMap<usize, Vec<usize>> {
    let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, sierra_info) in debug_info.sierra_statement_info.iter().enumerate() {
        let key = sierra_info.instruction_idx;
        map.entry(key).or_insert_with(Vec::new).push(i);
    }
    map
}

pub fn get_pc_mappings(
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
) -> Result<(HashMap<usize, Instruction>, HashMap<usize, usize>)> {
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
        let instruction = decode_instruction(instruction_encoding_u64)?;
        pc_inst_map.insert(pc, instruction.clone());
        if instruction.op1_addr == Op1Addr::Imm {
            skip_next_pc = true;
        }
        // pc_inst_serialized_map.insert(pc, InstructionSerializable(instruction));
        pc_to_inst_indexes_map.insert(pc, casm_index);
        casm_index += 1;
    }
    Ok((pc_inst_map, pc_to_inst_indexes_map))
}

pub fn get_pc_to_ptr_sys_call_mappings(
    casm_instructions: &Vec<CasmInstruction>,
    pc_to_inst_indexes_map: &HashMap<usize, usize>,
) -> HashMap<usize, CellExpression> {
    pc_to_inst_indexes_map
        .iter()
        .filter_map(|(pc, casm_index)| {
            //TODO! Check why this happen
            if *casm_index >= casm_instructions.len() {
                return None;
            }
            let instruction = casm_instructions[*casm_index].clone();
            if let Some(system_ptr) = instruction.hints.iter().find_map(|hint| match hint {
                Hint::Starknet(starknet_hint) => match starknet_hint {
                    StarknetHint::SystemCall { system } => {
                        Some(CellExpression::from_res_operand(system.clone()))
                    }
                    _ => None,
                },
                _ => None,
            }) {
                Some((*pc, system_ptr))
            } else {
                None
            }
        })
        .collect()
}

// Returns the encoded instruction (the value at pc) and the immediate value (the value at
// pc + 1, if it exists in the memory).
pub fn get_instruction_encoding(
    pc: usize,
    memory: &[Option<Felt252>],
) -> Result<(Felt252, Option<Felt252>)> {
    if memory[pc].is_none() {
        return Err(Error::msg("Memory at pc is None"));
    }
    let instruction_encoding = memory
        .get(pc)
        .and_then(|value| value.clone())
        .ok_or_else(|| {
            anyhow::Error::msg(format!("Memory at pc = {} is None or out of bounds", pc))
        })?;
    let prime = BigUint::parse_bytes(PRIME_STR[2..].as_bytes(), 16)
        .ok_or_else(|| anyhow::Error::msg("Failed to parse prime"))?;
    let imm_addr = BigUint::from(pc + 1) % prime;
    let imm_addr = usize::try_from(imm_addr.clone())
        .map_err(|_| anyhow::Error::msg("Failed to convert imm_addr to usize"))?;
    let optional_imm = memory[imm_addr].clone();
    Ok((instruction_encoding, optional_imm))
}

#[derive(Serialize, Debug)]
pub struct SierraFormattedProgram {
    pub type_declarations: Vec<String>,
    pub libfunc_declarations: Vec<String>,
    pub statements: Vec<String>,
    pub funcs: Vec<String>,
}

pub fn format_sierra_program(sierra_program: Program) -> SierraFormattedProgram {
    SierraFormattedProgram {
        type_declarations: sierra_program
            .type_declarations
            .iter()
            .map(|type_decl| type_decl.to_string())
            .collect(),
        libfunc_declarations: sierra_program
            .libfunc_declarations
            .iter()
            .map(|libfunc_decl| libfunc_decl.to_string())
            .collect(),
        statements: sierra_program
            .statements
            .iter()
            .enumerate()
            .map(|(index, statement)| format!("{} // {}", statement.to_string(), index))
            .collect(),
        funcs: sierra_program
            .funcs
            .iter()
            .map(|func| func.to_string())
            .collect(),
    }
}

pub fn felt_to_stark_felt(felt: &Felt252) -> StarkFelt {
    let biguint = format!("{:#x}", felt.to_biguint());
    StarkFelt::try_from(biguint.as_str()).expect("Felt252 must be in StarkFelt's range.")
}

pub fn is_panic_result(return_type: &Option<String>) -> bool {
    return_type
        .as_ref()
        .is_some_and(|result_type| result_type.starts_with("core::panics::PanicResult"))
}
