use crate::debugger_data_fetcher::extract_debugger_data_from_contract_class;
use crate::external_class_cache::ExternalClassCache;
use crate::voyager_persistence::persist_compiled_voyager_class;
use crate::ClassDebuggerDataWithContractClass;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{info, warn};
use verification::voyager::{compile_voyager_source, VoyagerSourceResponse};

/// Manages background retry compilations for contracts that timed out during
/// inline Voyager compilation. On success, persists results to DB + GCS so
/// future transactions find verified code without re-compiling.
#[derive(Clone)]
pub struct BackgroundRetryService {
    semaphore: Arc<Semaphore>,
    /// Class hashes currently being retried (prevents duplicate retries).
    active_retries: Arc<RwLock<HashSet<String>>>,
    db_pool: Pool<Postgres>,
    gcs_client: google_cloud_storage::client::Storage,
    external_cache: ExternalClassCache,
}

impl BackgroundRetryService {
    pub fn new(
        db_pool: Pool<Postgres>,
        gcs_client: google_cloud_storage::client::Storage,
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
            active_retries: Arc::new(RwLock::new(HashSet::new())),
            db_pool,
            gcs_client,
            external_cache,
        }
    }

    /// Enqueue a background retry for a timed-out compilation.
    /// No-op if a retry is already in flight for this class hash.
    pub fn enqueue_retry(&self, class_hash: String, source_response: VoyagerSourceResponse) {
        let svc = self.clone();

        tokio::spawn(async move {
            {
                let mut active_retries = svc.active_retries.write().await;
                if !active_retries.insert(class_hash.clone()) {
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
                    svc.active_retries.write().await.remove(&class_hash);
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

                    persist_compiled_voyager_class(
                        &svc.gcs_client,
                        &svc.db_pool,
                        &compiled,
                        "background-retry",
                    )
                    .await;

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
                        "Background retry: persisted {} to DB+GCS successfully",
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

            svc.active_retries.write().await.remove(&class_hash);
        });
    }
}
