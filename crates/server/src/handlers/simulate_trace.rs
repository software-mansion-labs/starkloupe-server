use crate::app_state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use blockifier::transaction::objects::TransactionExecutionInfo;
use db::Simulation;
use serde::Serialize;
use simulate::{simulate, SimulationArgs};
use sqlx::types::Uuid;
use starknet::core::types::{
    CallType, EventContent, ExecuteInvocation, FeeEstimate, FieldElement, FunctionInvocation,
    InvokeTransactionTrace, SimulatedTransaction, TransactionTrace,
};
use starknet_api::hash::StarkFelt;
use std::{str::FromStr, sync::Arc};

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    simulated_transaction: SimulatedTransaction,
    simulation: Option<Simulation>,
}

pub async fn simulate_trace(
    State(state): State<Arc<AppState>>,
    id: Path<String>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    // Implement your business logic here
    let sim: Simulation = sqlx::query_as!(
        Simulation,
        "SELECT * FROM simulations WHERE id = $1",
        Uuid::from_str(&id.0).unwrap()
    )
    .fetch_one(&state.db_pool)
    .await
    .unwrap();

    let tx_info = simulate(SimulationArgs {
        chain_id: sim.chain_id.clone(),
        block_at: (sim.block_at as u64).clone(),
        nonce: (sim.nonce as u64).clone(),
        wallet_address: sim.wallet_address.clone(),
        calldata: sim.calldata.clone().unwrap_or_default(),
    });

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: to_simulated_transaction(tx_info),
            simulation: Some(sim),
        })),
        Err(_) => Err(StatusCode::EXPECTATION_FAILED),
    }
}

pub async fn simulate_transaction(
    Json(payload): Json<SimulationArgs>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    let tx_info = simulate(payload);

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: to_simulated_transaction(tx_info),
            simulation: None,
        })),
        Err(e) => {
            dbg!(e);
            Err(StatusCode::EXPECTATION_FAILED)
        }
    }
}

fn to_simulated_transaction(tx: TransactionExecutionInfo) -> SimulatedTransaction {
    let eci = tx.execute_call_info.unwrap();

    SimulatedTransaction {
        transaction_trace: TransactionTrace::Invoke(InvokeTransactionTrace {
            execute_invocation: ExecuteInvocation::Success(to_function_invocation(&eci)),
            // TODO: Implement this
            validate_invocation: None,
            fee_transfer_invocation: None,
        }),
        // TODO: Implement this
        fee_estimation: FeeEstimate {
            gas_consumed: 0,
            gas_price: 0,
            overall_fee: 0,
        },
    }
}

fn to_function_invocation(eci: &blockifier::execution::call_info::CallInfo) -> FunctionInvocation {
    let calldata = eci
        .call
        .calldata
        .0
        .iter()
        .map(|x| FieldElement::from_byte_slice_be(x.bytes()).unwrap())
        .collect();

    let events: Vec<EventContent> = eci
        .execution
        .events
        .iter()
        .map(|x| EventContent {
            data: x
                .event
                .data
                .0
                .iter()
                .map(|x| FieldElement::from(*x))
                .collect(),
            keys: x
                .event
                .keys
                .iter()
                .map(|x| FieldElement::from(x.0))
                .collect(),
        })
        .collect();

    let result = eci
        .execution
        .retdata
        .0
        .iter()
        .map(|x| FieldElement::from(*x))
        .collect();

    let internal_calls: Vec<FunctionInvocation> = eci
        .inner_calls
        .iter()
        .map(|x| to_function_invocation(x))
        .collect();

    FunctionInvocation {
        calldata: calldata,
        contract_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.storage_address.0.key(),
        )),
        call_type: match eci.call.call_type {
            blockifier::execution::entry_point::CallType::Call => CallType::Call,
            blockifier::execution::entry_point::CallType::Delegate => CallType::LibraryCall,
        },
        caller_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.caller_address.0.key(),
        )),
        class_hash: FieldElement::from(eci.call.class_hash.unwrap().0),
        entry_point_type: match eci.call.entry_point_type {
            starknet_api::deprecated_contract_class::EntryPointType::Constructor => {
                starknet::core::types::EntryPointType::Constructor
            }
            starknet_api::deprecated_contract_class::EntryPointType::External => {
                starknet::core::types::EntryPointType::External
            }
            starknet_api::deprecated_contract_class::EntryPointType::L1Handler => {
                starknet::core::types::EntryPointType::L1Handler
            }
        },
        events: events,
        // TODO: Implement this
        messages: vec![],
        result: result,
        entry_point_selector: FieldElement::from(eci.call.entry_point_selector.0),
        calls: internal_calls,
    }
}
