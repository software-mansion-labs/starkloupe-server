pub mod source_code;
pub mod source_code_mappings;
pub mod utils;
use crate::source_code_mappings::SourceCodeMappings;

use anyhow::Error;
use anyhow::{anyhow, Result};
use cairo_felt::{Felt252, PRIME_STR};
use cairo_lang_sierra::{
    ids::ConcreteTypeId,
    program::{GenFunction, Program, StatementIdx},
};
use cairo_lang_sierra_type_size::TypeSizeMap;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use indextree::{Arena, NodeId};
use num_bigint::BigUint;
use serde::Serialize;
use smol_str::SmolStr;
use source_code_mappings::CodeLocation;
use starknet_api::core::ClassHash;
use std::collections::{HashMap, HashSet};

pub fn get_internal_fn_call_trace(
    class_hash: ClassHash,
    relocated_memory: &Vec<Option<Felt252>>,
    vm_trace: &Vec<TraceEntry>,
) -> Result<(InternalFnCallTraceEntryNode, Option<Vec<String>>)> {
    let mappings = SourceCodeMappings::new(class_hash, relocated_memory, vm_trace)?;
    let is_cairo_debug_data = mappings.cairo_debug_data.is_some();

    let mut used_source_files: HashSet<String> = HashSet::new();

    let first_vm_trace_entry = vm_trace.first().unwrap();
    let mut current_fp = first_vm_trace_entry.fp;

    let entrypoint_function = get_sierra_function_at_pc(
        &first_vm_trace_entry.pc,
        &mappings.common_debug_data.pc_to_inst_indexes_map,
        &mappings.common_debug_data.casm_to_sierra_map,
        &mappings.common_debug_data.sierra_program,
    );

    let entrypoint_cairo_locations = mappings.get_cairo_locations_at_pc(&first_vm_trace_entry.pc);
    entrypoint_cairo_locations.iter().for_each(|loc| {
        used_source_files.insert(loc.file_path.clone());
    });

    let mut tree = InternalFnCallTraceTree::new(create_internal_fn_call_trace_entry(
        entrypoint_function,
        current_fp,
        &mappings.common_debug_data.type_sizes,
        &mappings.common_debug_data.type_names,
        entrypoint_cairo_locations,
    ));

    for trace_entry in vm_trace.iter() {
        let new_fp = trace_entry.fp;
        if new_fp > current_fp {
            // new function call
            let function = get_sierra_function_at_pc(
                // get sierra function at pc
                &trace_entry.pc,
                &mappings.common_debug_data.pc_to_inst_indexes_map,
                &mappings.common_debug_data.casm_to_sierra_map,
                &mappings.common_debug_data.sierra_program,
            );

            let cairo_locations = mappings.get_cairo_locations_at_pc(&trace_entry.pc);
            cairo_locations.iter().for_each(|loc| {
                used_source_files.insert(loc.file_path.clone());
            });

            let mut call_entry = create_internal_fn_call_trace_entry(
                function,
                new_fp,
                &mappings.common_debug_data.type_sizes,
                &mappings.common_debug_data.type_names,
                cairo_locations,
            );

            // https://docs.cairo-lang.org/how_cairo_works/functions.html#argument
            // TODO: check if is algorithm is correct and fix if needed
            // for argument in call_entry.arguments.iter_mut().rev() {
            //     let type_size = argument.type_size as usize;
            //     let mut values: Vec<String> = Vec::new();
            //     for i in 1..(type_size + 1) {
            //         let addr = trace_entry.ap - 2 - type_size + i;
            //         let value = mappings.common_debug_data.memory_map.get(&addr).unwrap();
            //         values.push(value.clone().to_string());
            //     }
            //     argument.value = values;
            // }
            tree.add_child(call_entry);
            current_fp = trace_entry.fp;
        } else if new_fp < current_fp {
            // return from function
            // let exit_function = tree.get_node_data(tree.current_node);
            // let mut result_values: Vec<Vec<String>> = Vec::new();
            // let mut offset = 0;

            // https://docs.cairo-lang.org/how_cairo_works/functions.html#return-values
            // TODO: check if is algorithm is correct and fix if needed
            // for result in exit_function.results.iter().rev() {
            //     let type_size = result.type_size as usize;
            //     let mut values: Vec<String> = Vec::new();
            //     for i in 1..(type_size + 1) {
            //         let addr = trace_entry.ap - type_size + i - offset;
            //         let value = mappings.common_debug_data.memory_map.get(&addr).unwrap();
            //         values.push(value.clone().to_string());
            //     }
            //     result_values.push(values);
            //     offset += type_size;
            // }
            // result_values.reverse();
            // tree.set_result_values_to_current_node(result_values);
            tree.move_to_parent();
            current_fp = trace_entry.fp;
        }
    }

    let used_source_files_vec: Option<Vec<String>> = match is_cairo_debug_data {
        true => Some(used_source_files.into_iter().collect()),
        false => None,
    };
    Ok((tree.get_root_serializable(), used_source_files_vec))
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallTraceEntry {
    pub fn_name: Option<String>,
    pub fp: usize,
    // pub results: Vec<InternalFnCallIO>,
    // pub arguments: Vec<InternalFnCallIO>,
    pub cairo_locations: Vec<CodeLocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InternalFnCallIO {
    pub type_name: String,
    pub type_size: i16,
    pub value: Vec<String>,
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

    // fn set_result_values_to_current_node(&mut self, result_values: Vec<Vec<String>>) {
    //     if let Some(node) = self.arena.get_mut(self.current_node) {
    //         let data = node.get_mut();
    //         for (i, result) in data.results.iter_mut().enumerate() {
    //             result.value = result_values[i].clone();
    //         }
    //     }
    // }

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

    fn get_node_data(&self, node_id: NodeId) -> &InternalFnCallTraceEntry {
        &self.arena[node_id].get()
    }
}

fn create_internal_fn_call_trace_entry(
    function: Option<&GenFunction<StatementIdx>>,
    fp: usize,
    type_sizes: &TypeSizeMap,
    type_names: &HashMap<ConcreteTypeId, SmolStr>,
    cairo_locations: Vec<CodeLocation>,
) -> InternalFnCallTraceEntry {
    let mut results: Vec<InternalFnCallIO> = Vec::new();
    let mut arguments: Vec<InternalFnCallIO> = Vec::new();
    if let Some(function) = function {
        function.signature.ret_types.iter().for_each(|ret_type| {
            results.push(InternalFnCallIO {
                type_name: type_names.get(ret_type).unwrap().to_string(),
                type_size: *type_sizes.get(&ret_type).unwrap(),
                value: Vec::new(),
            });
        });
        function.params.iter().for_each(|param| {
            arguments.push(InternalFnCallIO {
                type_name: type_names.get(&param.ty).unwrap().to_string(),
                type_size: *type_sizes.get(&param.ty).unwrap(),
                value: Vec::new(),
            });
        });
    }
    // InternalFnCallTraceEntry {
    //     fn_name: function.map(|f| f.to_string()),
    //     fp,
    //     results,
    //     arguments,
    //     cairo_locations,
    // }
    InternalFnCallTraceEntry {
        fn_name: function.map(|f| f.to_string()),
        fp,
        cairo_locations,
    }
}

fn get_sierra_function_at_pc<'a>(
    pc: &'a usize,
    pc_to_inst_indexes_map: &'a HashMap<usize, usize>,
    casm_to_sierra_map: &'a HashMap<usize, Vec<usize>>,
    sierra_program: &'a Program,
) -> Option<&'a GenFunction<StatementIdx>> {
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
            return func;
        }
    }
    return None;
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
