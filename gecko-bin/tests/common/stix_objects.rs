//! Builders for the ATT&CK objects the STIX fixtures hold, and the sync that
//! seeds a test database from one of those fixtures.
//!
//! Included with `#[path = "common/stix_objects.rs"] mod stix_objects;` beside
//! `mod common;` by every test that includes a fixture built from it.

use cyber_gecko::CyberGecko;
use cyber_gecko::stix::{cyber_post_sync, to_okf};
use gecko_engine::db::CORE_SCHEMA;
use gecko_engine::extension::GeckoExtension;
use gecko_engine::syncer::bundle::sync_bundle;
use mem_gecko::MemGecko;
use serde_json::{Value, json};

use super::common::TestDb;

/// A log-source reference to `component`.
pub fn source(component: &str, name: &str, channel: Option<&str>) -> Value {
    let mut r = json!({ "x_mitre_data_component_ref": component, "name": name });
    if let Some(channel) = channel {
        r["channel"] = json!(channel);
    }
    r
}

/// The ATT&CK object `stix_id` with an `external_id` of `attack_id`.
pub fn object(ty: &str, stix_id: &str, name: &str, attack_id: &str) -> Value {
    json!({
        "type": ty,
        "id": stix_id,
        "name": name,
        "external_references": [{ "source_name": "mitre-attack", "external_id": attack_id }],
        "x_mitre_deprecated": false,
    })
}

/// An analytic on `platform` reading the log sources `sources`.
pub fn analytic(stix_id: &str, attack_id: &str, platform: &str, sources: Vec<Value>) -> Value {
    let mut a = object("x-mitre-analytic", stix_id, attack_id, attack_id);
    a["x_mitre_platforms"] = json!([platform]);
    if !sources.is_empty() {
        a["x_mitre_log_source_references"] = Value::Array(sources);
    }
    a
}

/// Applies the core, mem and cyber schemas to `db`, then syncs the STIX bundle
/// `stix` with its post-sync writes; `what` names the bundle in a failure.
pub async fn sync_stix_fixture(db: &TestDb, stix: &str, what: &str) {
    let mut router = db.router();
    router.apply_schema(CORE_SCHEMA).await.expect("core schema");
    for ext in [
        Box::new(MemGecko::new()) as Box<dyn GeckoExtension>,
        Box::new(CyberGecko::new()),
    ] {
        router
            .apply_schema(ext.schema())
            .await
            .unwrap_or_else(|e| panic!("{} schema: {e}", ext.name()));
    }
    let (manifest, rels) = to_okf(stix).expect("fixture converts");
    sync_bundle(
        &mut router,
        &manifest,
        &cyber_post_sync(&rels),
        mem_gecko::BELIEF_TIER_ROOT,
    )
    .await
    .unwrap_or_else(|e| panic!("sync {what}: {e:?}"));
}
