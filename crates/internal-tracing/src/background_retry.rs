use crate::debugger_data_fetcher::extract_debugger_data_from_contract_class;
use crate::external_class_cache::ExternalClassCache;
use crate::ClassDebuggerDataWithContractClass;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{info, warn};
use verification::db::insert_contract_class;
use verification::s3::upload_class_to_s3;
use verification::voyager::{compile_voyager_source, VoyagerSourceResponse};

/// Manages background retry compilations for contracts that timed out during
/// inline Voyager compilation. On success, persists results to DB + S3 so
/// future transactions find verified code without re-compiling.
#[derive(Clone)]
pub struct BackgroundRetryService {
    semaphore: Arc<Semaphore>,
    /// Class hashes currently being retried (prevents duplicate retries).
    in_flight: Arc<RwLock<HashSet<String>>>,
    db_pool: Pool<Postgres>,
    s3_client: aws_sdk_s3::Client,
    external_cache: ExternalClassCache,
}

impl BackgroundRetryService {
    pub fn new(
        db_pool: Pool<Postgres>,
        s3_client: aws_sdk_s3::Client,
        external_cache: ExternalClassCache,
    ) -> Self {
        let max_compilations: usize = std::env::var("MAX_BACKGROUND_COMPILATIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        info!(
            "BackgroundRetryService initialized with max_compilations={}",
            max_compilations
        );

        Self {
            semaphore: Arc::new(Semaphore::new(max_compilations)),
            in_flight: Arc::new(RwLock::new(HashSet::new())),
            db_pool,
            s3_client,
            external_cache,
        }
    }

    /// Enqueue a background retry for a timed-out compilation.
    /// No-op if a retry is already in flight for this class hash.
    pub fn enqueue_retry(&self, class_hash: String, source_response: VoyagerSourceResponse) {
        let svc = self.clone();

        tokio::spawn(async move {
            {
                let mut in_flight = svc.in_flight.write().await;
                if !in_flight.insert(class_hash.clone()) {
                    info!(
                        "Background retry already in flight for {}, skipping",
                        class_hash
                    );
                    return;
                }
            }

            info!(
                "Background retry enqueued for {}, waiting for semaphore permit",
                class_hash
            );

            let _permit = match svc.semaphore.acquire().await {
                Ok(permit) => permit,
                Err(_) => {
                    warn!(
                        "Background retry semaphore closed for {}, aborting",
                        class_hash
                    );
                    svc.in_flight.write().await.remove(&class_hash);
                    return;
                }
            };

            info!("Background retry starting compilation for {}", class_hash);

            // No tokio timeout — only the OS CPU limit (BUILD_CPU_LIMIT) bounds the build.
            match compile_voyager_source(source_response, None).await {
                Ok(compiled) => {
                    info!(
                        "Background retry compilation succeeded for {} (inline_hash={})",
                        class_hash, compiled.inline_class_hash
                    );

                    let inline_persist = persist_class(
                        &svc.s3_client,
                        &svc.db_pool,
                        &compiled.inline_class_hash,
                        &compiled.contract_class,
                        &compiled.source_code,
                        true,
                    );
                    match &compiled.original_contract_class {
                        Some(original) => {
                            let original_persist = persist_class(
                                &svc.s3_client,
                                &svc.db_pool,
                                &compiled.original_class_hash,
                                original,
                                &compiled.source_code,
                                false,
                            );
                            tokio::join!(inline_persist, original_persist);
                        }
                        None => inline_persist.await,
                    }

                    // Use `set`, not `update_inline_data` — Phase 1 failure leaves no cache entry.
                    let class_debugger_data = extract_debugger_data_from_contract_class(
                        &compiled.contract_class,
                        &compiled.source_code,
                    );
                    let inline_hash = compiled.inline_class_hash.clone();
                    let cache_data = ClassDebuggerDataWithContractClass {
                        inline_strategy_class_hash: Some(compiled.inline_class_hash),
                        class_debugger_data,
                        contract_class: compiled.contract_class,
                    };
                    svc.external_cache
                        .set(
                            &class_hash,
                            Some(cache_data),
                            compiled.original_contract_class,
                            Some(inline_hash),
                            "background-retry",
                        )
                        .await;
                    svc.external_cache.clear_failed(&class_hash).await;

                    info!(
                        "Background retry: persisted {} to DB+S3 successfully",
                        class_hash
                    );
                }
                Err(e) => {
                    warn!(
                        "Background retry compilation failed for {}: {:?} (no further retries)",
                        class_hash, e
                    );
                    // The longer retry also failed → drop the partial Phase 1 cache entry
                    // and mark failed so simple-trace stops re-fetching from Voyager
                    // for the failed TTL window.
                    svc.external_cache.invalidate(&class_hash).await;
                    svc.external_cache.mark_failed(&class_hash).await;
                }
            }

            svc.in_flight.write().await.remove(&class_hash);
        });
    }
}

/// Upload the class to S3 and insert the DB row in parallel. Errors on either
/// side are logged, not propagated — partial persistence is acceptable since
/// the next request will re-attempt the missing piece.
async fn persist_class(
    s3_client: &aws_sdk_s3::Client,
    db_pool: &Pool<Postgres>,
    class_hash: &str,
    contract_class: &ContractClass,
    source_code: &HashMap<String, String>,
    is_cairo_debug_info: bool,
) {
    let s3_fut = upload_class_to_s3(s3_client, class_hash, contract_class, &None, source_code);
    let db_fut = insert_contract_class(
        db_pool,
        class_hash,
        true,
        is_cairo_debug_info,
        true,
        None,
    );
    let (s3_res, db_res) = tokio::join!(s3_fut, db_fut);
    if let Err(e) = s3_res {
        warn!(
            "Background retry: failed to upload class {} to S3: {:?}",
            class_hash, e
        );
    }
    if let Err(e) = db_res {
        warn!(
            "Background retry: failed to insert class {} in DB: {:?}",
            class_hash, e
        );
    }
}
