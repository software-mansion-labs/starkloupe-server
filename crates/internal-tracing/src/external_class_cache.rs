use crate::ClassDebuggerDataWithContractClass;
use moka::future::Cache;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Cached entry for externally compiled class data
#[derive(Debug, Clone)]
pub struct CachedExternalClass {
    pub data: ClassDebuggerDataWithContractClass,
    pub cached_at: Instant,
    pub source: String,
}

/// Cache for externally compiled class data (e.g., from Voyager API)
///
/// This cache stores compiled debug data for classes that are not verified
/// in the Walnut database but were found on external verification services.
#[derive(Clone)]
pub struct ExternalClassCache {
    cache: Cache<String, Arc<CachedExternalClass>>,
    ttl: Duration,
    capacity: u64,
}

impl ExternalClassCache {
    /// Create a new cache with the given capacity and TTL
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of entries to store
    /// * `ttl_hours` - Time-to-live in hours for cached entries
    pub fn new(capacity: u64, ttl_hours: u64) -> Self {
        let ttl = Duration::from_secs(ttl_hours * 3600);

        let cache = Cache::builder()
            .max_capacity(capacity)
            .time_to_live(ttl)
            .build();

        info!(
            "External class cache initialized with capacity={}, ttl={}h",
            capacity, ttl_hours
        );

        Self {
            cache,
            ttl,
            capacity,
        }
    }

    /// Create cache from environment variables
    ///
    /// Uses:
    /// - `EXTERNAL_CLASS_CACHE_CAPACITY` (default: 500)
    /// - `EXTERNAL_CLASS_CACHE_TTL_HOURS` (default: 24)
    pub fn from_env() -> Self {
        let capacity = std::env::var("EXTERNAL_CLASS_CACHE_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);

        let ttl_hours = std::env::var("EXTERNAL_CLASS_CACHE_TTL_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24);

        Self::new(capacity, ttl_hours)
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
    ///
    /// # Arguments
    /// * `class_hash` - The original class hash (cache key)
    /// * `data` - The compiled class data with debug info
    /// * `source` - The source of the data (e.g., "voyager")
    pub async fn set(
        &self,
        class_hash: &str,
        data: ClassDebuggerDataWithContractClass,
        source: &str,
    ) {
        let entry = CachedExternalClass {
            data,
            cached_at: Instant::now(),
            source: source.to_string(),
        };

        self.cache.insert(class_hash.to_string(), Arc::new(entry)).await;
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

    /// Get a cached entry or compute it if missing.
    ///
    /// This method ensures **in-flight deduplication**: if multiple concurrent
    /// requests try to compute the same class hash, only one computation runs
    /// and others wait for the result. This prevents duplicate compilations
    /// for the same class when multiple debug requests arrive simultaneously.
    pub async fn get_or_try_compute<F>(
        &self,
        class_hash: &str,
        init: F,
    ) -> Result<Arc<CachedExternalClass>, Arc<anyhow::Error>>
    where
        F: std::future::Future<Output = Result<Arc<CachedExternalClass>, anyhow::Error>>,
    {
        self.cache
            .try_get_with(class_hash.to_string(), init)
            .await
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
}

/// Statistics about the external class cache
#[derive(Debug, Clone)]
pub struct ExternalCacheStats {
    pub entry_count: u64,
    pub capacity: u64,
    pub ttl_hours: u64,
}
