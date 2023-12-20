use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use simulate::{simulate, SimulationArgs, SimulationRes};

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_addr)
        .await
        .unwrap();

    // let q = "SELECT * FROM simulations WHERE project_id = 1 AND wallet_address = '0x75160f33545357e0906b1cb5cacc9e4fcec206a258e4cd15802a5658b209db2';";
    // let q = "SELECT id, team_id, chain_id, block_at, transaction_version, nonce, max_fee, cairo_version, wallet_address, calldata FROM simulations WHERE team_id=1 LIMIT 20";

    let rows = sqlx::query!("SELECT * FROM simulations WHERE project_id = 2;")
        .fetch_all(&pool)
        .await
        .unwrap();

    let simulations_res: Vec<SimulationRes> = rows
        .into_iter()
        .map(|row| SimulationRes {
            id: row.id.map_or(String::new(), |id| id.to_string()),
            project_id: row.project_id,
            chain_id: row.chain_id,
            block_at: row.block_at,
            transaction_version: row.transaction_version,
            nonce: row.nonce,
            max_fee: row.max_fee,
            cairo_version: row.cairo_version,
            wallet_address: row.wallet_address,
            calldata: row.calldata.map_or(Vec::new(), |calldata| calldata),
            created_at: row.created_at.assume_utc().unix_timestamp(),
            updated_at: row.updated_at.assume_utc().unix_timestamp(),
            status: row.status,
        })
        .collect();

    for sim in simulations_res.iter() {
        let tx_info = simulate(SimulationArgs {
            chain_id: sim.chain_id.clone(),
            block_at: (sim.block_at as u64).clone(),
            nonce: (sim.nonce as u64).clone(),
            wallet_address: sim.wallet_address.clone(),
            calldata: sim.calldata.clone(),
        });

        let sim_status = match tx_info {
            Ok(tx) => match tx.revert_error {
                Some(_) => "failure",
                None => "success",
            },
            Err(_) => "failure",
        };

        if sim_status != sim.status {
            println!("{}: {} -> {}", sim.id, sim.status, sim_status);
        }

        // sqlx::query!(
        //     "UPDATE simulations SET status = $1 WHERE id = $2",
        //     sim_status,
        //     id,
        // )
        // .execute(&state.db_pool)
        // .await
        // .unwrap();
    }
}
