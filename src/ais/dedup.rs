use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::time::{Duration, Instant};

pub struct DedupCache {
    seen: HashMap<u64, Instant>,
    ttl: Duration,
    last_clean: Instant,
}

impl DedupCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            seen: HashMap::with_capacity(4096),
            ttl,
            last_clean: Instant::now(),
        }
    }

    /// Returns true if this sentence was seen within TTL (duplicate)
    pub fn is_duplicate(&mut self, sentence: &str) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_clean) > Duration::from_secs(60) {
            self.seen.retain(|_, ts| now.duration_since(*ts) < self.ttl);
            self.last_clean = now;
        }
        let h = Self::hash_sentence(sentence);
        if let Some(ts) = self.seen.get(&h) {
            if now.duration_since(*ts) < self.ttl {
                return true;
            }
        }
        self.seen.insert(h, now);
        false
    }

    fn hash_sentence(sentence: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        sentence.hash(&mut hasher);
        hasher.finish()
    }
}
