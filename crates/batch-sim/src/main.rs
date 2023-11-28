use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use sqlx::types::Uuid;
use url::Url;

use starknet::core::types::{
    BlockId, BroadcastedInvokeTransaction, BroadcastedTransaction, FieldElement, SimulationFlag,
};
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};

fn create_rpc_client(chain_id: String) -> JsonRpcClient<HttpTransport> {
    let url = match chain_id.as_str() {
        "0x534e5f474f45524c49" => "https://3dfa-54-87-10-131.ngrok-free.app",
        "0x534e5f4d41494e" => "https://0721-54-87-10-131.ngrok-free.app",
        _ => panic!("Invalid chain id"),
    };
    JsonRpcClient::new(HttpTransport::new(Url::parse(url).unwrap()))
}

#[derive(Clone, Debug, Default)]
pub struct Simulation {
    pub id: Uuid,
    pub team_id: i32,
    pub chain_id: String,
    pub block_at: i32,
    pub transaction_version: i32,
    pub nonce: i32,
    pub max_fee: String,
    pub cairo_version: String,
    pub wallet_address: String,
    pub calldata: Vec<String>,
    // pub created_at: NaiveDateTime,
    // pub updated_at: NaiveDateTime,
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("debug"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_addr)
        .await
        .unwrap();

    let q = "SELECT * FROM simulations WHERE team_id = 1 AND wallet_address = '0x75160f33545357e0906b1cb5cacc9e4fcec206a258e4cd15802a5658b209db2';";
    // let q = "SELECT id, team_id, chain_id, block_at, transaction_version, nonce, max_fee, cairo_version, wallet_address, calldata FROM simulations WHERE team_id=1 LIMIT 20";

    let rows = sqlx::query!("SELECT * FROM simulations WHERE team_id = 2 AND wallet_address = '0x3e35f10ed657810eb18fc4a57601c2e60a16f27eeab3724bf5525c9db88cfb8' AND block_at > 900000 LIMIT 1;").fetch_all(&pool).await.unwrap();

    let simulations_res: Vec<Simulation> = rows
        .into_iter()
        .map(|simulation| Simulation {
            id: simulation.id.unwrap(),
            team_id: simulation.team_id,
            chain_id: simulation.chain_id,
            block_at: simulation.block_at,
            transaction_version: simulation.transaction_version,
            nonce: simulation.nonce,
            max_fee: simulation.max_fee,
            cairo_version: simulation.cairo_version,
            wallet_address: simulation.wallet_address,
            calldata: simulation.calldata.map_or(Vec::new(), |calldata| calldata),
            // created_at: NaiveDateTime::from_timestamp_opt(simulation.created_at.timestamp(), 0)
            //     .unwrap(),
            // updated_at: NaiveDateTime::from_timestamp_opt(simulation.updated_at.timestamp(), 0)
            //     .unwrap(),
        })
        .collect();

    for sim in simulations_res.iter() {
        let rpc_client = create_rpc_client(sim.chain_id.clone());

        let tx_b = BroadcastedTransaction::Invoke(BroadcastedInvokeTransaction {
            sender_address: FieldElement::from_hex_be(sim.wallet_address.as_str()).unwrap(),
            calldata: sim.calldata.clone()
                .iter()
                .map(|s| FieldElement::from_dec_str(s.as_str()).unwrap())
                .collect(),
            max_fee: FieldElement::from_dec_str("23200000090853470717981").unwrap(),
            signature: vec![],
            nonce: FieldElement::from_dec_str(sim.nonce.to_string().as_str()).unwrap(),
            is_query: false,
        });
        dbg!(tx_b.clone());
        let st = rpc_client
            .simulate_transaction(
                BlockId::Number(sim.block_at as u64),
                tx_b,
                [SimulationFlag::SkipValidate, SimulationFlag::SkipFeeCharge],
            )
            .await;

        dbg!(st);
    }

    // let s: db::Simulation = rows[0].into();
    // dbg!(s);

    // while let Some(row) = rows.try_next().await? {
    //     // map the row into a user-defined domain type
    //     let email: &str = row.try_get("email")?;
    // }
}
