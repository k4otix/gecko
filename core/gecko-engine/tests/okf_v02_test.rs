//! OKF provenance read from whole bundles: a hand-written fixture covering each
//! field, written to OKF v0.2, and this repository's own OpenWiki bundle. Serviceless.

use std::path::Path;

use chrono::{DateTime, Utc};
use gecko_engine::okf::parser::parse_bundle;
use gecko_engine::okf::types::{OkfBundle, OkfConcept, TrustTier};

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn concept<'a>(bundle: &'a OkfBundle, id: &str) -> &'a OkfConcept {
    bundle
        .concepts
        .iter()
        .find(|c| c.concept_id == id)
        .unwrap_or_else(|| panic!("no concept {id}"))
}

#[test]
fn the_v02_fixture_bundle_reads_as_written() {
    let bundle = parse_bundle(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/okf_v02"))
        .expect("the fixture parses");
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    assert_eq!(bundle.okf_version.as_deref(), Some("9.9"));

    let human = &concept(&bundle, "human").provenance;
    assert_eq!(human.trust_tier, Some(TrustTier::HumanReviewed));
    assert_eq!(human.status.as_deref(), Some("deprecated"));
    assert_eq!(human.stale_after, Some(utc("2027-01-01T00:00:00Z")));
    assert_eq!(human.verifications.len(), 2);

    let bare = &concept(&bundle, "bare").provenance;
    assert_eq!(bare.trust_tier, Some(TrustTier::MachineConfirmed));
    assert_eq!(bare.verifications.len(), 1);

    let legacy = concept(&bundle, "legacy");
    assert_eq!(legacy.timestamp, Some(utc("2026-11-01T00:00:00Z")));
    assert_eq!(legacy.provenance.generated_by.as_deref(), Some("tool/1.0"));
    assert!(legacy.extra_metadata.contains_key("timestamp"));

    let unknown = &concept(&bundle, "unknown").provenance;
    assert_eq!(unknown.status.as_deref(), Some("published"));
    assert_eq!(unknown.trust_tier, None);
}

#[test]
fn the_repos_own_openwiki_bundle_parses() {
    let bundle = parse_bundle(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../openwiki"))
        .expect("openwiki/ parses");
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    assert_eq!(bundle.okf_version.as_deref(), Some("0.2"));
    for c in &bundle.concepts {
        assert!(
            c.timestamp.is_some(),
            "{} has no generated.at",
            c.concept_id
        );
        assert!(
            c.provenance.generated_by.is_some(),
            "{} has no generated.by",
            c.concept_id
        );
        let expected = if c.provenance.verifications.is_empty() {
            None
        } else {
            Some(TrustTier::MachineConfirmed)
        };
        assert_eq!(c.provenance.trust_tier, expected, "{}", c.concept_id);
    }
}
