use crate::external_class_cache::ExternalClassCache;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;
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
            // Dedup: check-and-insert atomically under write lock
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

            // Acquire semaphore permit (blocks at capacity)
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

            // Compile with no tokio timeout (None) — only OS CPU limit applies
            match compile_voyager_source(source_response, None).await {
                Ok(compiled) => {
                    info!(
                        "Background retry compilation succeeded for {} (inline_hash={})",
                        class_hash, compiled.inline_class_hash
                    );

                    // Persist inline class to S3 + DB
                    if let Err(e) = upload_class_to_s3(
                        &svc.s3_client,
                        &compiled.inline_class_hash,
                        &compiled.contract_class,
                        &None,
                        &compiled.source_code,
                    )
                    .await
                    {
                        warn!(
                            "Background retry: failed to upload inline class {} to S3: {:?}",
                            compiled.inline_class_hash, e
                        );
                    }

                    if let Err(e) = insert_contract_class(
                        &svc.db_pool,
                        &compiled.inline_class_hash,
                        true,
                        true,
                        true,
                        None,
                    )
                    .await
                    {
                        warn!(
                            "Background retry: failed to insert inline class {} in DB: {:?}",
                            compiled.inline_class_hash, e
                        );
                    }

                    // Persist original class to S3 + DB if available
                    if let Some(ref original_contract_class) = compiled.original_contract_class {
                        if let Err(e) = upload_class_to_s3(
                            &svc.s3_client,
                            &compiled.original_class_hash,
                            original_contract_class,
                            &None,
                            &compiled.source_code,
                        )
                        .await
                        {
                            warn!(
                                "Background retry: failed to upload original class {} to S3: {:?}",
                                compiled.original_class_hash, e
                            );
                        }

                        if let Err(e) = insert_contract_class(
                            &svc.db_pool,
                            &compiled.original_class_hash,
                            true,
                            false,
                            true,
                            None,
                        )
                        .await
                        {
                            warn!(
                                "Background retry: failed to insert original class {} in DB: {:?}",
                                compiled.original_class_hash, e
                            );
                        }
                    }

                    // Clear the failed status so normal requests pick up the verified class
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
                }
            }

            // Always remove from in-flight set
            svc.in_flight.write().await.remove(&class_hash);
        });
    }
}
