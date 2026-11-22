//! Bundle syncer: maps parsed OKF bundles to TypeDB entities and relations.
//!
//! Concept IDs are plain OKF bundle-relative paths (no namespace prefix), unique
//! per database. A database holds one bundle by default and may hold several.
//! Each sync acts only on the concepts its bundle contains, and refuses concept
//! IDs that another bundle holds or that no bundle contains ([`find_collisions`]).
//!
//! Writes use TypeDB's parameterized `given` stage (typed, out-of-band values)
//! instead of string interpolation. This eliminates TypeQL injection and lets us
//! batch each write into a single multi-row query regardless of concept count.
//! Idempotency is content-hash based ([`plan_sync`]): unchanged concepts are
//! skipped, changed ones updated **in place** (the entity is preserved so its
//! containment, hierarchy, incoming links, and any extension-created relations
//! survive an edit). A concept whose `type_hint` changed is replaced instead: it
//! is deleted and reinserted under its new type. Orphans this bundle owns alone are garbage-collected;
//! orphans another bundle also contains are released (only this bundle's
//! containment is removed).
//!
//! # Record-path guardrail (invariants 1, 2 & 8)
//!
//! Concepts, documents and OKF links are **records**: observed, authoritative,
//! uncontested facts. This syncer writes them **directly** through the TypeDB
//! router. It MUST NOT route through the host-mediated belief-write path, and it
//! MUST NOT embed or push anything to the semantic retrieval accelerator. A
//! documentation sync is drag-free: it mints zero provenance/derivation apparatus
//! and zero accelerator entries. The belief tier — reified, run-id-stamped,
//! revisable — is populated exclusively by the epistemic writer in `mem-gecko`,
//! never here. The test `syncer_stays_on_the_record_path` enforces that this
//! module names none of those belief/accelerator types.
//!
//! Every sync fingerprints the belief tier at the start and end of its
//! transaction and refuses to commit when any count changed. A concept that a
//! belief edge references is never garbage-collected: the sync retires it instead
//! ([`retire_concepts`]), keeping the entity and its belief edges.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use tracing::{debug, info};
use typedb_driver::Transaction;
use typedb_driver::answer::ConceptRow;
use typedb_driver::concept::Value;
use typedb_driver::given::{GivenRowEntry, GivenRows};

use gecko_extension_api::GraphWrite;

use crate::db::graph_store::run_graph_writes;
use crate::db::required::required_attributes;
use crate::db::router::{DbError, TypeDbRouter};
use crate::db::surface::{BeliefTierSurface, belief_tier_fingerprint, belief_tier_surface};
use crate::db::validate_schema_label;
use crate::okf::types::{OkfBundle, OkfConcept, TypedAttrValue};

/// Results of a bundle sync operation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncResult {
    pub concepts_inserted: usize,
    pub concepts_updated: usize,
    pub concepts_skipped: usize,
    pub concepts_deleted: usize,
    /// Concepts this bundle dropped that another bundle still contains: only this
    /// bundle's containment was removed.
    pub concepts_released: usize,
    /// Concepts this sync newly retired: dropped from the bundle while a belief
    /// edge references them. A concept already retired counts as neither retired
    /// nor deleted.
    pub concepts_retired: usize,
    pub links_attempted: usize,
    pub citations_attempted: usize,
}

/// A concept already in the graph: its stored content hash, the names of every
/// bundle containing it (sorted, deduplicated; empty when uncontained), and
/// whether it is retired (owns a `retired-at`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredConcept {
    pub hash: Option<String>,
    pub bundles: Vec<String>,
    pub retired: bool,
}

/// The stored concepts `bundle_name` contains, as the concept-id → file-hash map
/// [`plan_sync`] diffs against.
pub fn owned_hashes(
    bundle_name: &str,
    stored: &HashMap<String, StoredConcept>,
) -> HashMap<String, Option<String>> {
    stored
        .iter()
        .filter(|(_, s)| s.bundles.iter().any(|b| b == bundle_name))
        .map(|(id, s)| (id.clone(), s.hash.clone()))
        .collect()
}

/// Every manifest concept ID that is stored but not contained by `bundle_name`,
/// sorted by ID and paired with a bundle that holds it (`None` when no bundle
/// contains it).
///
/// Only `concept` entities are considered. A manifest ID equal to a memory
/// item's ID (mem's `mem/…` namespace) is not reported here; it fails at insert
/// as a TypeDB key violation, so the sync still commits nothing.
pub fn find_collisions(
    bundle_name: &str,
    concepts: &[OkfConcept],
    stored: &HashMap<String, StoredConcept>,
) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = concepts
        .iter()
        .filter_map(|c| {
            let s = stored.get(&c.concept_id)?;
            if s.bundles.iter().any(|b| b == bundle_name) {
                None
            } else {
                Some((c.concept_id.clone(), s.bundles.first().cloned()))
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Splits the concepts `bundle_name` garbage-collects into `(delete, release)`,
/// both sorted: a concept only this bundle contains is deleted; one another
/// bundle also contains is released (only this bundle's containment goes).
pub fn split_gc(
    bundle_name: &str,
    to_delete: &[String],
    stored: &HashMap<String, StoredConcept>,
) -> (Vec<String>, Vec<String>) {
    let (mut release, mut delete): (Vec<String>, Vec<String>) =
        to_delete.iter().cloned().partition(|id| {
            stored
                .get(id)
                .is_some_and(|s| s.bundles.iter().any(|b| b != bundle_name))
        });
    delete.sort();
    release.sort();
    (delete, release)
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
/// rewritten). This is the content-hash idempotency check: only concepts whose
/// hash actually differs are rewritten.
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

/// Syncs an OKF bundle into TypeDB within a single write transaction. The
/// `extension_writes` run in that same transaction after the core writes, so a
/// failing extension write rolls back the whole sync. A manifest concept ID that
/// the database holds outside this bundle fails the sync with
/// [`DbError::ConceptIdCollision`] before anything is written, as does a written
/// concept whose `type_hint` is not `concept` or a subtype of it, with
/// [`DbError::Schema`].
///
/// A stored manifest concept whose exact type differs from its `type_hint`
/// (`concept` when absent) is replaced, whatever its hash: deleted with its OKF
/// relations and inserted under the new type, counted as updated. It loses its
/// incoming links from other bundles and its extension relations. The sync is
/// refused before anything is written when another bundle also contains such a
/// concept ([`DbError::TypeChangeShared`]) or a belief edge references it
/// ([`DbError::TypeChangeReferenced`]).
///
/// `belief_tier_root` names the root type of the belief tier, which the schema
/// must contain (otherwise [`DbError::Schema`]). A concept the sync would
/// garbage-collect that plays a role in a belief-tier relation is retired instead
/// ([`retire_concepts`]): it keeps its ID, the attributes its type requires, its
/// containment and its belief edges, and gains a `retired-at`. A concept already
/// retired gets no further writes; restoring its file makes it live again. The
/// sync is refused with [`DbError::BeliefTierWrite`] when any belief-tier label's
/// instance, attribute-ownership or role-player count differs between the start
/// of the transaction and just before commit. The guard does not see a
/// same-count swap: a value replaced in place, or one instance deleted and an
/// identical-shape one inserted.
pub async fn sync_bundle(
    db: &mut TypeDbRouter,
    manifest: &OkfBundle,
    extension_writes: &[GraphWrite],
    belief_tier_root: &str,
) -> Result<SyncResult, DbError> {
    let tx = db.begin_write().await?;
    let name = manifest.bundle_name.as_str();

    // 0. Fingerprint the belief tier before anything is read or written.
    let surface = belief_tier_surface(&tx, belief_tier_root).await?;
    let before = belief_tier_fingerprint(&tx, &surface).await?;

    // 1. Read every stored concept with its hash and containing bundles.
    let stored = fetch_stored_concepts(&tx).await?;

    // 2. Refuse IDs held outside this bundle. Returning drops the transaction
    //    uncommitted, so nothing is written.
    let collisions = find_collisions(name, &manifest.concepts, &stored);
    if !collisions.is_empty() {
        return Err(DbError::ConceptIdCollision {
            bundle: name.to_string(),
            ids: collisions,
        });
    }

    // 3. Plan against the concepts this bundle contains, and split its orphans
    //    into those it owns alone and those another bundle also contains.
    let plan = plan_sync(&manifest.concepts, &owned_hashes(name, &stored));
    let (gc_delete, gc_release) = split_gc(name, &plan.to_delete, &stored);
    debug!(
        insert = plan.to_insert.len(),
        update = plan.to_update.len(),
        skip = plan.skipped,
        delete = gc_delete.len(),
        release = gc_release.len(),
        "Computed sync plan"
    );
    // A stored concept whose exact type differs from its type_hint is retyped:
    // deleted with its OKF relations and inserted as new, whatever its hash.
    let retyped = retyped_concepts(&tx, &manifest.concepts, &plan).await?;
    let (retyped_updates, kept_updates): (Vec<usize>, Vec<usize>) = plan
        .to_update
        .iter()
        .copied()
        .partition(|i| retyped.contains_key(&manifest.concepts[*i].concept_id));
    let insert_idx: Vec<usize> = plan
        .to_insert
        .iter()
        .copied()
        .chain(
            manifest
                .concepts
                .iter()
                .enumerate()
                .filter_map(|(i, c)| retyped.contains_key(&c.concept_id).then_some(i)),
        )
        .collect();
    let new_concepts: Vec<&OkfConcept> =
        insert_idx.iter().map(|&i| &manifest.concepts[i]).collect();
    let write_concepts: Vec<&OkfConcept> = insert_idx
        .iter()
        .chain(kept_updates.iter())
        .map(|&i| &manifest.concepts[i])
        .collect();
    // Refuse a type_hint outside `concept`'s closure before anything is written.
    check_type_hints(&tx, &write_concepts).await?;
    // Refuse to retype a concept another bundle also contains: replacing it would
    // drop that bundle's containment.
    let shared_retypes: BTreeSet<&String> = retyped
        .keys()
        .filter(|id| {
            stored
                .get(*id)
                .is_some_and(|s| s.bundles.iter().any(|b| b != name))
        })
        .collect();
    if !shared_retypes.is_empty() {
        let ids = shared_retypes
            .into_iter()
            .map(|id| {
                let (from, to) = retyped[id].clone();
                (id.clone(), from, to)
            })
            .collect();
        return Err(DbError::TypeChangeShared { ids });
    }
    // Refuse to retype a concept a belief edge references: the edge cannot follow.
    let retyped_ids: Vec<&str> = retyped.keys().map(String::as_str).collect();
    let referenced_retypes: BTreeSet<String> = belief_edge_references(&tx, &retyped_ids, &surface)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    if !referenced_retypes.is_empty() {
        let ids = referenced_retypes
            .into_iter()
            .map(|id| {
                let (from, to) = retyped[&id].clone();
                (id, from, to)
            })
            .collect();
        return Err(DbError::TypeChangeReferenced { ids });
    }

    // 4. Ensure the bundle entity and its metadata.
    upsert_bundle(&tx, manifest).await?;

    // 5. Garbage-collect orphans: retire those this bundle owns alone that a
    //    belief edge references (unless already retired), delete the rest it owns
    //    alone (with their OKF relations, so none dangle), and release shared ones
    //    from this bundle.
    let gc_ids: Vec<&str> = gc_delete.iter().map(String::as_str).collect();
    let referenced: HashSet<String> = belief_edge_references(&tx, &gc_ids, &surface)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (referenced_ids, delete_ids): (Vec<&str>, Vec<&str>) = gc_ids
        .iter()
        .copied()
        .partition(|id| referenced.contains(*id));
    let retire_ids: Vec<&str> = referenced_ids
        .into_iter()
        .filter(|id| !stored.get(*id).is_some_and(|s| s.retired))
        .collect();
    if !retire_ids.is_empty() {
        let required = required_attributes(&db.database_schema().await?)?;
        retire_concepts(&tx, &retire_ids, &surface, &required, Utc::now()).await?;
    }
    delete_concepts_gc(&tx, &delete_ids).await?;
    // Retyped concepts go the same way, to be inserted under their new type in 6.
    delete_concepts_gc(&tx, &retyped_ids).await?;
    let release_ids: Vec<&str> = gc_release.iter().map(String::as_str).collect();
    release_containment(&tx, name, &release_ids).await?;

    // 6. Reconcile new, retyped and changed concepts. Retyped concepts were
    //    deleted above and are inserted as new. Changed concepts are updated IN
    //    PLACE: the entity is preserved (keeping containment, hierarchy, incoming
    //    links, and extension relations) and only file-derived content is refreshed.
    let update_ids: Vec<&str> = kept_updates
        .iter()
        .map(|&i| manifest.concepts[i].concept_id.as_str())
        .collect();
    clear_concept_content(&tx, &update_ids).await?;

    insert_concept_entities(&tx, &new_concepts).await?;
    attach_concept_content(&tx, &write_concepts).await?;

    // 7. Containment of written concepts in this bundle. The not-exists guard
    //    makes this a no-op for updated concepts, whose containment was preserved.
    let write_ids: Vec<&str> = write_concepts
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect();
    create_containment(&tx, name, &write_ids).await?;

    // 8. Hierarchy, links, citations (idempotent via not-exists guards).
    create_hierarchy(&tx, manifest).await?;
    let links_attempted = create_links(&tx, manifest).await?;
    let citations_attempted = create_citations(&tx, manifest).await?;

    // 9. Extension writes share the transaction, so they commit or roll back with it.
    run_graph_writes(&tx, extension_writes)
        .await
        .map_err(|e| DbError::Query(format!("extension write: {e}")))?;

    // 10. Refuse to commit any change to the belief tier. Returning drops the
    //     transaction uncommitted.
    let after = belief_tier_fingerprint(&tx, &surface).await?;
    let changes = before.changes_to(&after);
    if !changes.is_empty() {
        return Err(DbError::BeliefTierWrite { changes });
    }

    tx.commit()
        .await
        .map_err(|e| DbError::Transaction(e.to_string()))?;

    let result = SyncResult {
        concepts_inserted: plan.to_insert.len(),
        concepts_updated: kept_updates.len() + retyped.len(),
        concepts_skipped: plan.skipped - (retyped.len() - retyped_updates.len()),
        concepts_deleted: delete_ids.len(),
        concepts_released: gc_release.len(),
        concepts_retired: retire_ids.len(),
        links_attempted,
        citations_attempted,
    };
    info!(
        inserted = result.concepts_inserted,
        updated = result.concepts_updated,
        skipped = result.concepts_skipped,
        deleted = result.concepts_deleted,
        released = result.concepts_released,
        retired = result.concepts_retired,
        links_attempted = result.links_attempted,
        citations_attempted = result.citations_attempted,
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

fn bool_entry(b: bool) -> GivenRowEntry {
    GivenRowEntry::Value(Value::Boolean(b))
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

/// Ensures this sync's bundle entity exists (keyed on bundle-name) and sets its
/// provenance metadata once (path/description).
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

/// Fetches every stored concept with its content hash, the names of the bundles
/// containing it, and whether it is retired. The query yields one row per
/// containment (one row with no bundle for an uncontained concept), so rows are
/// folded by concept ID.
async fn fetch_stored_concepts(
    tx: &Transaction,
) -> Result<HashMap<String, StoredConcept>, DbError> {
    let answer = tx
        .query(
            r#"match $c isa concept, has concept-id $id;
                     try { $ct isa containment, links (container: $b, member: $c);
                           $b has bundle-name $bn; };
               fetch { "id": $id, "hash": $c.file-hash, "retired": $c.retired-at,
                       "bundle": $bn };"#,
        )
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
    if !answer.is_document_stream() {
        return Err(DbError::Query(
            "stored-concept read did not return documents".to_string(),
        ));
    }

    let mut out: HashMap<String, StoredConcept> = HashMap::new();
    let mut stream = answer.into_documents();
    while let Some(item) = stream.next().await {
        let doc = item.map_err(|e| DbError::Query(format!("stored-concept read: {e}")))?;
        let json = serde_json::to_value(doc.into_json())
            .map_err(|e| DbError::Query(format!("stored-concept document: {e}")))?;
        let id = json.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            DbError::Query(format!(
                "stored-concept document without a string id: {json}"
            ))
        })?;
        let entry = out.entry(id.to_string()).or_default();
        if let Some(hash) = json.get("hash").and_then(|v| v.as_str()) {
            entry.hash = Some(hash.to_string());
        }
        if json.get("retired").is_some_and(|v| !v.is_null()) {
            entry.retired = true;
        }
        if let Some(bundle) = json.get("bundle").and_then(|v| v.as_str()) {
            entry.bundles.push(bundle.to_string());
        }
    }
    for stored in out.values_mut() {
        stored.bundles.sort();
        stored.bundles.dedup();
    }
    Ok(out)
}

/// The string in column `col` of `row`, or a query error naming `what`.
fn row_string(row: &ConceptRow, col: &str, what: &str) -> Result<String, DbError> {
    row.get(col)
        .map_err(|e| DbError::Query(format!("{what}: column `{col}`: {e}")))?
        .ok_or_else(|| DbError::Query(format!("{what}: column `{col}` is unbound")))?
        .try_get_string()
        .map(str::to_string)
        .ok_or_else(|| DbError::Query(format!("{what}: column `{col}` is not a string")))
}

/// Every `(concept_id, relation_label)` pair, sorted and deduplicated, in which
/// one of the concepts `ids` plays a role in a relation of `surface`.
async fn belief_edge_references(
    tx: &Transaction,
    ids: &[&str],
    surface: &BeliefTierSurface,
) -> Result<Vec<(String, String)>, DbError> {
    let mut out = played_relations(tx, ids).await?;
    out.retain(|(_, relation)| surface.relations.contains(relation));
    Ok(out)
}

/// Every `(concept_id, relation_label)` pair, sorted and deduplicated, in which
/// one of the concepts `ids` plays a role in a relation of that exact type.
async fn played_relations(
    tx: &Transaction,
    ids: &[&str],
) -> Result<Vec<(String, String)>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let what = "played-relation read";
    let mut given = GivenRows::new(vec!["id".to_string()], ids.len());
    for id in ids {
        given
            .push_row(vec![str_entry(id)])
            .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    }
    let answer = tx
        .query_with_rows(
            "given $id: string; \
             match $c isa concept, has concept-id $cid; $cid == $id; \
                   $r isa! $rt, links ($c); let $rl = label($rt); \
             select $cid, $rl;",
            given,
        )
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    let mut out = Vec::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        out.push((
            row_string(&row, "cid", what)?,
            row_string(&row, "rl", what)?,
        ));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The labels of `concept` and every subtype of it in the schema.
async fn concept_subtypes(tx: &Transaction) -> Result<HashSet<String>, DbError> {
    let what = "concept subtype read";
    let answer = tx
        .query("match $t sub concept; let $l = label($t); select $l;")
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    let mut out = HashSet::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        out.insert(row_string(&row, "l", what)?);
    }
    Ok(out)
}

/// Checks that every written concept's `type_hint` is a valid label naming
/// `concept` or one of its subtypes. The subtype closure is read once, and only
/// when some hint names a type other than `concept`.
async fn check_type_hints(tx: &Transaction, concepts: &[&OkfConcept]) -> Result<(), DbError> {
    let hints: HashSet<&str> = concepts
        .iter()
        .map(|c| c.type_hint.as_deref().unwrap_or("concept"))
        .filter(|&t| t != "concept")
        .collect();
    if hints.is_empty() {
        return Ok(());
    }
    let concept_types = concept_subtypes(tx).await?;
    for type_name in hints {
        validate_schema_label(type_name)?;
        if !concept_types.contains(type_name) {
            return Err(DbError::Schema(format!(
                "type_hint {type_name:?} is not a subtype of `concept`"
            )));
        }
    }
    Ok(())
}

/// Garbage-collects concepts removed from the bundle. It is given only concepts
/// this bundle owns alone (see [`split_gc`]) that no belief edge references
/// (those are retired by [`retire_concepts`]). Deletes the OKF core relations
/// each concept plays (containment, hierarchy, concept-link, citation) so none
/// are left dangling, then the concept entity itself.
///
/// Only the OKF core relations are deleted explicitly; any other relation the
/// concept plays loses that player when the entity goes.
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

/// The manifest concepts already stored (those `plan` does not insert) whose
/// exact stored type differs from `type_hint` (`concept` when absent), keyed by
/// concept ID with their (stored type, new type).
async fn retyped_concepts(
    tx: &Transaction,
    concepts: &[OkfConcept],
    plan: &SyncPlan,
) -> Result<HashMap<String, (String, String)>, DbError> {
    let inserted: HashSet<usize> = plan.to_insert.iter().copied().collect();
    let stored: Vec<&OkfConcept> = concepts
        .iter()
        .enumerate()
        .filter(|(i, _)| !inserted.contains(i))
        .map(|(_, c)| c)
        .collect();
    let ids: Vec<&str> = stored.iter().map(|c| c.concept_id.as_str()).collect();
    let types = concept_types(tx, &ids).await?;
    let mut out = HashMap::new();
    for c in stored {
        let stored_type = types.get(&c.concept_id).ok_or_else(|| {
            DbError::Query(format!(
                "invariant violation: stored concept `{}` has no concept type row",
                c.concept_id
            ))
        })?;
        let new_type = c.type_hint.as_deref().unwrap_or("concept");
        if stored_type != new_type {
            out.insert(
                c.concept_id.clone(),
                (stored_type.clone(), new_type.to_string()),
            );
        }
    }
    Ok(out)
}

/// The exact type label of each concept `ids` names, keyed by concept ID.
async fn concept_types(tx: &Transaction, ids: &[&str]) -> Result<HashMap<String, String>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let what = "concept type read";
    let mut given = GivenRows::new(vec!["id".to_string()], ids.len());
    for id in ids {
        given
            .push_row(vec![str_entry(id)])
            .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    }
    let answer = tx
        .query_with_rows(
            "given $id: string; \
             match $c isa! $t, has concept-id $cid; $cid == $id; $t sub concept; \
                   let $tl = label($t); \
             select $cid, $tl;",
            given,
        )
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    let mut out = HashMap::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        out.insert(
            row_string(&row, "cid", what)?,
            row_string(&row, "tl", what)?,
        );
    }
    Ok(out)
}

/// Retires concepts dropped from the bundle that a belief edge still references.
/// Each keeps its entity, its `concept-id`, the attributes its type requires
/// (`required`, keyed by entity type label), its containment and every relation
/// of `surface`; every other attribute and relation it plays is deleted, and it
/// gains `retired-at` = `at`. A concept whose type `required` does not name is a
/// [`DbError::Schema`].
async fn retire_concepts(
    tx: &Transaction,
    ids: &[&str],
    surface: &BeliefTierSurface,
    required: &HashMap<String, BTreeSet<String>>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }

    // Attributes: one delete per concept type, keeping concept-id and the
    // attributes that type requires.
    let types = concept_types(tx, ids).await?;
    let mut by_type: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in ids {
        let type_name = types.get(*id).ok_or_else(|| {
            DbError::Query(format!(
                "invariant violation: concept `{id}` to retire has no concept type row"
            ))
        })?;
        by_type.entry(type_name.as_str()).or_default().push(id);
    }
    for (type_name, type_ids) in by_type {
        validate_schema_label(type_name)?;
        let keep = required.get(type_name).ok_or_else(|| {
            DbError::Schema(format!(
                "concept type `{type_name}` is missing from the schema's entity types"
            ))
        })?;
        let mut exclusions = String::from("not { $a isa concept-id; }; ");
        for label in keep.iter().filter(|l| *l != "concept-id") {
            validate_schema_label(label)?;
            exclusions.push_str(&format!("not {{ $a isa {label}; }}; "));
        }
        let query = format!(
            "given $id: string; \
             match $c isa! {type_name}, has concept-id $cid; $cid == $id; \
                   $c has $a; {exclusions}\
             delete has $a of $c;"
        );
        let rows = type_ids.iter().map(|id| vec![str_entry(id)]).collect();
        run_rows(tx, &query, &["id"], rows).await?;
    }

    // Relations: every one played except containment and the belief tier's.
    let rows = || -> Vec<Vec<GivenRowEntry>> { ids.iter().map(|id| vec![str_entry(id)]).collect() };
    let labels: BTreeSet<String> = played_relations(tx, ids)
        .await?
        .into_iter()
        .map(|(_, relation)| relation)
        .filter(|r| r != "containment" && !surface.relations.contains(r))
        .collect();
    for label in &labels {
        validate_schema_label(label)?;
        let query = format!(
            "given $id: string; \
             match $c isa concept, has concept-id $cid; $cid == $id; \
                   $r isa! {label}, links ($c); \
             delete $r;"
        );
        run_rows(tx, &query, &["id"], rows()).await?;
    }

    let stamped = ids
        .iter()
        .map(|id| vec![str_entry(id), dt_entry(&at)])
        .collect();
    run_rows(
        tx,
        "given $id: string, $at: datetime; \
         match $c isa concept, has concept-id $cid; $cid == $id; \
         insert $c has retired-at == $at;",
        &["id", "at"],
        stamped,
    )
    .await
}

/// Clears the file-derived content of changed concepts while preserving the
/// entity (and thus its containment, hierarchy, incoming links, and any
/// extension-created relations). Removes every non-key attribute and the
/// concept's OUTGOING links/citations; attributes are re-attached from the
/// current file by [`attach_concept_content`] and outgoing links/citations are
/// re-derived by [`create_links`] and [`create_citations`].
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

/// Inserts the entity and concept-id key for brand-new concepts, as the subtype
/// each one's `type_hint` names (already checked by [`check_type_hints`]). Their
/// content is attached separately by [`attach_concept_content`].
async fn insert_concept_entities(
    tx: &Transaction,
    concepts: &[&OkfConcept],
) -> Result<(), DbError> {
    let mut grouped: HashMap<&str, Vec<&OkfConcept>> = HashMap::new();
    for c in concepts {
        let type_name = c.type_hint.as_deref().unwrap_or("concept");
        grouped.entry(type_name).or_default().push(c);
    }
    for type_name in grouped.keys() {
        validate_schema_label(type_name)?;
    }

    for (type_name, type_concepts) in grouped {
        let rows: Vec<Vec<GivenRowEntry>> = type_concepts
            .iter()
            .map(|c| vec![str_entry(&c.concept_id)])
            .collect();

        let query = format!("given $id: string; insert $c isa {type_name}, has concept-id == $id;");
        run_rows(tx, &query, &["id"], rows).await?;
    }
    Ok(())
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
        c.engine
            .as_ref()
            .map(|e| e.as_str().to_string())
            .into_iter()
            .collect()
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

    // Multi-valued tags and granted host-capability scopes (S3); and the single
    // concatenated program (one code-block).
    attach_attr(tx, concepts, "tag", |c| c.tags.clone()).await?;
    attach_attr(tx, concepts, "scope", |c| c.scopes.clone()).await?;
    attach_attr(tx, concepts, "code-block", |c| {
        c.program.clone().into_iter().collect()
    })
    .await?;

    // Timestamp (datetime-typed).
    attach_datetime(tx, concepts, "timestamp", |c| c.timestamp).await?;

    // Extension-declared typed attributes (e.g. cyber-gecko's stix-id, ioc-value).
    attach_typed_attributes(tx, concepts).await?;

    Ok(())
}

/// Attaches extension-declared [`TypedAttribute`](crate::okf::types::TypedAttribute)s
/// as real typed `owns`, batched by (entity subtype, attribute name) so each
/// group is a single query. The `match` binds `$c` to the concept's actual
/// subtype (its `type_hint`) rather than the generic `concept`, because a typed
/// attribute is owned by that subtype and TypeDB would reject `has <attr>` on the
/// too-general `concept` type. Both interpolated labels are extension-supplied,
/// so each is validated as a TypeQL identifier (the subtype was checked against
/// `concept`'s closure by [`check_type_hints`]); values flow through the
/// parameterized `given` stage.
async fn attach_typed_attributes(
    tx: &Transaction,
    concepts: &[&OkfConcept],
) -> Result<(), DbError> {
    // Keyed on (entity subtype, attribute name); value carries the TypeQL value
    // type and the accumulated rows.
    type AttrGroup = (&'static str, Vec<Vec<GivenRowEntry>>);
    let mut groups: HashMap<(&str, &str), AttrGroup> = HashMap::new();

    for c in concepts {
        if c.typed_attributes.is_empty() {
            continue;
        }
        let type_name = c.type_hint.as_deref().unwrap_or("concept");
        for attr in &c.typed_attributes {
            let (val_type, val) = match &attr.value {
                TypedAttrValue::String(s) => ("string", str_entry(s)),
                TypedAttrValue::Bool(b) => ("boolean", bool_entry(*b)),
                TypedAttrValue::Datetime(dt) => ("datetime", dt_entry(dt)),
            };
            groups
                .entry((type_name, attr.name.as_str()))
                .or_insert((val_type, Vec::new()))
                .1
                .push(vec![str_entry(&c.concept_id), val]);
        }
    }

    for (type_name, attr) in groups.keys() {
        validate_schema_label(type_name)?;
        validate_schema_label(attr)?;
    }

    for ((type_name, attr), (val_type, rows)) in groups {
        let query = format!(
            "given $id: string, $val: {val_type}; \
             match $c isa {type_name}, has concept-id $cid; $cid == $id; \
             insert $c has {attr} == $val;"
        );
        run_rows(tx, &query, &["id", "val"], rows).await?;
    }
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

/// The `(id, name)` given rows pairing each concept ID with the bundle name.
fn containment_rows(bundle_name: &str, ids: &[&str]) -> Vec<Vec<GivenRowEntry>> {
    ids.iter()
        .map(|id| vec![str_entry(id), str_entry(bundle_name)])
        .collect()
}

/// Contains the given concepts in the bundle named `bundle_name`, skipping any
/// already contained by it.
async fn create_containment(
    tx: &Transaction,
    bundle_name: &str,
    ids: &[&str],
) -> Result<(), DbError> {
    let rows = containment_rows(bundle_name, ids);
    run_rows(
        tx,
        "given $id: string, $name: string; \
         match $b isa bundle, has bundle-name $bn; $bn == $name; \
               $c isa concept, has concept-id $cid; $cid == $id; \
               not { $x isa containment, links (container: $b, member: $c); }; \
         insert $ct isa containment, links (container: $b, member: $c);",
        &["id", "name"],
        rows,
    )
    .await
}

/// Removes only the containment between the bundle named `bundle_name` and each
/// given concept. Used in place of [`delete_concepts_gc`] for a concept that
/// another bundle also contains: the concept, its other containments and its
/// links stay.
async fn release_containment(
    tx: &Transaction,
    bundle_name: &str,
    ids: &[&str],
) -> Result<(), DbError> {
    let rows = containment_rows(bundle_name, ids);
    run_rows(
        tx,
        "given $id: string, $name: string; \
         match $b isa bundle, has bundle-name $bn; $bn == $name; \
               $c isa concept, has concept-id $cid; $cid == $id; \
               $ct isa containment, links (container: $b, member: $c); \
         delete $ct;",
        &["id", "name"],
        rows,
    )
    .await
}

/// The concept IDs of `manifest`. When links and hierarchy are written, these
/// are exactly the live concepts this bundle contains: a manifest ID held outside
/// the bundle has already refused the sync as a collision, and every concept the
/// bundle dropped has been deleted, released or retired.
fn bundle_ids(manifest: &OkfBundle) -> HashSet<&str> {
    manifest
        .concepts
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect()
}

/// Creates directory-hierarchy edges between concepts of this bundle. An edge
/// whose parent or child is not in this bundle stays unresolved.
async fn create_hierarchy(tx: &Transaction, manifest: &OkfBundle) -> Result<(), DbError> {
    let ids = bundle_ids(manifest);
    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .hierarchy
        .iter()
        .filter(|e| ids.contains(e.parent_id.as_str()) && ids.contains(e.child_id.as_str()))
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

/// Creates internal concept-to-concept links within this bundle. A link whose
/// source or target is not in this bundle stays unresolved (OKF v0.2 §6.1).
/// Returns the number of links attempted: those whose ends are both in this
/// bundle.
async fn create_links(tx: &Transaction, manifest: &OkfBundle) -> Result<usize, DbError> {
    let ids = bundle_ids(manifest);
    // One write must not carry two rows for one (source, target) pair: the `not`
    // guard cannot see relations inserted by the same query, so it misses rows
    // for one pair that differ in link text and would insert a link for each.
    // The first occurrence in document order keeps its link text.
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .links
        .iter()
        .filter(|l| ids.contains(l.source_id.as_str()) && ids.contains(l.target_id.as_str()))
        .filter(|l| seen.insert((l.source_id.as_str(), l.target_id.as_str())))
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

    // One row per (citing concept, URL); the first occurrence keeps its text.
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let rows: Vec<Vec<GivenRowEntry>> = manifest
        .citations
        .iter()
        .filter(|c| seen.insert((c.source_id.as_str(), c.target_url.as_str())))
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
            type_hint: None,
            title: None,
            description: None,
            resource_uri: None,
            tags: vec![],
            timestamp: None,
            body: String::new(),
            program: None,
            extra_metadata: HashMap::new(),
            typed_attributes: vec![],
            file_hash: hash.to_string(),
            source_path: format!("{id}.md"),
            consumes: vec![],
            produces: vec![],
            engine: None,
            scopes: vec![],
            timeout_ms: None,
        }
    }

    /// Invariants 1/2/8 guardrail: the record path must never reach into the
    /// belief-write path or the semantic accelerator. This module names none of
    /// those types (the guardrail doc-comment above is worded to avoid them, so
    /// this is a real check, not a tautology).
    #[test]
    fn syncer_stays_on_the_record_path() {
        // Scan only the production code (everything before this `#[cfg(test)]`
        // block) so the test's own reference strings don't trip it.
        let full = include_str!("bundle.rs");
        let prod = full
            .split("#[cfg(test)]")
            .next()
            .expect("source has a production section");
        // Assemble the needles at runtime so they don't appear as literals in the
        // scanned production span.
        let forbidden = [
            format!("{}{}", "Epistemic", "Writer"),
            format!("{}{}", "Epistemic", "Reader"),
            "Embedder".to_string(),
            format!("{}{}", "Semantic", "Index"),
        ];
        for needle in forbidden {
            assert!(
                !prod.contains(&needle),
                "syncer must not reference `{needle}` — records bypass the belief/accelerator path"
            );
        }
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

    fn stored(hash: &str, bundles: &[&str]) -> StoredConcept {
        StoredConcept {
            hash: Some(hash.to_string()),
            bundles: bundles.iter().map(|b| b.to_string()).collect(),
            retired: false,
        }
    }

    fn three_holders() -> HashMap<String, StoredConcept> {
        HashMap::from([
            ("a".to_string(), stored("ha", &["alpha"])),
            ("b".to_string(), stored("hb", &["beta"])),
            ("c".to_string(), stored("hc", &[])),
        ])
    }

    #[test]
    fn owned_hashes_keeps_only_this_bundles_concepts() {
        let owned = owned_hashes("alpha", &three_holders());
        let keys: Vec<&String> = owned.keys().collect();
        assert_eq!(keys, vec!["a"]);
        assert_eq!(owned["a"], Some("ha".to_string()));
    }

    #[test]
    fn find_collisions_reports_other_and_uncontained_holders() {
        let manifest = vec![concept("b", "x"), concept("c", "x"), concept("d", "x")];
        assert_eq!(
            find_collisions("alpha", &manifest, &three_holders()),
            vec![
                ("b".to_string(), Some("beta".to_string())),
                ("c".to_string(), None),
            ]
        );
    }

    #[test]
    fn concept_contained_by_two_bundles_is_owned_by_both() {
        let all = HashMap::from([("s".to_string(), stored("hs", &["alpha", "beta"]))]);
        let manifest = vec![concept("s", "hs")];
        for bundle in ["alpha", "beta"] {
            assert!(owned_hashes(bundle, &all).contains_key("s"), "{bundle}");
            assert!(
                find_collisions(bundle, &manifest, &all).is_empty(),
                "{bundle}"
            );
        }
        assert_eq!(
            find_collisions("gamma", &manifest, &all),
            vec![("s".to_string(), Some("alpha".to_string()))]
        );
    }

    #[test]
    fn gc_releases_a_shared_concept_and_deletes_an_exclusive_one() {
        let all = HashMap::from([
            ("e".to_string(), stored("he", &["alpha"])),
            ("s".to_string(), stored("hs", &["alpha", "beta"])),
        ]);
        assert_eq!(
            split_gc("alpha", &["e".to_string(), "s".to_string()], &all),
            (vec!["e".to_string()], vec!["s".to_string()])
        );
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
