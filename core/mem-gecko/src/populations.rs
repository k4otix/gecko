//! Population-membership materialization planning (SPEC-W2-W3 §3.5).
//!
//! A `population-member` row is a materialized cache of one concept's membership
//! in a population, over the window it held (`valid-from`/`valid-to`) and with a
//! freshness marker (`computed-at`). Given the rows already in the graph and the
//! member set the caller computed at time `at`, [`plan`] decides which rows to
//! open, close and refresh so that:
//!
//! - an unchanged member keeps its `valid-from` and only advances `computed-at`;
//! - a departed member's window closes (`valid-to` ← `at`) **and** its
//!   `computed-at` advances — both markers, never one;
//! - a member that left and rejoins gets a fresh epoch row; the closed row stays.
//!
//! The planner is pure; [`MemWriter::materialize_population`](crate::MemWriter::materialize_population)
//! reads the rows, calls it, and applies the result in one transaction.

use std::collections::BTreeSet;

use gecko_extension_api::{ConceptId, DateTime, EpistemicError};

/// One existing `population-member` row, as read from the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    /// The member concept.
    pub member: ConceptId,
    /// When the membership began holding (absent on rows written before windows).
    pub valid_from: Option<DateTime>,
    /// When the membership stopped holding; `None` means the row is open.
    pub valid_to: Option<DateTime>,
    /// When the row was last materialized.
    pub computed_at: Option<DateTime>,
}

/// What one materialization pass changes, by member.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterializationPlan {
    /// Members with no open row: a new epoch row opens at `at`.
    pub open: Vec<ConceptId>,
    /// Members whose open row closes at `at`.
    pub close: Vec<ConceptId>,
    /// Members whose open row stays open; only `computed-at` advances.
    pub refresh: Vec<ConceptId>,
}

/// Counts of the rows a materialization pass opened, closed and refreshed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterializeReport {
    /// New epoch rows opened.
    pub opened: usize,
    /// Open rows whose window was closed.
    pub closed: usize,
    /// Open rows whose `computed-at` advanced with membership unchanged.
    pub refreshed: usize,
}

impl MaterializationPlan {
    /// The report this plan produces once applied.
    pub fn report(&self) -> MaterializeReport {
        MaterializeReport {
            opened: self.open.len(),
            closed: self.close.len(),
            refreshed: self.refresh.len(),
        }
    }
}

/// Plans one materialization pass of a population at time `at`.
///
/// `requested` is the full member set the caller evaluated; duplicates collapse.
/// Errors on an empty concept-id, and when `at` precedes any timestamp already on
/// the population's rows — a stale pass must not overwrite a fresher one, or close
/// a window before it opened.
pub fn plan(
    rows: &[MemberRow],
    requested: &[ConceptId],
    at: DateTime,
) -> Result<MaterializationPlan, EpistemicError> {
    if let Some(bad) = requested.iter().find(|c| c.0.trim().is_empty()) {
        return Err(EpistemicError::InvalidInput(format!(
            "materialize_population: empty member concept-id {bad:?}"
        )));
    }
    if let Some(latest) = rows
        .iter()
        .flat_map(|r| [r.valid_from, r.valid_to, r.computed_at])
        .flatten()
        .max()
        && at < latest
    {
        return Err(EpistemicError::InvalidInput(format!(
            "materialize_population: pass time {at} precedes the population's latest \
             recorded time {latest}"
        )));
    }

    let wanted: BTreeSet<&ConceptId> = requested.iter().collect();
    let open_now: BTreeSet<&ConceptId> = rows
        .iter()
        .filter(|r| r.valid_to.is_none())
        .map(|r| &r.member)
        .collect();

    Ok(MaterializationPlan {
        open: wanted.difference(&open_now).map(|c| (*c).clone()).collect(),
        close: open_now.difference(&wanted).map(|c| (*c).clone()).collect(),
        refresh: open_now
            .intersection(&wanted)
            .map(|c| (*c).clone())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn c(id: &str) -> ConceptId {
        ConceptId::new(id)
    }

    fn open_row(id: &str, from: &str) -> MemberRow {
        MemberRow {
            member: c(id),
            valid_from: Some(t(from)),
            valid_to: None,
            computed_at: Some(t(from)),
        }
    }

    fn closed_row(id: &str, from: &str, to: &str) -> MemberRow {
        MemberRow {
            member: c(id),
            valid_from: Some(t(from)),
            valid_to: Some(t(to)),
            computed_at: Some(t(to)),
        }
    }

    const T0: &str = "2026-01-01T00:00:00Z";
    const T1: &str = "2026-02-01T00:00:00Z";
    const T2: &str = "2026-03-01T00:00:00Z";

    #[test]
    fn first_pass_opens_every_member() {
        let p = plan(&[], &[c("a"), c("b")], t(T0)).unwrap();
        assert_eq!(p.open, vec![c("a"), c("b")]);
        assert!(p.close.is_empty() && p.refresh.is_empty());
    }

    #[test]
    fn unchanged_membership_only_refreshes() {
        let rows = [open_row("a", T0), open_row("b", T0)];
        let p = plan(&rows, &[c("b"), c("a")], t(T1)).unwrap();
        assert_eq!(p.refresh, vec![c("a"), c("b")]);
        assert!(p.open.is_empty() && p.close.is_empty());
    }

    #[test]
    fn departure_closes_and_arrival_opens() {
        let rows = [open_row("a", T0), open_row("b", T0)];
        let p = plan(&rows, &[c("b"), c("c")], t(T1)).unwrap();
        assert_eq!(p.close, vec![c("a")]);
        assert_eq!(p.refresh, vec![c("b")]);
        assert_eq!(p.open, vec![c("c")]);
    }

    #[test]
    fn rejoin_opens_a_new_epoch_beside_the_closed_row() {
        let rows = [closed_row("a", T0, T1)];
        let p = plan(&rows, &[c("a")], t(T2)).unwrap();
        assert_eq!(p.open, vec![c("a")]);
        assert!(p.close.is_empty() && p.refresh.is_empty());
    }

    #[test]
    fn empty_request_closes_every_open_row() {
        let rows = [open_row("a", T0), closed_row("b", T0, T1)];
        let p = plan(&rows, &[], t(T2)).unwrap();
        assert_eq!(p.close, vec![c("a")]);
        assert!(p.open.is_empty() && p.refresh.is_empty());
    }

    #[test]
    fn duplicate_requests_collapse() {
        let p = plan(&[], &[c("a"), c("a")], t(T0)).unwrap();
        assert_eq!(p.open, vec![c("a")]);
    }

    #[test]
    fn same_instant_pass_is_allowed() {
        let rows = [open_row("a", T1)];
        assert!(plan(&rows, &[c("a")], t(T1)).is_ok());
    }

    #[test]
    fn stale_pass_is_rejected() {
        let rows = [closed_row("a", T0, T2)];
        let err = plan(&rows, &[c("a")], t(T1)).unwrap_err();
        assert!(matches!(err, EpistemicError::InvalidInput(_)));
    }

    #[test]
    fn empty_concept_id_is_rejected() {
        let err = plan(&[], &[c(" ")], t(T0)).unwrap_err();
        assert!(matches!(err, EpistemicError::InvalidInput(_)));
    }

    #[test]
    fn row_without_window_counts_as_open() {
        let rows = [MemberRow {
            member: c("a"),
            valid_from: None,
            valid_to: None,
            computed_at: None,
        }];
        let p = plan(&rows, &[], t(T0)).unwrap();
        assert_eq!(p.close, vec![c("a")]);
    }
}
