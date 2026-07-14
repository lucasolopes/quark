use std::collections::HashMap;
use std::sync::Mutex;

/// Per-IP rate limit in a fixed 60s window. Fail-open: a Valkey error lets the request through.
pub struct RateLimiter(Mode);

enum Mode {
    Disabled,
    Memory {
        per_min: u32,
        state: Mutex<MemState>,
    },
    Valkey {
        per_min: u32,
        conn: redis::aio::MultiplexedConnection,
    },
}

/// State of the memory mode: map ip -> (current window, count in window) +
/// the last window in which the map was swept (O(n) sweep at most once per window).
struct MemState {
    swept_window: u64,
    map: HashMap<String, (u64, u32)>,
}

const WINDOW_SECS: u64 = 60;

impl RateLimiter {
    pub fn disabled() -> RateLimiter {
        RateLimiter(Mode::Disabled)
    }

    pub fn memory(per_min: u32) -> RateLimiter {
        RateLimiter(Mode::Memory {
            per_min,
            state: Mutex::new(MemState {
                swept_window: 0,
                map: HashMap::new(),
            }),
        })
    }

    pub fn valkey(per_min: u32, conn: redis::aio::MultiplexedConnection) -> RateLimiter {
        RateLimiter(Mode::Valkey { per_min, conn })
    }

    /// `true` = allowed. `false` = exceeded the limit in this window.
    pub async fn check(&self, ip: &str, now_secs: u64) -> bool {
        let window = now_secs / WINDOW_SECS;
        match &self.0 {
            Mode::Disabled => true,
            Mode::Memory { per_min, state } => {
                let mut st = state.lock().unwrap();
                if st.swept_window != window {
                    st.map.retain(|_, (w, _)| *w == window);
                    st.swept_window = window;
                }
                let entry = st.map.entry(ip.to_string()).or_insert((window, 0));
                if entry.0 != window {
                    *entry = (window, 0);
                }
                entry.1 += 1;
                entry.1 <= *per_min
            }
            Mode::Valkey { per_min, conn } => {
                let key = format!("quark:rl:{ip}:{window}");
                let mut c = conn.clone();
                let count: Result<i64, _> = redis::cmd("INCR").arg(&key).query_async(&mut c).await;
                match count {
                    Ok(n) => {
                        if n == 1 {
                            let _: Result<(), _> = redis::cmd("EXPIRE")
                                .arg(&key)
                                .arg(WINDOW_SECS as i64 * 2)
                                .query_async(&mut c)
                                .await;
                        }
                        n as u32 <= *per_min
                    }
                    Err(_) => true,
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn memory_entries(&self) -> usize {
        match &self.0 {
            Mode::Memory { state, .. } => state.lock().unwrap().map.len(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RateLimiter;

    #[tokio::test]
    async fn disabled_always_allows() {
        let rl = RateLimiter::disabled();
        for _ in 0..1000 {
            assert!(rl.check("1.2.3.4", 100).await);
        }
    }

    #[tokio::test]
    async fn memory_blocks_above_the_limit_in_the_window() {
        let rl = RateLimiter::memory(2);
        let now = 600;
        assert!(rl.check("1.1.1.1", now).await);
        assert!(rl.check("1.1.1.1", now).await);
        assert!(!rl.check("1.1.1.1", now).await);
    }

    #[tokio::test]
    async fn memory_resets_on_the_next_window() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("2.2.2.2", 600).await);
        assert!(!rl.check("2.2.2.2", 600).await);
        assert!(rl.check("2.2.2.2", 660).await);
    }

    #[tokio::test]
    async fn memory_distinct_ips_do_not_interfere() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("3.3.3.3", 600).await);
        assert!(rl.check("4.4.4.4", 600).await);
    }

    #[tokio::test]
    async fn memory_sweeps_entries_from_old_windows() {
        let rl = RateLimiter::memory(100);
        rl.check("1.1.1.1", 600).await;
        rl.check("2.2.2.2", 600).await;
        rl.check("3.3.3.3", 600).await;
        assert_eq!(rl.memory_entries(), 3);
        rl.check("9.9.9.9", 660).await;
        assert_eq!(rl.memory_entries(), 1);
    }
}
