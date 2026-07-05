//! Bundle syncer: maps parsed OKF bundles to TypeDB entities and relations.
//!
//! Port of Tyke's `syncer.py` (`BundleSyncer`).

use futures_util::StreamExt;
use tracing::{debug, info};

use crate::db::router::{DbError, TypeDbRouter};
use crate::okf::types::{OkfBundle, OkfConcept};

/// Results of a bundle sync operation.
#[derive(Debug, Default)]
pub struct SyncResult {
    pub concepts_inserted: usize,
    pub concepts_updated: usize,
    pub concepts_skipped: usize,
    pub links_created: usize,
    pub citations_created: usize,
}

/// Escapes strings for safe embedding in TypeQL queries.
///
/// Port of Tyke's `escape_tql()`.
pub fn escape_tql(val: &str) -> String {
    val.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Syncs an OKF bundle into TypeDB.
///
/// Port of Tyke's `BundleSyncer.sync()`.
pub async fn sync_bundle(
    db: &mut TypeDbRouter,
    manifest: &OkfBundle,
) -> Result<SyncResult, DbError> {
    let tx = db.begin_write().await?;
    let mut result = SyncResult::default();

    // 1. Upsert bundle
    tx.query(&format!(
        r#"put $b isa bundle, has bundle-path "{}", has bundle-name "{}";"#,
        escape_tql(&manifest.bundle_path),
        escape_tql(&manifest.bundle_name)
    ))
    .await
    .map_err(|e| DbError::Query(e.to_string()))?;

    // 2. Sync concepts
    for concept in &manifest.concepts {
        // Check existing hash
        let query = format!(
            r#"match $c isa concept, has concept-id "{}", has file-hash $h; fetch {{"hash": $h}};"#,
            escape_tql(&concept.concept_id)
        );

        let answer = tx
            .query(&query)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        // Check if concept already exists by examining the query answer
        let mut concept_exists = false;
        if let typedb_driver::answer::QueryAnswer::ConceptDocumentStream(_, mut stream) = answer {
            if stream.next().await.is_some() {
                concept_exists = true;
            }
        }

        if concept_exists {
            // Hash changed — delete old entity and re-insert
            tx.query(&format!(
                r#"match $c isa concept, has concept-id "{}"; delete $c;"#,
                escape_tql(&concept.concept_id)
            ))
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

            result.concepts_updated += 1;
        } else {
            result.concepts_inserted += 1;
        }

        // Insert the concept
        insert_concept(&tx, concept).await?;

        // 3. Create containment relation
        tx.query(&format!(
            "match\n  $b isa bundle, has bundle-path \"{}\";\n  $c isa concept, has concept-id \"{}\";\n  not {{ $cont isa containment, links (container: $b, member: $c); }};\ninsert\n  $new_cont isa containment, links (container: $b, member: $c);",
            escape_tql(&manifest.bundle_path),
            escape_tql(&concept.concept_id)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
    }

    // 4. Create hierarchy relations
    for edge in &manifest.hierarchy {
        if edge.parent_id.is_empty() {
            continue;
        }

        tx.query(&format!(
            "match\n  $p isa concept, has concept-id \"{}\";\n  $c isa concept, has concept-id \"{}\";\n  not {{ $h isa hierarchy, links (parent: $p, child: $c); }};\ninsert\n  $new_h isa hierarchy, links (parent: $p, child: $c);",
            escape_tql(&edge.parent_id),
            escape_tql(&edge.child_id)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
    }

    // 5. Create internal links
    for link in &manifest.links {
        // We use insert if not exists to establish the link, and then put to ensure link-text is up to date
        tx.query(&format!(
            "match\n  $src isa concept, has concept-id \"{}\";\n  $tgt isa concept, has concept-id \"{}\";\n  not {{ $l isa concept-link, links (source: $src, target: $tgt); }};\ninsert\n  $new_l isa concept-link, links (source: $src, target: $tgt), has link-text \"{}\";",
            escape_tql(&link.source_id),
            escape_tql(&link.target_id),
            escape_tql(&link.link_text)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        tx.query(&format!(
            "match\n  $src isa concept, has concept-id \"{}\";\n  $tgt isa concept, has concept-id \"{}\";\n  $l isa concept-link, links (source: $src, target: $tgt);\nput\n  $l has link-text \"{}\";",
            escape_tql(&link.source_id),
            escape_tql(&link.target_id),
            escape_tql(&link.link_text)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        result.links_created += 1;
    }

    // 6. Create citations
    for cit in &manifest.citations {
        tx.query(&format!(
            r#"put $ext isa external-resource, has target-url "{}";"#,
            escape_tql(&cit.target_url)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        tx.query(&format!(
            "match\n  $src isa concept, has concept-id \"{}\";\n  $ext isa external-resource, has target-url \"{}\";\n  not {{ $cit isa citation, links (citing-concept: $src, cited-resource: $ext); }};\ninsert\n  $new_cit isa citation, links (citing-concept: $src, cited-resource: $ext), has link-text \"{}\";",
            escape_tql(&cit.source_id),
            escape_tql(&cit.target_url),
            escape_tql(&cit.link_text)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        tx.query(&format!(
            "match\n  $src isa concept, has concept-id \"{}\";\n  $ext isa external-resource, has target-url \"{}\";\n  $cit isa citation, links (citing-concept: $src, cited-resource: $ext);\nput\n  $cit has link-text \"{}\";",
            escape_tql(&cit.source_id),
            escape_tql(&cit.target_url),
            escape_tql(&cit.link_text)
        ))
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        result.citations_created += 1;
    }

    // 7. Graph Garbage Collection (Prune orphaned concepts)
    let active_ids: std::collections::HashSet<String> = manifest
        .concepts
        .iter()
        .map(|c| c.concept_id.clone())
        .collect();

    let gc_query = format!(
        "match\n  $b isa bundle, has bundle-path \"{}\";\n  $c isa concept, has concept-id $id;\n  $cont isa containment, links (container: $b, member: $c);\nfetch {{\"id\": $id}};",
        escape_tql(&manifest.bundle_path)
    );

    let gc_answer = tx
        .query(&gc_query)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
    if gc_answer.is_document_stream() {
        let mut stream = gc_answer.into_documents();
        while let Some(Ok(doc)) = stream.next().await {
            let json_str = doc.into_json().to_string();
            let json: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_default();
            if let Some(id) = json
                .as_object()
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
            {
                if !active_ids.contains(id) {
                    debug!(concept_id = %id, "Deleting orphaned concept");
                    let delete_query = format!(
                        "match $c isa concept, has concept-id \"{}\"; delete $c;",
                        escape_tql(id)
                    );
                    tx.query(&delete_query)
                        .await
                        .map_err(|e| DbError::Query(e.to_string()))?;
                }
            }
        }
    }

    tx.commit()
        .await
        .map_err(|e| DbError::Transaction(e.to_string()))?;

    info!(
        inserted = result.concepts_inserted,
        updated = result.concepts_updated,
        skipped = result.concepts_skipped,
        links = result.links_created,
        citations = result.citations_created,
        "Bundle sync complete"
    );

    Ok(result)
}

/// Inserts a single concept entity with all its attributes.
///
/// Port of Tyke's `BundleSyncer._insert_concept()`.
async fn insert_concept(
    tx: &typedb_driver::Transaction,
    concept: &OkfConcept,
) -> Result<(), DbError> {
    let mut parts = vec![
        format!(
            r#"insert $c isa concept, has concept-id "{}""#,
            escape_tql(&concept.concept_id)
        ),
        format!(
            r#"has concept-type "{}""#,
            escape_tql(&concept.concept_type)
        ),
        format!(r#"has body "{}""#, escape_tql(&concept.body)),
        format!(r#"has file-hash "{}""#, escape_tql(&concept.file_hash)),
    ];

    if let Some(title) = &concept.title {
        parts.push(format!(r#"has title "{}""#, escape_tql(title)));
    }
    if let Some(desc) = &concept.description {
        parts.push(format!(r#"has description "{}""#, escape_tql(desc)));
    }
    if let Some(uri) = &concept.resource_uri {
        parts.push(format!(r#"has resource-uri "{}""#, escape_tql(uri)));
    }

    for tag in &concept.tags {
        parts.push(format!(r#"has tag "{}""#, escape_tql(tag)));
    }

    for block in &concept.code_blocks {
        parts.push(format!(r#"has code-block "{}""#, escape_tql(block)));
    }

    if let Some(engine) = &concept.engine {
        let engine_str = match engine {
            crate::okf::types::ScriptEngine::Rhai => "rhai",
            crate::okf::types::ScriptEngine::QuickJs => "quickjs",
        };
        parts.push(format!(r#"has engine "{}""#, escape_tql(engine_str)));
    }

    if !concept.extra_metadata.is_empty() {
        let meta_str = serde_json::to_string(&concept.extra_metadata).unwrap_or_default();
        parts.push(format!(r#"has metadata-json "{}""#, escape_tql(&meta_str)));
    }

    if let Some(ts) = &concept.timestamp {
        let ts_str = ts.format("%Y-%m-%dT%H:%M:%S").to_string();
        parts.push(format!("has timestamp {}", ts_str));
    }

    let query = parts.join(",\n") + ";";
    debug!(concept_id = %concept.concept_id, "Inserting concept");

    tx.query(&query)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

    Ok(())
}
