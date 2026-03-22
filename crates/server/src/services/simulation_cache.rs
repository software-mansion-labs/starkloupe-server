use moka::future::Cache;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use starknet_api::block::BlockNumber;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use simulate::{DebuggerInfo, SimulationArgs, TransactionSimulationResult};
use sqlx::Pool;
use sqlx::Postgres;

/// Extracts all class_hashes from a TransactionSimulationResult
fn extract_class_hashes_from_result(result: &TransactionSimulationResult) -> Vec<String> {
    let mut class_hashes = Vec::new();

    if let Some(l2_data) = &result.l2_transaction_data {
        class_hashes.extend(
            l2_data
                .simulation_result
                .contract_calls_map
                .collect_all_class_hashes(),
        );
    }

    // L1TransactionData doesn't contain simulation result with class_hashes

    class_hashes
}

/// Extracts all class_hashes from a DebuggerInfo
fn extract_class_hashes_from_debugger_info(debug_info: &DebuggerInfo) -> Vec<String> {
    debug_info.contract_calls_map.collect_all_class_hashes()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheKey {
    pub hash: String,
}

impl CacheKey {
    // Helper to get a display-friendly identifier from the cache key
    pub fn display_id(&self) -> String {
        if self.hash.len() >= 8 {
            if self.hash.starts_with("DEBUG") {
                format!("[DEBUG:{}]", &self.hash[6..14.min(self.hash.len())])
            } else {
                format!("[{}]", &self.hash[..8])
            }
        } else {
            format!("[{}]", &self.hash)
        }
    }

    /// Hash simulation args into a cache key string. An optional prefix differentiates
    /// cache key types (e.g. "DEBUG:" for debug simulation keys).
    fn hash_simulation_args(
        prefix: Option<&[u8]>,
        args: &SimulationArgs,
        resolved_block_number: Option<&BlockNumber>,
    ) -> String {
        let mut hasher = Sha256::new();

        if let Some(prefix) = prefix {
            hasher.update(prefix);
        }

        hasher.update(args.sender_address.to_bytes_be());
        hasher.update(
            &args
                .calldata
                .0
                .iter()
                .flat_map(|f| f.to_bytes_be())
                .collect::<Vec<_>>(),
        );
        hasher.update(&args.transaction_version.0.to_bytes_be());
        hasher.update(args.chain_id.to_string().as_bytes());

        // Include resolved block_number to prevent incorrect cache hits
        // This ensures that "Latest" requests get different cache keys based on actual block number
        if let Some(block_number) = &resolved_block_number {
            hasher.update(block_number.0.to_be_bytes());
        }

        // Include rpc_url for custom RPC endpoints
        hasher.update(args.rpc_url.as_str().as_bytes());

        if let Some(entry_point) = args.entry_point_selector {
            hasher.update(entry_point.0.to_bytes_be());
        }

        if let Some(nonce) = &args.nonce {
            hasher.update(nonce.0.to_bytes_be());
        }

        if let Some(max_fee) = &args.max_fee {
            hasher.update(max_fee.0.to_be_bytes());
        }

        if let Some(transaction_type) = &args.transaction_type {
            hasher.update(format!("{:?}", transaction_type).as_bytes());
        }

        if let Some(resource_bounds) = &args.resource_bounds {
            hasher.update(format!("{:?}", resource_bounds).as_bytes());
        }

        if let Some(paymaster_data) = &args.paymaster_data {
            hasher.update(
                paymaster_data
                    .0
                    .iter()
                    .flat_map(|f| f.to_bytes_be())
                    .collect::<Vec<_>>(),
            );
        }

        format!("{:x}", hasher.finalize())
    }

    pub fn from_simulation_args_with_block_number(
        args: &SimulationArgs,
        resolved_block_number: Option<&BlockNumber>,
    ) -> Self {
        Self {
            hash: Self::hash_simulation_args(None, args, resolved_block_number),
        }
    }

    pub fn from_tx_hash(tx_hash: &str, chain_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(tx_hash.as_bytes());
        hasher.update(chain_id.as_bytes());

        let hash = format!("{:x}", hasher.finalize());
        Self { hash }
    }

    pub fn from_debug_args_with_block_number(
        args: &SimulationArgs,
        resolved_block_number: Option<&BlockNumber>,
    ) -> Self {
        Self {
            hash: Self::hash_simulation_args(Some(b"DEBUG:"), args, resolved_block_number),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CachedResult {
    Simulation(Arc<TransactionSimulationResult>),
    Debug(Arc<DebuggerInfo>),
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub result: CachedResult,
    pub cached_at: std::time::Instant,
    pub class_hashes: Vec<String>,
    pub verified_class_hashes_at_cache_time: Vec<String>,
}

impl CacheEntry {
    pub fn new_simulation(
        result: Arc<TransactionSimulationResult>,
        class_hashes: Vec<String>,
        verified_class_hashes: Vec<String>,
    ) -> Self {
        Self {
            result: CachedResult::Simulation(result),
            cached_at: std::time::Instant::now(),
            class_hashes,
            verified_class_hashes_at_cache_time: verified_class_hashes,
        }
    }

    pub fn new_debug(
        result: Arc<DebuggerInfo>,
        class_hashes: Vec<String>,
        verified_class_hashes: Vec<String>,
    ) -> Self {
        Self {
            result: CachedResult::Debug(result),
            cached_at: std::time::Instant::now(),
            class_hashes,
            verified_class_hashes_at_cache_time: verified_class_hashes,
        }
    }

    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}

#[derive(Clone)]
pub struct SimulationCache {
    cache: Cache<String, CacheEntry>,
    ttl: Duration,
    capacity: u64,
    hits: Arc<RwLock<u64>>,
    misses: Arc<RwLock<u64>>,
    operations_count: Arc<RwLock<u64>>,
    last_entry_count: Arc<RwLock<u64>>,
}

impl SimulationCache {
    pub fn new(max_capacity: u64, ttl_minutes: u64) -> Self {
        let ttl = Duration::from_secs(ttl_minutes * 60);

        // Single unified cache for both simulation and debug results
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();

        info!(
            "Initialized cache with capacity: {}, TTL: {}min",
            max_capacity, ttl_minutes
        );

        Self {
            cache,
            ttl,
            capacity: max_capacity,
            hits: Arc::new(RwLock::new(0)),
            misses: Arc::new(RwLock::new(0)),
            operations_count: Arc::new(RwLock::new(0)),
            last_entry_count: Arc::new(RwLock::new(0)),
        }
    }

    async fn log_stats_if_needed(&self) {
        let mut ops_count = self.operations_count.write().await;
        *ops_count += 1;

        // Check for automatic cleanup (LRU eviction or TTL expiration)
        // Only check after cache modifying operations (get, set), not on every call
        let current_entries = self.cache.entry_count();

        let mut last_count = self.last_entry_count.write().await;

        // Only log cleanup if we had a significant drop (not just normal set/get fluctuations)
        if current_entries < *last_count && (*last_count - current_entries) > 0 {
            let cleaned = *last_count - current_entries;
            // Only log if we actually cleaned more than expected (not just normal operations)
            if cleaned > 1 || current_entries == 0 {
                info!(
                    "[CACHE CLEAN] {} entries removed (LRU eviction or TTL expiration)",
                    cleaned
                );
            }
        }

        *last_count = current_entries;

        // Log stats every 10 operations (ops = cache operations: get, set, invalidate, clear)
        if *ops_count % 10 == 0 {
            let total_entries = self.cache.entry_count();
            let stats = self.stats().await;
            info!("[CACHE STATS] After {} ops: {} hits, {} misses, {:.1}% hit rate, {} entries (capacity: {})",
                *ops_count, stats.hits, stats.misses, stats.hit_rate, total_entries, self.capacity);
        }
    }

    async fn record_hit(&self) {
        *self.hits.write().await += 1;
        self.log_stats_if_needed().await;
    }

    async fn record_miss(&self) {
        *self.misses.write().await += 1;
        self.log_stats_if_needed().await;
    }

    /// Shared cache lookup logic: checks expiry, verification invalidation, and updates stats.
    /// Returns the validated `CacheEntry` on a hit, or `None` on a miss.
    async fn get_entry(
        &self,
        key: &CacheKey,
        db_pool: Option<&Pool<Postgres>>,
        label: &str,
    ) -> Option<CacheEntry> {
        let cached = match self.cache.get(&key.hash).await {
            Some(cached) => cached,
            None => {
                self.record_miss().await;
                return None;
            }
        };

        if cached.is_expired(self.ttl) {
            info!("[CACHE EXPIRED] Invalidating {}", key.display_id());
            self.invalidate(key).await;
            self.record_miss().await;
            return None;
        }

        if let Some(pool) = db_pool {
            if self
                .should_invalidate_due_to_verification(&cached, pool)
                .await
            {
                info!(
                    "[CACHE INVALIDATE] Classes verified after cache creation {}",
                    key.display_id()
                );
                self.invalidate(key).await;
                self.record_miss().await;
                return None;
            }
        }

        info!("[CACHE HIT] {} {}", label, key.display_id());
        self.record_hit().await;
        Some(cached)
    }

    pub async fn get(
        &self,
        key: &CacheKey,
        db_pool: Option<&Pool<Postgres>>,
    ) -> Option<Arc<TransactionSimulationResult>> {
        let entry = self.get_entry(key, db_pool, "simulation").await?;
        match &entry.result {
            CachedResult::Simulation(result) => Some(Arc::clone(result)),
            CachedResult::Debug(_) => {
                warn!(
                    "[CACHE ERROR] tried to get simulation but found debug {}",
                    key.display_id()
                );
                None
            }
        }
    }

    async fn should_invalidate_due_to_verification(
        &self,
        cached: &CacheEntry,
        db_pool: &Pool<Postgres>,
    ) -> bool {
        if cached.class_hashes.is_empty() {
            return false;
        }

        // Check which class_hashes are currently verified
        match verification::db::fetch_verified_classes(db_pool, &cached.class_hashes).await {
            Ok(verified_classes) => {
                let currently_verified: std::collections::HashSet<String> =
                    verified_classes.into_iter().map(|c| c.hash).collect();

                let verified_at_cache_time: std::collections::HashSet<String> = cached
                    .verified_class_hashes_at_cache_time
                    .iter()
                    .cloned()
                    .collect();

                // Check if there are any NEWLY verified classes (verified now but not at cache time)
                let newly_verified: Vec<_> = currently_verified
                    .difference(&verified_at_cache_time)
                    .collect();

                if !newly_verified.is_empty() {
                    debug!(
                        "[CACHE] Found {} newly verified classes: {:?}",
                        newly_verified.len(),
                        newly_verified
                    );
                    return true;
                }

                false
            }
            Err(e) => {
                warn!(
                    "Failed to check verification status for cache invalidation: {:?}",
                    e
                );
                false
            }
        }
    }

    /// Fetch the currently verified class hashes from the database.
    async fn fetch_verified_class_hashes(
        db_pool: Option<&Pool<Postgres>>,
        class_hashes: &[String],
    ) -> Vec<String> {
        let Some(pool) = db_pool else {
            return Vec::new();
        };
        match verification::db::fetch_verified_classes(pool, class_hashes).await {
            Ok(verified_classes) => verified_classes.into_iter().map(|c| c.hash).collect(),
            Err(e) => {
                warn!("Failed to fetch verified classes for cache: {:?}", e);
                Vec::new()
            }
        }
    }

    pub async fn set(
        &self,
        key: &CacheKey,
        result: Arc<TransactionSimulationResult>,
        db_pool: Option<&Pool<Postgres>>,
    ) {
        let class_hashes = extract_class_hashes_from_result(&result);
        let verified_class_hashes = Self::fetch_verified_class_hashes(db_pool, &class_hashes).await;
        let cached_entry = CacheEntry::new_simulation(result, class_hashes, verified_class_hashes);
        self.cache.insert(key.hash.clone(), cached_entry).await;
        debug!("[CACHE SET] simulation {}", key.display_id());
        self.log_stats_if_needed().await;
    }

    pub async fn get_debug(
        &self,
        key: &CacheKey,
        db_pool: Option<&Pool<Postgres>>,
    ) -> Option<Arc<DebuggerInfo>> {
        let entry = self.get_entry(key, db_pool, "debug").await?;
        match &entry.result {
            CachedResult::Debug(result) => Some(Arc::clone(result)),
            CachedResult::Simulation(_) => {
                warn!(
                    "[CACHE ERROR] tried to get debug but found simulation {}",
                    key.display_id()
                );
                None
            }
        }
    }

    pub async fn set_debug(
        &self,
        key: &CacheKey,
        result: Arc<DebuggerInfo>,
        db_pool: Option<&Pool<Postgres>>,
    ) {
        let class_hashes = extract_class_hashes_from_debugger_info(&result);
        let verified_class_hashes = Self::fetch_verified_class_hashes(db_pool, &class_hashes).await;
        let cached_entry = CacheEntry::new_debug(result, class_hashes, verified_class_hashes);
        self.cache.insert(key.hash.clone(), cached_entry).await;
        debug!("[CACHE SET] debug {}", key.display_id());
        self.log_stats_if_needed().await;
    }

    pub async fn invalidate(&self, key: &CacheKey) {
        self.cache.invalidate(&key.hash).await;
        info!("[CACHE INVALIDATE] {}", key.display_id());
    }

    pub async fn stats(&self) -> CacheStats {
        let hits = *self.hits.read().await;
        let misses = *self.misses.read().await;
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        // Count simulation vs debug entries by iterating through cache
        // This is expensive but only called occasionally
        let (sim_count, debug_count) = self.count_entry_types().await;

        CacheStats {
            hits,
            misses,
            total_requests: total,
            hit_rate,
            cached_entries: sim_count,
            debug_cached_entries: debug_count,
            capacity: self.capacity,
            sim_capacity: self.capacity, // All capacity is shared now
            debug_capacity: self.capacity,
        }
    }

    async fn count_entry_types(&self) -> (u64, u64) {
        // This is a simple approximation - we'll estimate based on key prefixes
        // since moka doesn't provide iterator access to cache contents
        let total_entries = self.cache.entry_count();

        // For now, we can't efficiently iterate through moka cache entries
        // So we'll just return total count and 0 for debug
        // TODO: Consider switching to a different cache if we need this granularity
        (total_entries, 0)
    }
}

#[derive(Debug, Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub total_requests: u64,
    pub hit_rate: f64,
    pub cached_entries: u64,
    pub debug_cached_entries: u64,
    pub capacity: u64,
    pub sim_capacity: u64,
    pub debug_capacity: u64,
}

// Default configuration
impl Default for SimulationCache {
    fn default() -> Self {
        // Production: 100 entries, 24 hours TTL
        Self::new(100, 1440)
    }
}
