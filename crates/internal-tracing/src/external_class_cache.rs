use crate::ClassDebuggerDataWithContractClass;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, RwLock};
use tracing::{debug, info};

/// Cached entry for externally compiled class data
#[derive(Debug, Clone)]
pub struct CachedExternalClass {
    pub data: ClassDebuggerDataWithContractClass,
    /// Non-inline compiled class with debug info (CASM matches original).
    /// Used for simple trace function calls.
    pub original_contract_class: Option<ContractClass>,
    pub cached_at: Instant,
    pub source: String,
}

/// Sender for notifying when a compilation completes
type CompilationNotifier = watch::Sender<bool>;

/// Cache for externally compiled class data (e.g., from Voyager API)
///
/// This cache stores compiled debug data for classes that are not verified
/// in the Walnut database but were found on external verification services.
#[derive(Debug, Clone)]
pub struct ExternalClassCache {
    cache: Cache<String, Arc<CachedExternalClass>>,
    /// Tracks classes that are currently being compiled
    /// When a compilation starts, we insert a watch channel
    /// When it finishes, we send a signal and remove the entry
    pending_compilations: Arc<RwLock<HashMap<String, watch::Receiver<bool>>>>,
    /// Tracks classes whose compilation has failed
    /// This prevents repeated compilation attempts for classes that are known to fail
    failed_compilations: Cache<String, Instant>,
    ttl: Duration,
    failed_ttl: Duration,
    capacity: u64,
}

impl ExternalClassCache {
    /// Create a new cache with the given capacity and TTL
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of entries to store
    /// * `ttl_hours` - Time-to-live in hours for cached entries
    /// * `failed_ttl_hours` - Time-to-live in hours for failed compilation entries
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
            failed_compilations,
            ttl,
            failed_ttl,
            capacity,
        }
    }

    /// Create cache from environment variables
    ///
    /// Uses:
    /// - `EXTERNAL_CLASS_CACHE_CAPACITY` (default: 500)
    /// - `EXTERNAL_CLASS_CACHE_TTL_HOURS` (default: 24)
    /// - `EXTERNAL_CLASS_CACHE_FAILED_TTL_HOURS` (default: 1)
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

    /// Get a cached entry by class hash
    ///
    /// Returns None if:
    /// - The entry doesn't exist
    /// - The entry has expired (TTL)
    pub async fn get(&self, class_hash: &str) -> Option<Arc<CachedExternalClass>> {
        let entry = self.cache.get(class_hash).await?;

        // Check if entry is expired (moka handles this, but double-check)
        if entry.cached_at.elapsed() > self.ttl {
            debug!("External cache entry expired for {}", class_hash);
            self.cache.invalidate(class_hash).await;
            return None;
        }

        debug!("External cache hit for {}", class_hash);
        Some(entry)
    }

    /// Store a compiled class in the cache
    pub async fn set(
        &self,
        class_hash: &str,
        data: ClassDebuggerDataWithContractClass,
        original_contract_class: Option<ContractClass>,
        source: &str,
    ) {
        let entry = CachedExternalClass {
            data,
            original_contract_class,
            cached_at: Instant::now(),
            source: source.to_string(),
        };

        self.cache
            .insert(class_hash.to_string(), Arc::new(entry))
            .await;
        debug!("External cache set for {} (source: {})", class_hash, source);
    }

    /// Check if a class hash is in the cache
    pub async fn contains(&self, class_hash: &str) -> bool {
        self.cache.contains_key(class_hash)
    }

    /// Get the current number of entries in the cache
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Get cache statistics
    pub fn stats(&self) -> ExternalCacheStats {
        ExternalCacheStats {
            entry_count: self.cache.entry_count(),
            capacity: self.capacity,
            ttl_hours: self.ttl.as_secs() / 3600,
        }
    }

    /// Invalidate a specific cache entry
    pub async fn invalidate(&self, class_hash: &str) {
        self.cache.invalidate(class_hash).await;
        debug!("External cache invalidated for {}", class_hash);
    }

    /// Clear all entries from the cache
    pub async fn clear(&self) {
        self.cache.invalidate_all();
        info!("External class cache cleared");
    }

    /// Mark a class as being compiled. Returns a notifier to signal when done.
    /// If another compilation is already in progress, returns None.
    pub async fn start_compilation(&self, class_hash: &str) -> Option<CompilationNotifier> {
        let mut pending = self.pending_compilations.write().await;

        // Check if already being compiled
        if pending.contains_key(class_hash) {
            info!(
                "Compilation already in progress for {}, skipping",
                class_hash
            );
            return None;
        }

        // Create a watch channel - initial value is false (not done)
        let (tx, rx) = watch::channel(false);
        pending.insert(class_hash.to_string(), rx);
        debug!("Started compilation tracking for {}", class_hash);
        Some(tx)
    }

    /// Wait for a pending compilation to complete.
    /// Returns true if there was a pending compilation and it completed.
    /// Returns false if no compilation was pending.
    pub async fn wait_for_pending(&self, class_hash: &str) -> bool {
        // Get a clone of the receiver if compilation is pending
        let receiver = {
            let pending = self.pending_compilations.read().await;
            pending.get(class_hash).cloned()
        };

        if let Some(mut rx) = receiver {
            info!("Waiting for pending compilation of {}", class_hash);
            // Wait for the compilation to complete (value becomes true)
            loop {
                if *rx.borrow() {
                    return true;
                }
                if rx.changed().await.is_err() {
                    // Sender was dropped, compilation might have failed
                    return false;
                }
            }
        }

        false
    }

    /// Check if a class is currently being compiled
    pub async fn is_compiling(&self, class_hash: &str) -> bool {
        let pending = self.pending_compilations.read().await;
        pending.contains_key(class_hash)
    }

    /// Mark a compilation as finished and remove from pending.
    /// This should be called after the compilation result is cached.
    pub async fn finish_compilation(&self, class_hash: &str, notifier: CompilationNotifier) {
        // Signal completion
        let _ = notifier.send(true);

        // Remove from pending
        let mut pending = self.pending_compilations.write().await;
        pending.remove(class_hash);
    }

    /// Mark a compilation as failed.
    /// This prevents repeated compilation attempts for the same class.
    /// The failed status will be cached for `failed_ttl` duration.
    pub async fn mark_failed(&self, class_hash: &str) {
        self.failed_compilations
            .insert(class_hash.to_string(), Instant::now())
            .await;
        info!(
            "Marked compilation as failed for {} (will retry after {:?})",
            class_hash, self.failed_ttl
        );
    }

    /// Check if a compilation has previously failed for this class.
    /// Returns true if the class is in the failed cache (and TTL hasn't expired).
    pub async fn has_failed(&self, class_hash: &str) -> bool {
        self.failed_compilations.contains_key(class_hash)
    }

    /// Clear the failed status for a class (e.g., to force a retry).
    pub async fn clear_failed(&self, class_hash: &str) {
        self.failed_compilations.invalidate(class_hash).await;
        debug!("Cleared failed status for {}", class_hash);
    }
}

/// Statistics about the external class cache
#[derive(Debug, Clone)]
pub struct ExternalCacheStats {
    pub entry_count: u64,
    pub capacity: u64,
    pub ttl_hours: u64,
}
