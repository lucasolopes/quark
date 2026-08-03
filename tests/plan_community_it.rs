// Codigo de teste pode entrar em panico: a falha e o proprio sinal.
#![allow(clippy::unwrap_used)]

//! The Community edition must never enforce a plan limit. A self-hosted AGPL
//! install is free and unlimited (LUC-19), so every gate resolves to `Ok`.
//!
//! This binary runs in BOTH builds on purpose: with `--features ee` the state
//! it builds has no plan configured, which must also resolve to the entry plan
//! without denying anything a Free tenant may do.

use quark::api::entitlement::{require, require_quota, Feature, Quota};
use quark::store::open_backends;
use quark::tenant::DEFAULT_TENANT;

mod common;

async fn state() -> std::sync::Arc<quark::api::AppState> {
    let dir = tempfile::tempdir().unwrap();
    let (store, sink) = open_backends(dir.path(), false).await.unwrap();
    common::TestState::new(store, sink).build()
}

#[tokio::test]
async fn community_allows_every_feature() {
    let st = state().await;
    for f in Feature::ALL {
        assert!(
            require(&st, DEFAULT_TENANT, f).await.is_ok(),
            "Community denied {f:?}"
        );
    }
}

#[tokio::test]
async fn community_allows_any_quota_usage() {
    let st = state().await;
    for q in Quota::ALL {
        assert!(
            require_quota(&st, DEFAULT_TENANT, q, 10_000).await.is_ok(),
            "Community denied {q:?} at 10k"
        );
    }
}
