use crate::analytics::{Aggregates, AnalyticsSink, ClickEvent, Stats, EVENTS_MAX};
use crate::store::{Record, Store, StoreError};
use heed::byteorder::BigEndian;
use heed::types::{Bytes, Str, U64};
use heed::{Database, Env, EnvOpenOptions};
use std::collections::BTreeMap;
use std::path::Path;

type BeU64 = U64<BigEndian>;

/// Particionamento defensivo do espaço de 40 bits entre nós (ver docs/SCALING.md).
/// Os 8 bits altos identificam o nó; os 32 baixos são o contador local do nó.
/// `allow(dead_code)`: helpers ainda não têm chamador — a fiação com
/// `next_id`/`open` é a Task 2 deste tijolo.
#[allow(dead_code)]
const NODE_BITS: u32 = 8;
#[allow(dead_code)]
const LOCAL_BITS: u32 = 40 - NODE_BITS; // 32
#[allow(dead_code)]
const LOCAL_MAX: u64 = (1u64 << LOCAL_BITS) - 1; // 4_294_967_295

/// Lê `QUARK_NODE_ID`: ausente/vazio → `None` (modo single-node, 40 bits inteiros);
/// "0".."255" → `Some(n)`; qualquer outra coisa → erro (fail-fast no startup).
#[allow(dead_code)]
fn parse_node_id(raw: Option<String>) -> Result<Option<u8>, StoreError> {
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u8>().map(Some).map_err(|_| {
            StoreError::Backend(format!("QUARK_NODE_ID inválido: {s} (esperado 0-255)"))
        }),
    }
}

/// Compõe o id final a partir do contador. Sem node-id: identidade (o chamador
/// aplica o teto de `permute::MAX_ID`, como hoje). Com node-id: prefixa os bits
/// altos e falha ANTES de estourar a faixa local (invariante de não-vazamento).
#[allow(dead_code)]
fn compose_id(node_id: Option<u8>, counter: u64) -> Result<u64, StoreError> {
    match node_id {
        None => Ok(counter),
        Some(node) => {
            if counter > LOCAL_MAX {
                Err(StoreError::IdSpaceExhausted)
            } else {
                Ok(((node as u64) << LOCAL_BITS) | counter)
            }
        }
    }
}

pub struct LmdbStore {
    env: Env,
    links: Database<BeU64, Bytes>,  // id -> Record (json)
    aliases: Database<Str, BeU64>,  // alias -> id
    meta: Database<Str, BeU64>,     // "next_id" -> u64
    stats: Database<BeU64, Bytes>,  // id -> Aggregates (json)
    events: Database<BeU64, Bytes>, // id -> Vec<ClickEvent> (json, truncado a EVENTS_MAX)
}

impl LmdbStore {
    pub fn open(path: &Path) -> Result<LmdbStore, StoreError> {
        std::fs::create_dir_all(path).map_err(heed::Error::Io)?;
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(64 * 1024 * 1024 * 1024) // 64 GiB de espaço de endereço virtual (mmap)
                .max_dbs(5)
                .open(path)?
        };
        let mut wtxn = env.write_txn()?;
        let links = env.create_database(&mut wtxn, Some("links"))?;
        let aliases = env.create_database(&mut wtxn, Some("aliases"))?;
        let meta = env.create_database(&mut wtxn, Some("meta"))?;
        let stats = env.create_database(&mut wtxn, Some("stats"))?;
        let events = env.create_database(&mut wtxn, Some("events"))?;
        wtxn.commit()?;
        Ok(LmdbStore {
            env,
            links,
            aliases,
            meta,
            stats,
            events,
        })
    }
}

#[async_trait::async_trait]
impl Store for LmdbStore {
    async fn next_id(&self) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.links.get(&rtxn, &id)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        self.links.put(&mut wtxn, &id, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError> {
        let rtxn = self.env.read_txn()?;
        Ok(self.aliases.get(&rtxn, alias)?)
    }

    /// Grava alias + link em uma única transação: ou ambos ou nenhum.
    /// Evita link órfão se o processo falhar entre as duas escritas.
    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        if self.aliases.get(&wtxn, alias)?.is_some() {
            return Ok(false);
        }
        self.links.put(&mut wtxn, &id, &bytes)?;
        self.aliases.put(&mut wtxn, alias, &id)?;
        wtxn.commit()?;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl AnalyticsSink for LmdbStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        // Agrupa por id em memória pra minimizar leituras/escritas.
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id).or_default().push(e);
        }
        let mut wtxn = self.env.write_txn()?;
        for (id, evs) in by_id {
            // agregados: lê-modifica-grava
            let mut agg: Aggregates = match self.stats.get(&wtxn, &id)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Aggregates::default(),
            };
            for e in &evs {
                agg.apply(e);
            }
            self.stats.put(&mut wtxn, &id, &serde_json::to_vec(&agg)?)?;

            // eventos crus: append + trunca aos últimos EVENTS_MAX
            let mut recent: Vec<ClickEvent> = match self.events.get(&wtxn, &id)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Vec::new(),
            };
            for e in &evs {
                recent.push((*e).clone());
            }
            if recent.len() > EVENTS_MAX {
                let drop = recent.len() - EVENTS_MAX;
                recent.drain(0..drop);
            }
            self.events
                .put(&mut wtxn, &id, &serde_json::to_vec(&recent)?)?;
        }
        wtxn.commit()?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let agg = match self.stats.get(&rtxn, &id)? {
            Some(b) => serde_json::from_slice::<Aggregates>(b)?,
            None => return Ok(None),
        };
        let recent: Vec<ClickEvent> = match self.events.get(&rtxn, &id)? {
            Some(b) => serde_json::from_slice(b)?,
            None => Vec::new(),
        };
        Ok(Some(Stats {
            aggregates: agg,
            recent,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_id, parse_node_id, LOCAL_BITS, LOCAL_MAX};
    use crate::store::StoreError;

    #[test]
    fn parse_node_id_ausente_ou_vazio_vira_none() {
        assert!(matches!(parse_node_id(None), Ok(None)));
        assert!(matches!(parse_node_id(Some(String::new())), Ok(None)));
    }

    #[test]
    fn parse_node_id_valido() {
        assert!(matches!(parse_node_id(Some("0".into())), Ok(Some(0))));
        assert!(matches!(parse_node_id(Some("255".into())), Ok(Some(255))));
        assert!(matches!(parse_node_id(Some("7".into())), Ok(Some(7))));
    }

    #[test]
    fn parse_node_id_invalido_erra() {
        assert!(matches!(
            parse_node_id(Some("256".into())),
            Err(StoreError::Backend(_))
        ));
        assert!(matches!(
            parse_node_id(Some("-1".into())),
            Err(StoreError::Backend(_))
        ));
        assert!(matches!(
            parse_node_id(Some("abc".into())),
            Err(StoreError::Backend(_))
        ));
    }

    #[test]
    fn compose_id_sem_node_e_identidade() {
        // modo default: contador vira o id direto (checagem de MAX_ID fica no chamador)
        assert_eq!(compose_id(None, 1).unwrap(), 1);
        assert_eq!(compose_id(None, 1_000_000_000).unwrap(), 1_000_000_000);
    }

    #[test]
    fn compose_id_com_node_prefixa_os_bits_altos() {
        assert_eq!(compose_id(Some(0), 1).unwrap(), 1);
        assert_eq!(compose_id(Some(1), 1).unwrap(), (1u64 << LOCAL_BITS) | 1);
        assert_eq!(compose_id(Some(5), 42).unwrap(), (5u64 << LOCAL_BITS) | 42);
    }

    #[test]
    fn compose_id_faixas_de_nos_sao_disjuntas() {
        // o maior id do nó 0 é menor que o menor id do nó 1
        let maior_no_0 = compose_id(Some(0), LOCAL_MAX).unwrap();
        let menor_no_1 = compose_id(Some(1), 1).unwrap();
        assert!(maior_no_0 < menor_no_1);
    }

    #[test]
    fn compose_id_estouro_do_contador_local_erra() {
        assert_eq!(
            compose_id(Some(3), LOCAL_MAX).unwrap(),
            (3u64 << LOCAL_BITS) | LOCAL_MAX
        );
        assert!(matches!(
            compose_id(Some(3), LOCAL_MAX + 1),
            Err(StoreError::IdSpaceExhausted)
        ));
    }
}
