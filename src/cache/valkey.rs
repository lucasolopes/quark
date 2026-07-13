use crate::cache::{CacheTier, TierError};
use crate::store::Record;
use redis::AsyncCommands;

/// Tier L2 (rede) sobre Valkey/Redis via `redis::aio::MultiplexedConnection`.
/// Todo erro (rede, protocolo, (de)serialização) vira `TierError` — nunca
/// panica; quem decide o fallback pro store é o `Cache::get` (breaker).
pub struct ValkeyTier {
    conn: redis::aio::MultiplexedConnection,
}

impl ValkeyTier {
    pub async fn open(url: &str) -> Result<ValkeyTier, TierError> {
        let client = redis::Client::open(url).map_err(|e| TierError(e.to_string()))?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| TierError(e.to_string()))?;
        Ok(ValkeyTier { conn })
    }

    fn key(id: u64) -> String {
        format!("q:{id}")
    }
}

#[async_trait::async_trait]
impl CacheTier for ValkeyTier {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError> {
        let mut conn = self.conn.clone();
        let bytes: Option<Vec<u8>> = conn
            .get(Self::key(id))
            .await
            .map_err(|e| TierError(e.to_string()))?;
        match bytes {
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| TierError(e.to_string())),
            None => Ok(None),
        }
    }

    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError> {
        let mut conn = self.conn.clone();
        let b = serde_json::to_vec(rec).map_err(|e| TierError(e.to_string()))?;
        conn.set_ex::<_, _, ()>(Self::key(id), b, ttl_secs)
            .await
            .map_err(|e| TierError(e.to_string()))?;
        Ok(())
    }

    async fn invalidate(&self, id: u64) -> Result<(), TierError> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(Self::key(id))
            .await
            .map_err(|e| TierError(e.to_string()))?;
        Ok(())
    }
}
