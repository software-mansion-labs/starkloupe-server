use anyhow::{Context, Error};
use byteorder::{ByteOrder, LittleEndian};
use cairo_felt::{Felt252, PRIME_STR};
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
use indextree::{Arena, NodeId};
use itertools::chain;
use num_bigint::{BigInt, BigUint};
use serde::Serialize;
use serde_json::Value;
use starknet_api::core::ClassHash;
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

pub fn get_internal_fn_call_trace(
    class_hash: ClassHash,
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
) -> Option<InternalFnCallTraceEntryNode> {
    let folder_with_precompiled_contracts = "precompiled-contracts";
    let file_name_with_sierra_contract = format!("{}.json", class_hash.to_string());
    let file_path_with_sierra_contract =
        Path::new(folder_with_precompiled_contracts).join(file_name_with_sierra_contract);
    if file_path_with_sierra_contract.exists() {
        // Parse Sierra contract class
        let sierra_json = read_json(file_path_with_sierra_contract)
            .expect("Unable to read Sierra contract json file");
        let sierra_class: ContractClass = serde_json::from_value(sierra_json).unwrap();

        // Extract Sierra program
        let sierra_program = sierra_class.extract_sierra_program().unwrap();

        // Compile Sierra contract class to CASM Program
        let casm_program = compile_sierra_contract_class(sierra_class, usize::MAX);

        let casm_to_sierra_map = make_casm_to_sierra_map(&casm_program.debug_info, 0);

        let (pc_inst_map, pc_to_inst_indexes_map) = get_pc_mappings(relocated_memory, vm_trace);

        let memory_map: HashMap<usize, BigInt> = relocated_memory
            .iter()
            .filter_map(|x| x.as_ref().map(|_| x.clone().unwrap()))
            .map(|x| x.to_bigint())
            .enumerate()
            .map(|(i, v)| (i + 1, v))
            .collect();

        Some(get_internal_fn_calls_trace(
            vm_trace,
            pc_to_inst_indexes_map,
            casm_to_sierra_map,
            &sierra_program,
        ))
    } else {
        println!(
            "Can't find Sierra contract for class hash: {:#?}",
            class_hash.to_string()
        );
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntry {
    pub fn_name: Option<String>,
    pub fp: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntryNode {
    pub data: InternalFnCallTraceEntry,
    pub nested_calls: Vec<InternalFnCallTraceEntryNode>,
}

#[derive(Debug)]
struct InternalFnCallTraceTree {
    arena: Arena<InternalFnCallTraceEntry>,
    current_node: NodeId,
    root: NodeId,
}

impl InternalFnCallTraceTree {
    fn new(root_entry: InternalFnCallTraceEntry) -> Self {
        let mut arena = Arena::new();
        let root = arena.new_node(root_entry);
        InternalFnCallTraceTree {
            arena,
            current_node: root,
            root,
        }
    }

    fn add_child(&mut self, entry: InternalFnCallTraceEntry) {
        let child = self.arena.new_node(entry);
        self.current_node.append(child, &mut self.arena);
        self.current_node = child;
    }

    fn move_to_parent(&mut self) {
        let mut ancestors = self.current_node.ancestors(&self.arena);
        ancestors.next();
        if let Some(parent) = ancestors.next() {
            self.current_node = parent;
        }
    }

    fn get_serializable(&self, node_id: NodeId) -> InternalFnCallTraceEntryNode {
        let mut nested_calls = Vec::new();
        for child_node_id in node_id.children(&self.arena) {
            nested_calls.push(self.get_serializable(child_node_id));
        }
        InternalFnCallTraceEntryNode {
            data: self.arena[node_id].get().clone(),
            nested_calls,
        }
    }

    fn get_root_serializable(&self) -> InternalFnCallTraceEntryNode {
        self.get_serializable(self.root)
    }
}

fn get_internal_fn_calls_trace(
    vm_trace: &Vec<TraceEntry>,
    pc_to_inst_indexes_map: HashMap<usize, usize>,
    casm_to_sierra_map: HashMap<usize, Vec<usize>>,
    sierra_program: &Program,
) -> InternalFnCallTraceEntryNode {
    let first_vm_trace_entry = vm_trace.first().unwrap();
    let mut current_fp = first_vm_trace_entry.fp;

    let entrypoint_internal_fn_name = get_internal_fn_name(
        &first_vm_trace_entry.pc,
        &pc_to_inst_indexes_map,
        &casm_to_sierra_map,
        sierra_program,
    );

    let mut tree = InternalFnCallTraceTree::new(InternalFnCallTraceEntry {
        fn_name: entrypoint_internal_fn_name,
        fp: current_fp,
    });

    for trace_entry in vm_trace.iter() {
        let new_fp = trace_entry.fp;
        if new_fp > current_fp {
            // println!("current_fp: {}; new_fp: {}", current_fp, new_fp);
            tree.add_child(InternalFnCallTraceEntry {
                fn_name: get_internal_fn_name(
                    &trace_entry.pc,
                    &pc_to_inst_indexes_map,
                    &casm_to_sierra_map,
                    sierra_program,
                ),
                fp: new_fp,
            });
            current_fp = trace_entry.fp;
        } else if new_fp < current_fp {
            // println!("current_fp: {}; new_fp: {}", current_fp, new_fp);
            tree.move_to_parent();
            current_fp = trace_entry.fp;
        }
    }

    tree.get_root_serializable()
}

fn get_internal_fn_name(
    pc: &usize,
    pc_to_inst_indexes_map: &HashMap<usize, usize>,
    casm_to_sierra_map: &HashMap<usize, Vec<usize>>,
    sierra_program: &Program,
) -> Option<String> {
    let casm_index = pc_to_inst_indexes_map
        .get(pc)
        .expect("Failed to get casm index");
    let sierra_indexes = casm_to_sierra_map.get(casm_index);
    if let Some(sierra_indexes) = sierra_indexes {
        let first_sierra_index = sierra_indexes.first();
        if let Some(first_sierra_index) = first_sierra_index {
            let func = sierra_program
                .funcs
                .iter()
                .find(|&func| func.entry_point.0 == *first_sierra_index);
            if let Some(func) = func {
                return Some(func.to_string());
            }
        }
    }
    return None;
}

fn get_pc_mappings(
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

fn compile_sierra_contract_class(
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

fn make_casm_to_sierra_map(
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

// Returns the encoded instruction (the value at pc) and the immediate value (the value at
// pc + 1, if it exists in the memory).
pub fn get_instruction_encoding(
    pc: usize,
    memory: &[Option<Felt252>],
) -> anyhow::Result<(Felt252, Option<Felt252>)> {
    if memory[pc].is_none() {
        return Err(Error::msg("Memory at pc is None"));
    }
    let instruction_encoding = memory[pc].clone().unwrap();
    let prime = BigUint::parse_bytes(PRIME_STR[2..].as_bytes(), 16).unwrap();

    let imm_addr = BigUint::from(pc + 1) % prime;
    let imm_addr = usize::try_from(imm_addr.clone()).map_err(|_| Error::msg(""))?;
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

fn format_sierra_program(sierra_program: &Program) -> SierraFormattedProgram {
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

fn read_json(file_path: PathBuf) -> anyhow::Result<Value> {
    let sierra_file = File::open(file_path).context("Unable to open json file")?;
    serde_json::from_reader(sierra_file).context("Unable to read json file")
}
