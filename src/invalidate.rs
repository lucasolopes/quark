use crate::api::AppState;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Valkey pub/sub channel carrying cross-node invalidation messages. Payloads are
/// tiny text: `link:<id>` (drop one L1 cache entry everywhere) or `blocklist`
/// (force every node to reload its blocklist snapshot on the next check).
pub const INVALIDATION_CHANNEL: &str = "quark:invalidate";

/// Backoff between subscriber reconnect attempts. The per-node TTL (L1 60s /
/// blocklist 60s) covers staleness while a subscriber is reconnecting.
const RECONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

/// Bound on a PUBLISH so a connected-but-unresponsive Valkey (overload, pause,
/// black-hole) cannot hang the admin write that triggered the invalidation.
/// Mirrors the L2 cache op timeout; on timeout the message is simply dropped,
/// and the per-node TTL still bounds staleness.
const PUBLISH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// Best-effort publisher for cross-node invalidation messages. Holds an optional
/// clone of the shared multiplexed control connection; when absent (single-node,
/// no `QUARK_VALKEY_URL`) every publish is a silent no-op, so single-node
/// behavior is unchanged.
pub struct Invalidator {
    pub conn: Option<redis::aio::MultiplexedConnection>,
}

impl Invalidator {
    /// Publishes a message on the invalidation channel. Best-effort and
    /// fail-open: any Valkey error is logged and swallowed, never blocking the
    /// caller or the hot path. A `None` connection is a no-op.
    pub async fn publish(&self, msg: &str) {
        let Some(conn) = &self.conn else {
            return;
        };
        let mut c = conn.clone();
        let mut cmd = redis::cmd("PUBLISH");
        cmd.arg(INVALIDATION_CHANNEL).arg(msg);
        let publish = cmd.query_async::<()>(&mut c);
        match tokio::time::timeout(PUBLISH_TIMEOUT, publish).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("invalidate: publish '{msg}' failed (ignored): {e}"),
            Err(_) => eprintln!("invalidate: publish '{msg}' timed out (ignored)"),
        }
    }
}

/// Parsed invalidation message. Unknown payloads yield `None`.
#[derive(Debug, PartialEq, Eq)]
enum Invalidation {
    Link(u64),
    Blocklist,
}

/// Parses a channel payload into an `Invalidation`. `link:<u64>` and `blocklist`
/// are the only accepted forms; anything else (bad prefix, non-numeric id,
/// garbage) is `None`.
fn parse_message(payload: &str) -> Option<Invalidation> {
    if payload == "blocklist" {
        return Some(Invalidation::Blocklist);
    }
    let rest = payload.strip_prefix("link:")?;
    rest.parse::<u64>().ok().map(Invalidation::Link)
}

/// Spawns the background subscriber. Opens a dedicated pub/sub connection
/// (SUBSCRIBE monopolizes a connection and cannot share the multiplexed one),
/// subscribes to the invalidation channel, and dispatches each message to the
/// LOCAL-ONLY invalidation methods (never re-publishing, so no cross-node loop).
/// On stream error or disconnect it logs, backs off, and reconnects forever.
pub fn spawn_invalidation_subscriber(url: String, state: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match run_once(&url, &state).await {
                Ok(()) => eprintln!("invalidate: subscriber stream ended; reconnecting"),
                Err(e) => eprintln!("invalidate: subscriber error ({e}); reconnecting"),
            }
            tokio::time::sleep(RECONNECT_BACKOFF).await;
        }
    })
}

/// One connect/subscribe/consume cycle. Returns `Ok` when the stream ends
/// cleanly, `Err` on any connection/protocol failure; the caller reconnects
/// either way.
async fn run_once(url: &str, state: &Arc<AppState>) -> Result<(), redis::RedisError> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(INVALIDATION_CHANNEL).await?;
    eprintln!("invalidate: subscribed to {INVALIDATION_CHANNEL}");
    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("invalidate: unreadable payload (ignored): {e}");
                continue;
            }
        };
        match parse_message(&payload) {
            Some(Invalidation::Link(id)) => state.cache.invalidate_local(id).await,
            Some(Invalidation::Blocklist) => state.blocklist.invalidate_local().await,
            None => eprintln!("invalidate: unknown message '{payload}' (ignored)"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_without_connection_is_noop() {
        let inv = Invalidator { conn: None };
        inv.publish("link:1").await;
        inv.publish("blocklist").await;
    }

    #[test]
    fn parses_link_ids() {
        assert_eq!(parse_message("link:42"), Some(Invalidation::Link(42)));
        assert_eq!(parse_message("link:0"), Some(Invalidation::Link(0)));
    }

    #[test]
    fn parses_blocklist() {
        assert_eq!(parse_message("blocklist"), Some(Invalidation::Blocklist));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(parse_message("link:x"), None);
        assert_eq!(parse_message("link:"), None);
        assert_eq!(parse_message("link:-1"), None);
        assert_eq!(parse_message("blocklist:1"), None);
        assert_eq!(parse_message(""), None);
        assert_eq!(parse_message("garbage"), None);
    }
}
