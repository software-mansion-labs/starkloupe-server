use moka::future::Cache;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use starknet_api::block::BlockNumber;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use simulate::{DebuggerInfo, SimulationArgs, TransactionSimulationResult};

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

    pub fn from_simulation_args_with_block_number(
        args: &SimulationArgs,
        resolved_block_number: Option<&BlockNumber>,
    ) -> Self {
        let mut hasher = Sha256::new();

        // Include key simulation parameters in hash
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

        // Include resolved block_number in cache key to prevent incorrect cache hits
        // This ensures that "Latest" requests get different cache keys based on actual block number
        if let Some(block_number) = &resolved_block_number {
            hasher.update(block_number.0.to_be_bytes());
        }

        // Include rpc_url in cache key for custom RPC endpoints
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

        // Include transaction_type in cache key for different transaction types
        if let Some(transaction_type) = &args.transaction_type {
            hasher.update(format!("{:?}", transaction_type).as_bytes());
        }

        // Include resource_bounds for v3 transactions
        if let Some(resource_bounds) = &args.resource_bounds {
            hasher.update(format!("{:?}", resource_bounds).as_bytes());
        }

        // Include paymaster_data for sponsored transactions
        if let Some(paymaster_data) = &args.paymaster_data {
            hasher.update(
                paymaster_data
                    .0
                    .iter()
                    .flat_map(|f| f.to_bytes_be())
                    .collect::<Vec<_>>(),
            );
        }

        let hash = format!("{:x}", hasher.finalize());
        Self { hash }
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
        let mut hasher = Sha256::new();

        // Include "DEBUG" prefix to differentiate from regular simulation cache
        hasher.update(b"DEBUG:");

        // Include same parameters as simulation args
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

        // Include resolved block_number in debug cache key to prevent incorrect cache hits
        if let Some(block_number) = &resolved_block_number {
            hasher.update(block_number.0.to_be_bytes());
        }

        // Include rpc_url in debug cache key for custom RPC endpoints
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

        // Include transaction_type in debug cache key for different transaction types
        if let Some(transaction_type) = &args.transaction_type {
            hasher.update(format!("{:?}", transaction_type).as_bytes());
        }

        // Include resource_bounds for v3 transactions in debug cache
        if let Some(resource_bounds) = &args.resource_bounds {
            hasher.update(format!("{:?}", resource_bounds).as_bytes());
        }

        // Include paymaster_data for sponsored transactions in debug cache
        if let Some(paymaster_data) = &args.paymaster_data {
            hasher.update(
                paymaster_data
                    .0
                    .iter()
                    .flat_map(|f| f.to_bytes_be())
                    .collect::<Vec<_>>(),
            );
        }

        let hash = format!("{:x}", hasher.finalize());
        Self { hash }
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
}

impl CacheEntry {
    pub fn new_simulation(result: Arc<TransactionSimulationResult>) -> Self {
        Self {
            result: CachedResult::Simulation(result),
            cached_at: std::time::Instant::now(),
        }
    }

    pub fn new_debug(result: Arc<DebuggerInfo>) -> Self {
        Self {
            result: CachedResult::Debug(result),
            cached_at: std::time::Instant::now(),
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

    pub async fn get(&self, key: &CacheKey) -> Option<Arc<TransactionSimulationResult>> {
        match self.cache.get(&key.hash).await {
            Some(cached) => {
                if cached.is_expired(self.ttl) {
                    info!("[CACHE EXPIRED] Invalidating {}", key.display_id());
                    self.invalidate(key).await;
                    let mut misses = self.misses.write().await;
                    *misses += 1;
                    drop(misses);
                    self.log_stats_if_needed().await;
                    None
                } else {
                    info!("[CACHE HIT] simulation {}", key.display_id());
                    let mut hits = self.hits.write().await;
                    *hits += 1;
                    drop(hits);
                    self.log_stats_if_needed().await;

                    match &cached.result {
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
            }
            None => {
                info!("[CACHE MISS] simulation {}", key.display_id());
                let mut misses = self.misses.write().await;
                *misses += 1;
                drop(misses);
                self.log_stats_if_needed().await;
                None
            }
        }
    }

    pub async fn set(&self, key: &CacheKey, result: Arc<TransactionSimulationResult>) {
        let cached_entry = CacheEntry::new_simulation(result);
        self.cache.insert(key.hash.clone(), cached_entry).await;
        debug!("[CACHE SET] simulation {}", key.display_id());
        self.log_stats_if_needed().await;
    }

    pub async fn get_debug(&self, key: &CacheKey) -> Option<Arc<DebuggerInfo>> {
        match self.cache.get(&key.hash).await {
            Some(cached) => {
                if cached.is_expired(self.ttl) {
                    info!("[CACHE EXPIRED] Invalidating {}", key.display_id());
                    self.invalidate(key).await;
                    let mut misses = self.misses.write().await;
                    *misses += 1;
                    drop(misses);
                    self.log_stats_if_needed().await;
                    None
                } else {
                    info!("[CACHE HIT] debug {}", key.display_id());
                    let mut hits = self.hits.write().await;
                    *hits += 1;
                    drop(hits);
                    self.log_stats_if_needed().await;

                    match &cached.result {
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
            }
            None => {
                info!("[CACHE MISS] debug {}", key.display_id());
                let mut misses = self.misses.write().await;
                *misses += 1;
                drop(misses);
                self.log_stats_if_needed().await;
                None
            }
        }
    }

    pub async fn set_debug(&self, key: &CacheKey, result: Arc<DebuggerInfo>) {
        let cached_entry = CacheEntry::new_debug(result);
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
