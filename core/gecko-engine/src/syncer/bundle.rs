//! Bundle syncer: maps parsed OKF bundles to TypeDB entities and relations.
//!
//! GECKO is single-bundle-scoped: one bundle maps to one TypeDB database, so
//! concept IDs are plain OKF bundle-relative paths (no namespace prefix) and the
//! database itself is the isolation boundary between bundles.
//!
//! Writes use TypeDB's parameterized `given` stage (typed, out-of-band values)
//! instead of string interpolation. This eliminates TypeQL injection and lets us
//! batch each write into a single multi-row query regardless of concept count.
//! Idempotency is content-hash based ([`plan_sync`]): unchanged concepts are
//! skipped, changed ones updated **in place** (the entity is preserved so its
//! containment, hierarchy, incoming links, and any extension-created relations
//! survive an edit), and orphaned ones garbage-collected.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use tracing::{debug, info};
use typedb_driver::Transaction;
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

use crate::db::router::{DbError, TypeDbRouter};
use crate::okf::types::{OkfBundle, OkfConcept, ScriptEngine};

/// Results of a bundle sync operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncResult {
    pub concepts_inserted: usize,
    pub concepts_updated: usize,
    pub concepts_skipped: usize,
    pub concepts_deleted: usize,
    pub links_created: usize,
    pub citations_created: usize,
}

/// Escapes string literals for safe embedding in TypeQL.
///
/// Retained for the CLI read path; the write path in this module uses
/// parameterized `given` queries and never interpolates values into query text.
pub fn escape_tql(val: &str) -> String {
    val.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Per-concept disposition relative to what is already in the graph, computed by
/// comparing content hashes. Pure and unit-tested — no database access.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    /// Indices into the manifest's concepts that are new (absent from the graph).
    pub to_insert: Vec<usize>,
    /// Indices into the manifest's concepts whose content hash changed.
    pub to_update: Vec<usize>,
    /// Count of concepts whose hash matched (no writes needed).
    pub skipped: usize,
    /// Concept IDs present in the graph but absent from the manifest (to GC).
    pub to_delete: Vec<String>,
}

/// Computes the sync plan by diffing the manifest against the concept IDs and
/// content hashes already stored in the graph.
///
/// `existing` maps each stored concept-id to its stored `file-hash` (`None` when
/// a concept exists but has no stored hash — treated as changed so it is
/// rewritten). This is the real content-hash idempotency check (fixes C1): only
/// concepts whose hash actually differs are rewritten.
pub fn plan_sync(concepts: &[OkfConcept], existing: &HashMap<String, Option<String>>) -> SyncPlan {
    let mut plan = SyncPlan::default();
    let manifest_ids: HashSet<&str> = concepts.iter().map(|c| c.concept_id.as_str()).collect();

    for (idx, concept) in concepts.iter().enumerate() {
        match existing.get(&concept.concept_id) {
            None => plan.to_insert.push(idx),
            Some(stored) if stored.as_deref() == Some(concept.file_hash.as_str()) => {
                plan.skipped += 1
            }
            Some(_) => plan.to_update.push(idx),
        }
    }

    for existing_id in existing.keys() {
        if !manifest_ids.contains(existing_id.as_str()) {
            plan.to_delete.push(existing_id.clone());
        }
    }
    plan.to_delete.sort(); // deterministic output

    plan
}

/// Syncs an OKF bundle into TypeDB within a single write transaction.
pub async fn sync_bundle(
    db: &mut TypeDbRouter,
    manifest: &OkfBundle,
) -> Result<SyncResult, DbError> {
    let tx = db.begin_write().await?;

    // 1. Ensure the singleton bundle entity + its metadata.
    upsert_bundle(&tx, manifest).await?;

    // 2. Read existing (concept-id -> file-hash) to compute the idempotent plan.
    let existing = fetch_existing_hashes(&tx).await?;
    let plan = plan_sync(&manifest.concepts, &existing);
    debug!(
        insert = plan.to_insert.len(),
        update = plan.to_update.len(),
        skip = plan.skipped,
        delete = plan.to_delete.len(),
        "Computed sync plan"
    );

    // 3. Garbage-collect orphaned concepts: remove each concept's OKF relations
    //    (so none are left dangling) and then the concept entity.
    let gc_ids: Vec<&str> = plan.to_delete.iter().map(String::as_str).collect();
    delete_concepts_gc(&tx, &gc_ids).await?;

    // 4. Reconcile new + changed concepts. Changed concepts are updated IN PLACE:
    //    the entity is preserved (keeping containment, hierarchy, incoming links,
    //    and extension relations) and only file-derived content is refreshed.
    let update_ids: Vec<&str> = plan
        .to_update
        .iter()
        .map(|&i| manifest.concepts[i].concept_id.as_str())
        .collect();
    clear_concept_content(&tx, &update_ids).await?;

    let new_concepts: Vec<&OkfConcept> = plan
        .to_insert
        .iter()
        .map(|&i| &manifest.concepts[i])
        .collect();
    insert_concept_entities(&tx, &new_concepts).await?;

    let write_concepts: Vec<&OkfConcept> = plan
        .to_insert
        .iter()
        .chain(plan.to_update.iter())
        .map(|&i| &manifest.concepts[i])
        .collect();
    attach_concept_content(&tx, &write_concepts).await?;

    // 5. Containment for written concepts. The not-exists guard makes this a
    //    no-op for updated concepts, whose containment was preserved.
    let write_ids: Vec<&str> = write_concepts
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect();
    create_containment(&tx, &write_ids).await?;

    // 6. Hierarchy, links, citations (idempotent via not-exists guards).
    create_hierarchy(&tx, manifest).await?;
    let links_created = create_links(&tx, manifest).await?;
    let citations_created = create_citations(&tx, manifest).await?;

    tx.commit()
        .await
        .map_err(|e| DbError::Transaction(e.to_string()))?;

    let result = SyncResult {
        concepts_inserted: plan.to_insert.len(),
        concepts_updated: plan.to_update.len(),
        concepts_skipped: plan.skipped,
        concepts_deleted: plan.to_delete.len(),
        links_created,
        citations_created,
    };
    info!(
        inserted = result.concepts_inserted,
        updated = result.concepts_updated,
        skipped = result.concepts_skipped,
        deleted = result.concepts_deleted,
        links = result.links_created,
        citations = result.citations_created,
        "Bundle sync complete"
    );
    Ok(result)
}

// ── TypeQL helpers ───────────────────────────────────────────────────────────

fn str_entry(s: &str) -> GivenRowEntry {
    GivenRowEntry::Value(Value::String(s.to_string()))
}

fn dt_entry(dt: &DateTime<Utc>) -> GivenRowEntry {
    GivenRowEntry::Value(Value::Datetime(dt.naive_utc()))
}

fn engine_str(engine: &ScriptEngine) -> String {
    match engine {
        ScriptEngine::Rhai => "rhai",
        ScriptEngine::QuickJs => "quickjs",
    }
    .to_string()
}

/// Runs a parameterized `given` write, feeding `rows` as typed input values.
/// No-op when `rows` is empty (an empty `given` input would otherwise be sent).
async fn run_rows(
    tx: &Transaction,
    query: &str,
    vars: &[&str],
    rows: Vec<Vec<GivenRowEntry>>,
) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut given = GivenRows::new(vars.iter().map(|s| s.to_string()).collect(), rows.len());
    for row in rows {
        given
            .push_row(row)
            .map_err(|e| DbError::Query(e.to_string()))?;
    }
    tx.query_with_rows(query, given)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
    Ok(())
}

/// Ensures the singleton bundle entity exists (keyed on bundle-name) and sets its
/// provenance metadata once (path/description). GECKO is single-bundle-scoped so
/// there is exactly one bundle per database.
async fn upsert_bundle(tx: &Transaction, manifest: &OkfBundle) -> Result<(), DbError> {
    run_rows(
        tx,
        "given $name: string; put $b isa bundle, has bundle-name == $name;",
        &["name"],
        vec![vec![str_entry(&manifest.bundle_name)]],
    )
    .await?;

    // Set bundle-path if not already recorded (set-once provenance).
    run_rows(
        tx,
        "given $name: string, $path: string; \
         match $b isa bundle, has bundle-name $bn; $bn == $name; \
               not { $b has bundle-path $existing; }; \
         insert $b has bundle-path == $path;",
        &["name", "path"],
        vec![vec![
            str_entry(&manifest.bundle_name),
            str_entry(&manifest.bundle_path),
        ]],
    )
    .await?;

    if let Some(desc) = &manifest.bundle_description {
        run_rows(
            tx,
            "given $name: string, $desc: string; \
             match $b isa bundle, has bundle-name $bn; $bn == $name; \
                   not { $b has description $existing; }; \
             insert $b has description == $desc;",
            &["name", "desc"],
            vec![vec![str_entry(&manifest.bundle_name), str_entry(desc)]],
        )
        .await?;
    }

    Ok(())
}

/// Fetches every stored concept's ID and (optional) content hash.
async fn fetch_existing_hashes(
    tx: &Transaction,
) -> Result<HashMap<String, Option<String>>, DbError> {
    let answer = tx
        .query(r#"match $c isa concept, has concept-id $id; fetch { "id": $id, "hash": $c.file-hash };"#)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

    let mut out = HashMap::new();
    if answer.is_document_stream() {
        let mut stream = answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            let json: serde_json::Value =
                serde_json::from_str(&doc.into_json().to_string()).unwrap_or_default();
            if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                let hash = json
                    .get("hash")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                out.insert(id.to_string(), hash);
            }
        }
    }
    Ok(out)
}

/// Garbage-collects concepts that were removed from the bundle. Deletes the OKF
/// core relations each concept plays (containment, hierarchy, concept-link,
/// citation) so none are left dangling, then the concept entity itself.
///
/// Extension-defined relations (e.g. mem-gecko `contextualizes`) are intentionally
/// left untouched — their lifecycle belongs to the extension, not the OKF sync.
async fn delete_concepts_gc(tx: &Transaction, ids: &[&str]) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    let rows = || -> Vec<Vec<GivenRowEntry>> { ids.iter().map(|id| vec![str_entry(id)]).collect() };

    // Each specific OKF relation type the concept may participate in. We target
    // concrete types (not `okf-link`) so extension relations that also subtype
    // `okf-link` are not swept up.
    for rel in ["containment", "hierarchy", "concept-link", "citation"] {
        let query = format!(
            "given $id: string; \
             match $c isa concept, has concept-id $cid; $cid == $id; \
                   $r isa {rel}, links ($c); \
             delete $r;"
        );
        run_rows(tx, &query, &["id"], rows()).await?;
    }

    run_rows(
        tx,
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
         delete $c;",
        &["id"],
        rows(),
    )
    .await
}

/// Clears the file-derived content of changed concepts while preserving the
/// entity (and thus its containment, hierarchy, incoming links, and any
/// extension-created relations). Removes every non-key attribute and the
/// concept's OUTGOING links/citations; all of it is re-attached from the current
/// file by [`attach_concept_content`] and the relation passes.
async fn clear_concept_content(tx: &Transaction, ids: &[&str]) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    let rows = || -> Vec<Vec<GivenRowEntry>> { ids.iter().map(|id| vec![str_entry(id)]).collect() };

    // Remove every attribute except the concept-id key.
    run_rows(
        tx,
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
               $c has $a; not { $a is $cid; }; \
         delete has $a of $c;",
        &["id"],
        rows(),
    )
    .await?;

    // Remove outgoing links/citations (re-derived from the new body). Incoming
    // links (where this concept is the target) are left intact.
    run_rows(
        tx,
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
               $l isa concept-link, links (source: $c); \
         delete $l;",
        &["id"],
        rows(),
    )
    .await?;
    run_rows(
        tx,
        "given $id: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
               $cit isa citation, links (citing-concept: $c); \
         delete $cit;",
        &["id"],
        rows(),
    )
    .await
}

/// Inserts the entity and concept-id key for brand-new concepts. Their content
/// is attached separately by [`attach_concept_content`].
async fn insert_concept_entities(
    tx: &Transaction,
    concepts: &[&OkfConcept],
) -> Result<(), DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = concepts
        .iter()
        .map(|c| vec![str_entry(&c.concept_id)])
        .collect();
    run_rows(
        tx,
        "given $id: string; insert $c isa concept, has concept-id == $id;",
        &["id"],
        rows,
    )
    .await
}

/// Attaches all file-derived content (every attribute) to concepts whose entity
/// already exists. Used for new concepts (after [`insert_concept_entities`]) and
/// for updated concepts (after [`clear_concept_content`]), batched by attribute
/// kind — one query each regardless of concept count.
async fn attach_concept_content(tx: &Transaction, concepts: &[&OkfConcept]) -> Result<(), DbError> {
    if concepts.is_empty() {
        return Ok(());
    }

    // Required scalars.
    attach_attr(tx, concepts, "concept-type", |c| {
        vec![c.concept_type.clone()]
    })
    .await?;
    attach_attr(tx, concepts, "body", |c| vec![c.body.clone()]).await?;
    attach_attr(tx, concepts, "file-hash", |c| vec![c.file_hash.clone()]).await?;

    // Optional scalars.
    attach_attr(tx, concepts, "title", |c| {
        c.title.clone().into_iter().collect()
    })
    .await?;
    attach_attr(tx, concepts, "description", |c| {
        c.description.clone().into_iter().collect()
    })
    .await?;
    attach_attr(tx, concepts, "resource-uri", |c| {
        c.resource_uri.clone().into_iter().collect()
    })
    .await?;
    attach_attr(tx, concepts, "engine", |c| {
        c.engine.as_ref().map(engine_str).into_iter().collect()
    })
    .await?;
    attach_attr(tx, concepts, "metadata-json", |c| {
        if c.extra_metadata.is_empty() {
            vec![]
        } else {
            vec![serde_json::to_string(&c.extra_metadata).unwrap_or_default()]
        }
    })
    .await?;

    // Multi-valued.
    attach_attr(tx, concepts, "tag", |c| c.tags.clone()).await?;
    attach_attr(tx, concepts, "code-block", |c| c.code_blocks.clone()).await?;

    // Timestamp (datetime-typed).
    attach_datetime(tx, concepts, "timestamp", |c| c.timestamp).await?;

    Ok(())
}

/// Attaches a string-valued attribute to already-inserted concepts. `values`
/// yields zero or more values per concept (0..1 for optional scalars, 0..N for
/// multi-valued attributes). The attribute type name is a fixed schema label, not
/// user data.
async fn attach_attr(
    tx: &Transaction,
    concepts: &[&OkfConcept],
    attr: &str,
    values: impl Fn(&OkfConcept) -> Vec<String>,
) -> Result<(), DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = concepts
        .iter()
        .flat_map(|c| {
            let id = c.concept_id.clone();
            values(c)
                .into_iter()
                .map(move |v| vec![str_entry(&id), str_entry(&v)])
        })
        .collect();
    let query = format!(
        "given $id: string, $val: string; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
         insert $c has {attr} == $val;"
    );
    run_rows(tx, &query, &["id", "val"], rows).await
}

/// Attaches an optional datetime-valued attribute to already-inserted concepts.
async fn attach_datetime(
    tx: &Transaction,
    concepts: &[&OkfConcept],
    attr: &str,
    value: impl Fn(&OkfConcept) -> Option<DateTime<Utc>>,
) -> Result<(), DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = concepts
        .iter()
        .filter_map(|c| value(c).map(|dt| vec![str_entry(&c.concept_id), dt_entry(&dt)]))
        .collect();
    let query = format!(
        "given $id: string, $val: datetime; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
         insert $c has {attr} == $val;"
    );
    run_rows(tx, &query, &["id", "val"], rows).await
}

/// Links the given concepts to the (singleton) bundle via containment.
async fn create_containment(tx: &Transaction, ids: &[&str]) -> Result<(), DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = ids.iter().map(|id| vec![str_entry(id)]).collect();
    run_rows(
        tx,
        "given $id: string; \
         match $b isa bundle; \
               $c isa concept, has concept-id $cid; $cid == $id; \
               not { $x isa containment, links (container: $b, member: $c); }; \
         insert $ct isa containment, links (container: $b, member: $c);",
        &["id"],
        rows,
    )
    .await
}

/// Creates directory-hierarchy edges between concepts.
async fn create_hierarchy(tx: &Transaction, manifest: &OkfBundle) -> Result<(), DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .hierarchy
        .iter()
        .filter(|e| !e.parent_id.is_empty())
        .map(|e| vec![str_entry(&e.parent_id), str_entry(&e.child_id)])
        .collect();
    run_rows(
        tx,
        "given $parent: string, $child: string; \
         match $p isa concept, has concept-id $pid; $pid == $parent; \
               $c isa concept, has concept-id $cid; $cid == $child; \
               not { $h isa hierarchy, links (parent: $p, child: $c); }; \
         insert $nh isa hierarchy, links (parent: $p, child: $c);",
        &["parent", "child"],
        rows,
    )
    .await
}

/// Creates internal concept-to-concept links. Returns the number of links
/// attempted (links whose target is missing are tolerated and skipped by TypeDB).
async fn create_links(tx: &Transaction, manifest: &OkfBundle) -> Result<usize, DbError> {
    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .links
        .iter()
        .map(|l| {
            vec![
                str_entry(&l.source_id),
                str_entry(&l.target_id),
                str_entry(&l.link_text),
            ]
        })
        .collect();
    let attempted = rows.len();
    run_rows(
        tx,
        "given $src: string, $tgt: string, $txt: string; \
         match $s isa concept, has concept-id $sid; $sid == $src; \
               $t isa concept, has concept-id $tid; $tid == $tgt; \
               not { $l isa concept-link, links (source: $s, target: $t); }; \
         insert $nl isa concept-link, links (source: $s, target: $t), has link-text == $txt;",
        &["src", "tgt", "txt"],
        rows,
    )
    .await?;
    Ok(attempted)
}

/// Creates external-resource citation nodes and links. Returns the number of
/// citations attempted.
async fn create_citations(tx: &Transaction, manifest: &OkfBundle) -> Result<usize, DbError> {
    // Deduplicate the external-resource URLs (keyed by target-url).
    let mut urls: Vec<&str> = manifest
        .citations
        .iter()
        .map(|c| c.target_url.as_str())
        .collect();
    urls.sort();
    urls.dedup();
    let ext_rows: Vec<Vec<GivenRowEntry>> = urls.iter().map(|u| vec![str_entry(u)]).collect();
    run_rows(
        tx,
        "given $url: string; put $ext isa external-resource, has target-url == $url;",
        &["url"],
        ext_rows,
    )
    .await?;

    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .citations
        .iter()
        .map(|c| {
            vec![
                str_entry(&c.source_id),
                str_entry(&c.target_url),
                str_entry(&c.link_text),
            ]
        })
        .collect();
    let attempted = rows.len();
    run_rows(
        tx,
        "given $src: string, $url: string, $txt: string; \
         match $s isa concept, has concept-id $sid; $sid == $src; \
               $ext isa external-resource, has target-url $eu; $eu == $url; \
               not { $cit isa citation, links (citing-concept: $s, cited-resource: $ext); }; \
         insert $nc isa citation, links (citing-concept: $s, cited-resource: $ext), has link-text == $txt;",
        &["src", "url", "txt"],
        rows,
    )
    .await?;
    Ok(attempted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concept(id: &str, hash: &str) -> OkfConcept {
        OkfConcept {
            concept_id: id.to_string(),
            concept_type: "Note".to_string(),
            title: None,
            description: None,
            resource_uri: None,
            tags: vec![],
            timestamp: None,
            body: String::new(),
            code_blocks: vec![],
            extra_metadata: HashMap::new(),
            file_hash: hash.to_string(),
            source_path: format!("{id}.md"),
            consumes: vec![],
            produces: vec![],
            engine: None,
            scopes: vec![],
            timeout_ms: None,
        }
    }

    #[test]
    fn test_escape_tql() {
        assert_eq!(escape_tql(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn test_plan_sync_classifies_each_concept() {
        let concepts = vec![
            concept("a", "hash-a"),  // unchanged -> skip
            concept("b", "hash-b2"), // changed -> update
            concept("c", "hash-c"),  // new -> insert
        ];
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), Some("hash-a".to_string()));
        existing.insert("b".to_string(), Some("hash-b1".to_string()));
        existing.insert("d".to_string(), Some("hash-d".to_string())); // orphan -> delete

        let plan = plan_sync(&concepts, &existing);

        assert_eq!(plan.to_insert, vec![2]); // "c"
        assert_eq!(plan.to_update, vec![1]); // "b"
        assert_eq!(plan.skipped, 1); // "a"
        assert_eq!(plan.to_delete, vec!["d".to_string()]);
    }

    #[test]
    fn test_plan_sync_missing_hash_is_update() {
        let concepts = vec![concept("a", "hash-a")];
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), None); // present but no stored hash

        let plan = plan_sync(&concepts, &existing);
        assert_eq!(plan.to_update, vec![0]);
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn test_plan_sync_empty_graph_inserts_all() {
        let concepts = vec![concept("a", "h1"), concept("b", "h2")];
        let plan = plan_sync(&concepts, &HashMap::new());
        assert_eq!(plan.to_insert, vec![0, 1]);
        assert!(plan.to_update.is_empty());
        assert_eq!(plan.skipped, 0);
        assert!(plan.to_delete.is_empty());
    }
}
