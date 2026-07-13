// src/abuse/ratelimit.rs — implementado na Task 2

use std::collections::HashMap;
use std::sync::Mutex;

/// Rate-limit por IP em janela fixa de 60s. Fail-open: erro de Valkey deixa passar.
pub struct RateLimiter(Mode);

enum Mode {
    Disabled,
    Memory {
        per_min: u32,
        // ip -> (janela atual, contagem na janela)
        state: Mutex<HashMap<String, (u64, u32)>>,
    },
    Valkey {
        per_min: u32,
        conn: redis::aio::MultiplexedConnection,
    },
}

const WINDOW_SECS: u64 = 60;

impl RateLimiter {
    pub fn disabled() -> RateLimiter {
        RateLimiter(Mode::Disabled)
    }

    pub fn memory(per_min: u32) -> RateLimiter {
        RateLimiter(Mode::Memory {
            per_min,
            state: Mutex::new(HashMap::new()),
        })
    }

    pub fn valkey(per_min: u32, conn: redis::aio::MultiplexedConnection) -> RateLimiter {
        RateLimiter(Mode::Valkey { per_min, conn })
    }

    /// `true` = permitido. `false` = estourou o limite nesta janela.
    pub async fn check(&self, ip: &str, now_secs: u64) -> bool {
        let window = now_secs / WINDOW_SECS;
        match &self.0 {
            Mode::Disabled => true,
            Mode::Memory { per_min, state } => {
                let mut map = state.lock().unwrap();
                let entry = map.entry(ip.to_string()).or_insert((window, 0));
                if entry.0 != window {
                    *entry = (window, 0); // nova janela: reseta
                }
                entry.1 += 1;
                entry.1 <= *per_min
            }
            Mode::Valkey { per_min, conn } => {
                let key = format!("quark:rl:{ip}:{window}");
                let mut c = conn.clone();
                // INCR + EXPIRE; qualquer erro => fail-open (permite)
                let count: Result<i64, _> = redis::cmd("INCR").arg(&key).query_async(&mut c).await;
                match count {
                    Ok(n) => {
                        // best-effort: só põe TTL na primeira vez; erro de EXPIRE não bloqueia
                        if n == 1 {
                            let _: Result<(), _> = redis::cmd("EXPIRE")
                                .arg(&key)
                                .arg(WINDOW_SECS as i64 * 2)
                                .query_async(&mut c)
                                .await;
                        }
                        n as u32 <= *per_min
                    }
                    Err(_) => true, // fail-open
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RateLimiter;

    #[tokio::test]
    async fn disabled_sempre_permite() {
        let rl = RateLimiter::disabled();
        for _ in 0..1000 {
            assert!(rl.check("1.2.3.4", 100).await);
        }
    }

    #[tokio::test]
    async fn memoria_bloqueia_acima_do_limite_na_janela() {
        let rl = RateLimiter::memory(2);
        let now = 600; // janela = 600/60 = 10
        assert!(rl.check("1.1.1.1", now).await); // 1
        assert!(rl.check("1.1.1.1", now).await); // 2
        assert!(!rl.check("1.1.1.1", now).await); // 3 -> bloqueia
    }

    #[tokio::test]
    async fn memoria_reseta_na_proxima_janela() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("2.2.2.2", 600).await); // janela 10, count 1
        assert!(!rl.check("2.2.2.2", 600).await); // estoura
        assert!(rl.check("2.2.2.2", 660).await); // janela 11: reseta, permite
    }

    #[tokio::test]
    async fn memoria_ips_distintos_nao_interferem() {
        let rl = RateLimiter::memory(1);
        assert!(rl.check("3.3.3.3", 600).await);
        assert!(rl.check("4.4.4.4", 600).await); // outro IP, própria conta
    }
}
