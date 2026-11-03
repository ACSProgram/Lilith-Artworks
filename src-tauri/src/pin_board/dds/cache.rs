//! 纹理读取结果缓存。
//!
//! 大图（上亿像素）每次请求都要重新解码全部 BC7 块，成本与目标尺寸几乎无关；
//! 缓存按 (仓库根, 画板, 条目, 请求长边) 保存解码结果，滑动回来或重复请求同一
//! 尺寸时直接复用。预算由设置中的缓存等级决定。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

/// 默认预算（中等，128 MiB）。
const TEXTURE_CACHE_DEFAULT_BUDGET_BYTES: u64 = 128 * 1024 * 1024;

/// 素材板纹理缓存等级对应的 Rust 解码结果缓存预算（前端 GPU 缓存等级见
/// `texturePolicy.ts` 的 `textureBudgetsForLevel`）。低 64 / 中 128 / 高 256 MiB。
pub(crate) fn texture_cache_budget_bytes(level: &str) -> u64 {
    match level {
        "low" => 64 * 1024 * 1024,
        "high" => 256 * 1024 * 1024,
        _ => TEXTURE_CACHE_DEFAULT_BUDGET_BYTES,
    }
}

struct TextureCacheEntry {
    bytes: Vec<u8>,
    last_used: u64,
}

/// 纹理结果缓存的键。仓库根参与键——切换仓库后画板与条目 id 会从头复用，只按
/// (画板, 条目, 请求长边) 命中会把上一个仓库的解码结果当成本仓库的。字段各自具名，
/// 调用处因此不会把画板 id 与条目 id 写反；根路径按 `PathBuf` 原样比较，不做
/// 有损字符串转换。
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct TextureCacheKey {
    root: PathBuf,
    board_id: i64,
    item_id: i64,
    max_dimension: u32,
}

impl TextureCacheKey {
    pub(crate) fn new(root: &Path, board_id: i64, item_id: i64, max_dimension: u32) -> Self {
        Self {
            root: root.to_path_buf(),
            board_id,
            item_id,
            max_dimension,
        }
    }
}

static TEXTURE_CACHE: OnceLock<Mutex<HashMap<TextureCacheKey, TextureCacheEntry>>> =
    OnceLock::new();
static TEXTURE_CACHE_CLOCK: AtomicU64 = AtomicU64::new(1);

fn texture_cache_tick() -> u64 {
    TEXTURE_CACHE_CLOCK.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn texture_cache_get(key: &TextureCacheKey) -> Option<Vec<u8>> {
    let cache = TEXTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match cache.get_mut(key) {
        Some(entry) => {
            entry.last_used = texture_cache_tick();
            Some(entry.bytes.clone())
        }
        None => None,
    }
}

pub(crate) fn texture_cache_store(key: TextureCacheKey, bytes: Vec<u8>, budget_bytes: u64) {
    let cache = TEXTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(
        key,
        TextureCacheEntry {
            bytes,
            last_used: texture_cache_tick(),
        },
    );
    texture_cache_evict(&mut cache, budget_bytes);
}

/// 预算超限时按最久未使用逐个淘汰（条目数受预算约束，线性扫描可接受）。
///
/// 至少保留一条：正在写入的那份刚被 `texture_cache_store` 插进来，`last_used`
/// 最大，循环不会先删到它。
fn texture_cache_evict(cache: &mut HashMap<TextureCacheKey, TextureCacheEntry>, budget_bytes: u64) {
    while cache.len() > 1 {
        let total: u64 = cache.values().map(|entry| entry.bytes.len() as u64).sum();
        if total <= budget_bytes {
            break;
        }
        let Some(victim) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&victim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_key(root: &str, board_id: i64, item_id: i64, max_dimension: u32) -> TextureCacheKey {
        TextureCacheKey::new(Path::new(root), board_id, item_id, max_dimension)
    }

    #[test]
    fn evicts_oldest_texture_cache_entries_first() {
        let mut cache: HashMap<TextureCacheKey, TextureCacheEntry> = HashMap::new();
        let mut tick = 0u64;
        let mut insert =
            |key: TextureCacheKey,
             bytes: usize,
             budget_bytes: u64,
             cache: &mut HashMap<TextureCacheKey, TextureCacheEntry>| {
                tick += 1;
                cache.insert(
                    key,
                    TextureCacheEntry {
                        bytes: vec![0; bytes],
                        last_used: tick,
                    },
                );
                texture_cache_evict(cache, budget_bytes);
            };

        let budget = 96 * 1024 * 1024;
        insert(
            cache_key("root-a", 1, 1, 256),
            40 * 1024 * 1024,
            budget,
            &mut cache,
        );
        insert(
            cache_key("root-a", 1, 2, 256),
            40 * 1024 * 1024,
            budget,
            &mut cache,
        );
        insert(
            cache_key("root-a", 1, 3, 256),
            40 * 1024 * 1024,
            budget,
            &mut cache,
        );

        // 120 MiB 超过 96 MiB 预算：淘汰最久未使用的一条（40 MiB）后回到预算内。
        assert!(cache.contains_key(&cache_key("root-a", 1, 3, 256)));
        assert!(cache.contains_key(&cache_key("root-a", 1, 2, 256)));
        assert!(!cache.contains_key(&cache_key("root-a", 1, 1, 256)));
    }

    #[test]
    fn texture_cache_round_trips_payloads() {
        texture_cache_store(
            cache_key("root-a", 7, 3, 256),
            vec![1, 2, 3, 4],
            96 * 1024 * 1024,
        );
        assert_eq!(
            texture_cache_get(&cache_key("root-a", 7, 3, 256)),
            Some(vec![1, 2, 3, 4])
        );
        assert_eq!(texture_cache_get(&cache_key("root-a", 7, 4, 256)), None);
    }

    #[test]
    fn texture_cache_separates_repository_roots() {
        // 切仓库后画板与条目 id 会复用：同一 (board, item, dim) 在另一个仓库必须
        // 不命中，否则会读到上一个仓库的解码结果。
        texture_cache_store(
            cache_key("root-a", 7, 3, 256),
            vec![1, 2, 3, 4],
            96 * 1024 * 1024,
        );
        assert_eq!(texture_cache_get(&cache_key("root-b", 7, 3, 256)), None);
        assert_eq!(
            texture_cache_get(&cache_key("root-a", 7, 3, 256)),
            Some(vec![1, 2, 3, 4])
        );
    }

    #[test]
    fn texture_cache_budget_follows_cache_level() {
        assert_eq!(texture_cache_budget_bytes("low"), 64 * 1024 * 1024);
        assert_eq!(texture_cache_budget_bytes("medium"), 128 * 1024 * 1024);
        assert_eq!(texture_cache_budget_bytes("high"), 256 * 1024 * 1024);
        assert_eq!(
            texture_cache_budget_bytes(""),
            TEXTURE_CACHE_DEFAULT_BUDGET_BYTES
        );
    }
}
