use crate::ClassDebuggerDataWithContractClass;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};

/// Cached entry for externally compiled class data.
///
/// Compilation is split into two phases:
/// - Phase 1: non-inline (dev/release) build → sets `original_contract_class`
/// - Phase 2: inline (walnut-debug or existing profile) build → sets `data` + `inline_strategy_class_hash`
///
/// If the matching profile already had inline avoid strategy, both are set at once (no Phase 2).
#[derive(Debug, Clone)]
pub struct CachedExternalClass {
    /// Inline class debug data (with coverage annotations).
    /// None until Phase 2 completes (or until both phases complete at once).
    pub data: Option<ClassDebuggerDataWithContractClass>,
    /// Inline class hash. None until Phase 2 completes.
    pub inline_strategy_class_hash: Option<String>,
    /// Non-inline compiled class whose CASM matches the original on-chain class.
    /// Set after Phase 1 completes.
    pub original_contract_class: Option<ContractClass>,
    pub cached_at: Instant,
    pub source: String,
}

type CompilationNotifier = watch::Sender<bool>;

/// Cache for externally compiled class data (e.g., from Voyager API).
///
/// Simple trace waits for `phase1_ready` (with 45s timeout) to get function calls.
/// Debug trace waits for `pending_compilations` (full Phase 2 completion) to get inline debug data.
#[derive(Debug, Clone)]
pub struct ExternalClassCache {
    cache: Cache<String, Arc<CachedExternalClass>>,
    /// Phase 2 (full/inline) completion signal. Removed when Phase 2 finishes.
    pending_compilations: Arc<RwLock<HashMap<String, watch::Receiver<bool>>>>,
    /// Phase 1 (non-inline) completion signal. Removed when Phase 1 finishes.
    phase1_ready: Arc<RwLock<HashMap<String, watch::Receiver<bool>>>>,
    /// Classes whose compilation has failed (prevents retries until TTL expires).
    failed_compilations: Cache<String, Instant>,
    ttl: Duration,
    failed_ttl: Duration,
    capacity: u64,
}

impl ExternalClassCache {
    pub fn new(capacity: u64, ttl_hours: u64, failed_ttl_hours: u64) -> Self {
        let ttl = Duration::from_secs(ttl_hours * 3600);
        let failed_ttl = Duration::from_secs(failed_ttl_hours * 3600);

        let cache = Cache::builder()
            .max_capacity(capacity)
            .time_to_live(ttl)
            .build();

        let failed_compilations = Cache::builder()
            .max_capacity(capacity)
            .time_to_live(failed_ttl)
            .build();

        info!(
            "External class cache initialized with capacity={}, ttl={}h, failed_ttl={}h",
            capacity, ttl_hours, failed_ttl_hours
        );

        Self {
            cache,
            pending_compilations: Arc::new(RwLock::new(HashMap::new())),
            phase1_ready: Arc::new(RwLock::new(HashMap::new())),
            failed_compilations,
            ttl,
            failed_ttl,
            capacity,
        }
    }

    pub fn from_env() -> Self {
        let capacity = std::env::var("EXTERNAL_CLASS_CACHE_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);

        let ttl_hours = std::env::var("EXTERNAL_CLASS_CACHE_TTL_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24);

        let failed_ttl_hours = std::env::var("EXTERNAL_CLASS_CACHE_FAILED_TTL_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        Self::new(capacity, ttl_hours, failed_ttl_hours)
    }

    /// Get a cached entry by class hash.
    pub async fn get(&self, class_hash: &str) -> Option<Arc<CachedExternalClass>> {
        let entry = self.cache.get(class_hash).await?;

        if entry.cached_at.elapsed() > self.ttl {
            debug!("External cache entry expired for {}", class_hash);
            self.cache.invalidate(class_hash).await;
            return None;
        }

        debug!("External cache hit for {}", class_hash);
        Some(entry)
    }

    /// Store a (possibly partial) compiled class in the cache.
    /// `data` is None until Phase 2 completes.
    pub async fn set(
        &self,
        class_hash: &str,
        data: Option<ClassDebuggerDataWithContractClass>,
        original_contract_class: Option<ContractClass>,
        inline_strategy_class_hash: Option<String>,
        source: &str,
    ) {
        let entry = CachedExternalClass {
            data,
            inline_strategy_class_hash,
            original_contract_class,
            cached_at: Instant::now(),
            source: source.to_string(),
        };
        self.cache
            .insert(class_hash.to_string(), Arc::new(entry))
            .await;
        debug!("External cache set for {} (source: {})", class_hash, source);
    }

    /// Update an existing cache entry with Phase 2 (inline) data.
    /// Called after the walnut-debug (or equivalent) build completes.
    pub async fn update_inline_data(
        &self,
        class_hash: &str,
        data: ClassDebuggerDataWithContractClass,
        inline_strategy_class_hash: Option<String>,
    ) {
        if let Some(existing) = self.cache.get(class_hash).await {
            let updated = CachedExternalClass {
                data: Some(data),
                inline_strategy_class_hash,
                original_contract_class: existing.original_contract_class.clone(),
                cached_at: existing.cached_at,
                source: format!("{} + voyager-inline", existing.source),
            };
            self.cache
                .insert(class_hash.to_string(), Arc::new(updated))
                .await;
            debug!("Updated inline data for {}", class_hash);
        } else {
            warn!(
                "Cannot update inline data for {} - not in cache",
                class_hash
            );
        }
    }

    /// Check if a class hash is in the cache (even if `data` is not yet populated).
    pub async fn contains(&self, class_hash: &str) -> bool {
        self.cache.contains_key(class_hash)
    }

    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    pub fn stats(&self) -> ExternalCacheStats {
        ExternalCacheStats {
            entry_count: self.cache.entry_count(),
            capacity: self.capacity,
            ttl_hours: self.ttl.as_secs() / 3600,
        }
    }

    pub async fn invalidate(&self, class_hash: &str) {
        self.cache.invalidate(class_hash).await;
        debug!("External cache invalidated for {}", class_hash);
    }

    pub async fn clear(&self) {
        self.cache.invalidate_all();
        info!("External class cache cleared");
    }

    /// Start tracking a two-phase compilation.
    ///
    /// Returns `Some((phase1_notifier, phase2_notifier))` if tracking started,
    /// or `None` if another compilation is already in progress for this class.
    ///
    /// - `phase1_notifier`: call `signal_phase1_ready` after Phase 1 (non-inline build)
    /// - `phase2_notifier`: call `finish_compilation` after Phase 2 (inline build)
    pub async fn start_compilation(
        &self,
        class_hash: &str,
    ) -> Option<(CompilationNotifier, CompilationNotifier)> {
        let mut pending = self.pending_compilations.write().await;

        if pending.contains_key(class_hash) {
            info!(
                "Compilation already in progress for {}, skipping",
                class_hash
            );
            return None;
        }

        let (phase1_tx, phase1_rx) = watch::channel(false);
        let (phase2_tx, phase2_rx) = watch::channel(false);

        {
            let mut phase1 = self.phase1_ready.write().await;
            phase1.insert(class_hash.to_string(), phase1_rx);
        }
        pending.insert(class_hash.to_string(), phase2_rx);

        debug!("Started two-phase compilation tracking for {}", class_hash);
        Some((phase1_tx, phase2_tx))
    }

    /// Signal that Phase 1 (non-inline build) is complete.
    /// Unblocks callers of `wait_for_phase1_ready`.
    pub async fn signal_phase1_ready(&self, class_hash: &str, notifier: CompilationNotifier) {
        let _ = notifier.send(true);
        let mut phase1 = self.phase1_ready.write().await;
        phase1.remove(class_hash);
        debug!("Phase 1 ready signaled for {}", class_hash);
    }

    /// Wait for Phase 1 (non-inline build) to complete.
    /// Returns immediately if Phase 1 is not pending.
    pub async fn wait_for_phase1_ready(&self, class_hash: &str) -> bool {
        let receiver = {
            let phase1 = self.phase1_ready.read().await;
            phase1.get(class_hash).cloned()
        };

        if let Some(mut rx) = receiver {
            debug!("Waiting for phase 1 completion of {}", class_hash);
            loop {
                if *rx.borrow() {
                    return true;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        }

        false
    }

    /// Check if Phase 1 (non-inline build) is still in progress.
    pub async fn is_phase1_pending(&self, class_hash: &str) -> bool {
        let phase1 = self.phase1_ready.read().await;
        phase1.contains_key(class_hash)
    }

    /// Wait for the full compilation (Phase 2 / inline build) to complete.
    pub async fn wait_for_pending(&self, class_hash: &str) -> bool {
        let receiver = {
            let pending = self.pending_compilations.read().await;
            pending.get(class_hash).cloned()
        };

        if let Some(mut rx) = receiver {
            info!("Waiting for full compilation of {}", class_hash);
            loop {
                if *rx.borrow() {
                    return true;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        }

        false
    }

    /// Check if Phase 2 (full/inline compilation) is still in progress.
    pub async fn is_compiling(&self, class_hash: &str) -> bool {
        let pending = self.pending_compilations.read().await;
        pending.contains_key(class_hash)
    }

    /// Signal that Phase 2 (inline build) is complete.
    /// Unblocks callers of `wait_for_pending`.
    pub async fn finish_compilation(&self, class_hash: &str, notifier: CompilationNotifier) {
        let _ = notifier.send(true);
        let mut pending = self.pending_compilations.write().await;
        pending.remove(class_hash);
        debug!("Full compilation finished for {}", class_hash);
    }

    pub async fn mark_failed(&self, class_hash: &str) {
        self.failed_compilations
            .insert(class_hash.to_string(), Instant::now())
            .await;
        info!(
            "Marked compilation as failed for {} (will retry after {:?})",
            class_hash, self.failed_ttl
        );
    }

    pub async fn has_failed(&self, class_hash: &str) -> bool {
        self.failed_compilations.contains_key(class_hash)
    }

    pub async fn clear_failed(&self, class_hash: &str) {
        self.failed_compilations.invalidate(class_hash).await;
        debug!("Cleared failed status for {}", class_hash);
    }
}

#[derive(Debug, Clone)]
pub struct ExternalCacheStats {
    pub entry_count: u64,
    pub capacity: u64,
    pub ttl_hours: u64,
}
