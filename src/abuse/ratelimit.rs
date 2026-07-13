// src/abuse/ratelimit.rs — implementado na Task 2

use std::collections::HashMap;
use std::sync::Mutex;

/// Rate-limit por IP em janela fixa de 60s. Fail-open: erro de Valkey deixa passar.
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

/// Estado do modo memória: mapa ip -> (janela atual, contagem na janela) +
/// a última janela em que o mapa foi podado (poda O(n) no máximo 1x por janela).
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

    /// `true` = permitido. `false` = estourou o limite nesta janela.
    pub async fn check(&self, ip: &str, now_secs: u64) -> bool {
        let window = now_secs / WINDOW_SECS;
        match &self.0 {
            Mode::Disabled => true,
            Mode::Memory { per_min, state } => {
                let mut st = state.lock().unwrap();
                if st.swept_window != window {
                    // Poda O(n), no máximo uma vez por virada de janela: entradas de
                    // janelas antigas são inúteis (a nova janela zera a contagem mesmo),
                    // então descartá-las evita crescimento ilimitado do HashMap.
                    st.map.retain(|_, (w, _)| *w == window);
                    st.swept_window = window;
                }
                let entry = st.map.entry(ip.to_string()).or_insert((window, 0));
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

    #[tokio::test]
    async fn memoria_poda_entradas_de_janelas_antigas() {
        let rl = RateLimiter::memory(100);
        // janela 10 (600/60): 3 IPs distintos
        rl.check("1.1.1.1", 600).await;
        rl.check("2.2.2.2", 600).await;
        rl.check("3.3.3.3", 600).await;
        assert_eq!(rl.memory_entries(), 3);
        // janela 11 (660/60): uma checagem varre as antigas, sobra só a nova
        rl.check("9.9.9.9", 660).await;
        assert_eq!(rl.memory_entries(), 1);
    }
}
