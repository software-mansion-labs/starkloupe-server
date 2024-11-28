use anyhow::Result;
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
};
use itertools::chain;
use serde::Serialize;
use std::collections::HashMap;
use walnut_shared::felt252_serde::sierra_from_felt252s;

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

pub fn get_pc_mappings(instructions: &[CasmInstruction]) -> HashMap<usize, usize> {
    let mut pc_to_inst_indexes_map = HashMap::new();
    let mut offset = 1;
    for (i, inst) in instructions.iter().enumerate() {
        pc_to_inst_indexes_map.insert(offset, i);
        offset += inst.body.op_size();
    }
    pc_to_inst_indexes_map
}

pub fn get_pc_to_ptr_sys_call_mappings(
    casm_instructions: &[CasmInstruction],
    pc_to_inst_indexes_map: &HashMap<usize, usize>,
) -> HashMap<usize, CellExpression> {
    pc_to_inst_indexes_map
        .iter()
        .filter_map(|(&pc, &casm_index)| {
            casm_instructions
                .get(casm_index)?
                .hints
                .iter()
                .find_map(|hint| {
                    if let Hint::Starknet(StarknetHint::SystemCall { system }) = hint {
                        Some((pc, CellExpression::from_res_operand(system.clone())))
                    } else {
                        None
                    }
                })
        })
        .collect()
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

pub fn is_panic_result(return_type: Option<&str>) -> bool {
    return_type.map_or(false, |result_type| result_type.contains("PanicResult"))
}

pub fn get_raw_function_name(fn_name: &str) -> Option<String> {
    if fn_name.is_empty() {
        return None;
    }

    let mut raw_fn_name = String::new();
    let mut inside_generics = 0;
    let mut segment = String::new();

    for char in fn_name.chars() {
        match char {
            '<' => {
                inside_generics += 1;
            }
            '>' => {
                if inside_generics > 0 {
                    inside_generics -= 1;
                }
            }
            ':' if inside_generics == 0 => {
                if !segment.is_empty() {
                    if !raw_fn_name.is_empty() {
                        raw_fn_name.push_str("::");
                    }
                    raw_fn_name.push_str(&segment);
                    segment.clear();
                }
            }
            _ if inside_generics == 0 => {
                segment.push(char);
            }
            _ => {}
        }
    }

    if !segment.is_empty() {
        if !raw_fn_name.is_empty() {
            raw_fn_name.push_str("::");
        }
        raw_fn_name.push_str(&segment);
    }

    Some(raw_fn_name)
}

pub fn is_loop(function_name: &str) -> bool {
    let mut inside_brackets = false;
    let mut found_expr = false;
    let mut digits_found = false;

    for c in function_name.chars() {
        if c == '[' {
            inside_brackets = true;
            found_expr = false;
            digits_found = false;
        } else if c == ']' {
            if inside_brackets && found_expr && digits_found {
                return true;
            }
            inside_brackets = false;
        } else if inside_brackets {
            if !found_expr && function_name.contains("expr") {
                found_expr = true;
            }
            if found_expr && c.is_ascii_digit() {
                digits_found = true;
            }
        }
    }

    false
}
