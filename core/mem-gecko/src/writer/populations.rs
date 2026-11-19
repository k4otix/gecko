//! The populations writer: the sole writer of `population-member` rows
//! (SPEC-W2-W3 §3.5). Planning is pure and lives in [`crate::populations`].

use std::collections::HashSet;
use std::sync::Arc;

use gecko_extension_api::{ConceptId, DateTime, EpistemicError};
use serde_json::Value;

use super::{MemWriter, Result, dedup_ids, parse_dt, str_field};
use crate::populations::{self, MaterializeReport, MemberRow};
use crate::tql;

/// How far past the host clock a population pass time may be. A pass stamped
/// further ahead would make every later pass look stale.
const MAX_PASS_CLOCK_SKEW: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

impl MemWriter {
    /// Creates the population keyed by `spec_hash` with its defining spec. Returns
    /// `true` when created and `false` when it already exists with the same spec;
    /// an existing population with a different spec is an error, since `spec-hash`
    /// identifies one cohort definition.
    pub async fn define_population(
        &self,
        spec_hash: &str,
        spec_dialect: &str,
        spec_text: &str,
    ) -> Result<bool> {
        for (field, v) in [
            ("spec_hash", spec_hash),
            ("spec_dialect", spec_dialect),
            ("spec_text", spec_text),
        ] {
            if v.trim().is_empty() {
                return Err(EpistemicError::InvalidInput(format!(
                    "define_population: {field} is empty"
                )));
            }
        }
        let lock = self.population_lock(spec_hash);
        let _guard = lock.lock().await;
        if self
            .population_spec_matches(spec_hash, spec_dialect, spec_text)
            .await?
        {
            return Ok(false);
        }
        let written = self
            .graph
            .write(&[tql::define_population_op(
                spec_hash,
                spec_dialect,
                spec_text,
            )])
            .await;
        match written {
            Ok(()) => Ok(true),
            // Another writer created it between the read and the write: report it
            // as existing when the specs agree.
            Err(e) => match self
                .population_spec_matches(spec_hash, spec_dialect, spec_text)
                .await
            {
                Ok(true) => Ok(false),
                Ok(false) | Err(_) => Err(e),
            },
        }
    }

    /// `true` when population `spec_hash` exists with this spec, `false` when it
    /// does not exist, and an error when it exists with a different spec.
    async fn population_spec_matches(
        &self,
        spec_hash: &str,
        spec_dialect: &str,
        spec_text: &str,
    ) -> Result<bool> {
        let (q, v, r) = tql::population_spec_read(spec_hash);
        match self.graph.read(&q, &v, &r).await?.first() {
            None => Ok(false),
            Some(d)
                if str_field(d, "dialect") == spec_dialect && str_field(d, "text") == spec_text =>
            {
                Ok(true)
            }
            Some(_) => Err(EpistemicError::InvalidInput(format!(
                "define_population: population {spec_hash:?} exists with a different spec"
            ))),
        }
    }

    /// The lock serializing this writer's passes over population `spec_hash`.
    fn population_lock(&self, spec_hash: &str) -> Arc<futures_util::lock::Mutex<()>> {
        self.population_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(spec_hash.to_string())
            .or_default()
            .clone()
    }

    /// Materializes the membership of population `spec_hash` as evaluated by the
    /// caller at `at`: `members` is the complete member set. New members open an
    /// epoch row, departed members' rows close (`valid-to` and `computed-at` both
    /// set to `at`), and unchanged members advance only `computed-at`. All row
    /// changes commit in one transaction, each guarded against a concurrent pass.
    ///
    /// Errors when the population does not exist, when a member is not a stored
    /// `concept`, when `at` precedes a time already recorded on the population, or
    /// when `at` is ahead of the host clock by more than a few minutes.
    pub async fn materialize_population(
        &self,
        spec_hash: &str,
        members: &[ConceptId],
        at: DateTime,
    ) -> Result<MaterializeReport> {
        check_pass_time(at)?;
        let lock = self.population_lock(spec_hash);
        let _guard = lock.lock().await;
        let rows = self.population_rows(spec_hash).await?;
        let plan = populations::plan(&rows, members, at)?;
        self.require_concepts(&plan.open).await?;
        self.graph
            .write(&[
                tql::open_membership_op(spec_hash, &plan.open, at),
                tql::close_membership_op(spec_hash, &plan.close, at),
                tql::refresh_membership_op(spec_hash, &plan.refresh, at),
            ])
            .await?;
        Ok(plan.report())
    }

    /// Closes `member`'s open row in population `spec_hash` at `at` (the member was
    /// superseded or otherwise left), setting `valid-to` and advancing
    /// `computed-at`. Other members' rows are untouched. Returns `false` when the
    /// member has no open row. `at` is bounded as for
    /// [`materialize_population`](Self::materialize_population).
    pub async fn close_population_membership(
        &self,
        spec_hash: &str,
        member: &ConceptId,
        at: DateTime,
    ) -> Result<bool> {
        check_pass_time(at)?;
        let lock = self.population_lock(spec_hash);
        let _guard = lock.lock().await;
        let rows = self.population_rows(spec_hash).await?;
        let staying: Vec<ConceptId> = rows
            .iter()
            .filter(|r| r.valid_to.is_none() && &r.member != member)
            .map(|r| r.member.clone())
            .collect();
        let plan = populations::plan(&rows, &staying, at)?;
        if !plan.close.contains(member) {
            return Ok(false);
        }
        self.graph
            .write(&[tql::close_membership_op(
                spec_hash,
                std::slice::from_ref(member),
                at,
            )])
            .await?;
        Ok(true)
    }

    /// Every `population-member` row of `spec_hash`. Errors when the population
    /// does not exist.
    async fn population_rows(&self, spec_hash: &str) -> Result<Vec<MemberRow>> {
        if spec_hash.trim().is_empty() {
            return Err(EpistemicError::InvalidInput(
                "population: spec_hash is empty".into(),
            ));
        }
        let (q, v, r) = tql::population_spec_read(spec_hash);
        if self.graph.read(&q, &v, &r).await?.is_empty() {
            return Err(EpistemicError::InvalidInput(format!(
                "population {spec_hash:?} does not exist"
            )));
        }
        let (q, v, r) = tql::population_rows_read(spec_hash);
        let docs = self.graph.read(&q, &v, &r).await?;
        docs.iter()
            .map(|d| {
                Ok(MemberRow {
                    member: ConceptId(str_field(d, "id")),
                    valid_from: stored_dt_field(d, "vf")?,
                    valid_to: stored_dt_field(d, "vt")?,
                    computed_at: stored_dt_field(d, "ca")?,
                })
            })
            .collect()
    }

    /// Errors unless every id in `ids` is a stored `concept`.
    async fn require_concepts(&self, ids: &[ConceptId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let (q, v, r) = tql::existing_concepts_read(ids);
        let found: HashSet<String> = dedup_ids(&self.graph.read(&q, &v, &r).await?)
            .into_iter()
            .collect();
        let missing: Vec<&str> = ids
            .iter()
            .map(|c| c.0.as_str())
            .filter(|c| !found.contains(*c))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(EpistemicError::InvalidInput(format!(
                "population member(s) are not stored concepts: {missing:?}"
            )))
        }
    }
}

/// A stored datetime field: `None` when absent, an error when present but
/// unparseable (reading it as absent would turn a closed window open).
fn stored_dt_field(d: &Value, k: &str) -> Result<Option<DateTime>> {
    match d.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().and_then(parse_dt).map(Some).ok_or_else(|| {
            EpistemicError::Storage(format!("unparseable stored datetime {k}: {v}"))
        }),
    }
}

/// Rejects a population pass time further ahead of the host clock than
/// [`MAX_PASS_CLOCK_SKEW`].
fn check_pass_time(at: DateTime) -> Result<()> {
    let limit = chrono::Utc::now() + MAX_PASS_CLOCK_SKEW;
    if at > limit {
        return Err(EpistemicError::InvalidInput(format!(
            "population pass time {at} is ahead of the host clock"
        )));
    }
    Ok(())
}
