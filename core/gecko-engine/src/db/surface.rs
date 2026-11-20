//! The belief-tier surface and fingerprint.
//!
//! The **surface** is read from the live schema: the reflexive `sub` closure of a
//! root type, and every relation owning a role that is played only by that
//! closure. The **fingerprint** counts, for every surface label, its instances,
//! its owned attributes and (for relations) its role players, so two
//! fingerprints of one surface show whether anything in the belief tier changed.
//! Every query here is a read.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use futures_util::StreamExt;
use typedb_driver::Transaction;
use typedb_driver::answer::ConceptRow;

use crate::db::router::DbError;
use crate::db::validate_schema_label;

/// The belief-tier types of a schema: the root's subtype closure and the
/// relations whose roles only that closure plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeliefTierSurface {
    /// The reflexive `sub` closure of the root type.
    pub entities: BTreeSet<String>,
    /// Every relation owning a role played by at least one type and only by
    /// types in `entities`.
    pub relations: BTreeSet<String>,
}

/// One count a fingerprint records for a surface label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BeliefTierMeasure {
    /// Instances of the label, subtypes included.
    Instances,
    /// Attributes owned by instances of the label.
    Attributes,
    /// Role players of instances of a relation label.
    Links,
}

impl fmt::Display for BeliefTierMeasure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Instances => "instances",
            Self::Attributes => "attributes",
            Self::Links => "links",
        })
    }
}

/// A count that differs between two fingerprints of one surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeliefTierChange {
    pub label: String,
    pub measure: BeliefTierMeasure,
    pub before: u64,
    pub after: u64,
}

impl BeliefTierChange {
    /// The signed difference `after - before`.
    pub fn delta(&self) -> i64 {
        (i128::from(self.after) - i128::from(self.before)) as i64
    }
}

impl fmt::Display for BeliefTierChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}→{}",
            self.label, self.measure, self.before, self.after
        )
    }
}

/// The counts of every surface label, keyed by label and measure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BeliefTierFingerprint {
    pub counts: BTreeMap<(String, BeliefTierMeasure), u64>,
}

impl BeliefTierFingerprint {
    /// Every key whose count differs between `self` and `after`, in key order. A
    /// key present on only one side counts as 0 on the other.
    pub fn changes_to(&self, after: &Self) -> Vec<BeliefTierChange> {
        let keys: BTreeSet<&(String, BeliefTierMeasure)> =
            self.counts.keys().chain(after.counts.keys()).collect();
        keys.into_iter()
            .filter_map(|key| {
                let before = self.counts.get(key).copied().unwrap_or(0);
                let now = after.counts.get(key).copied().unwrap_or(0);
                (before != now).then(|| BeliefTierChange {
                    label: key.0.clone(),
                    measure: key.1,
                    before,
                    after: now,
                })
            })
            .collect()
    }
}

/// Whether a driver error is TypeDB's unknown-type-label error.
fn is_unknown_label(e: &typedb_driver::Error) -> bool {
    let code = e.code();
    if code == "INF2" || code == "INF3" {
        return true;
    }
    let text = e.to_string();
    text.contains("[INF2]") || text.contains("[INF3]")
}

/// Maps an error from the closure read: an unknown `root` is a schema error,
/// anything else a query error.
fn closure_error(root: &str, e: typedb_driver::Error) -> DbError {
    if is_unknown_label(&e) {
        DbError::Schema(format!(
            "belief-tier root type `{root}` is not in the schema: {e}"
        ))
    } else {
        DbError::Query(format!("belief-tier closure of `{root}`: {e}"))
    }
}

/// The string in column `col` of `row`, or a query error naming `what`.
fn string_col(row: &ConceptRow, col: &str, what: &str) -> Result<String, DbError> {
    row.get(col)
        .map_err(|e| DbError::Query(format!("{what}: column `{col}`: {e}")))?
        .ok_or_else(|| DbError::Query(format!("{what}: column `{col}` is unbound")))?
        .try_get_string()
        .map(str::to_string)
        .ok_or_else(|| DbError::Query(format!("{what}: column `{col}` is not a string")))
}

/// Reads the belief-tier surface rooted at `root` from the schema `tx` sees.
///
/// An unknown `root`, a closure that lacks `root`, and a closure that plays roles
/// but yields no relation are schema errors; every other failure is a query error.
pub async fn belief_tier_surface(
    tx: &Transaction,
    root: &str,
) -> Result<BeliefTierSurface, DbError> {
    validate_schema_label(root)?;

    let closure = format!("match $t sub {root}; let $l = label($t); select $l;");
    let answer = tx
        .query(closure)
        .await
        .map_err(|e| closure_error(root, e))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!(
            "belief-tier closure of `{root}`: expected a row stream"
        )));
    }
    let mut rows = answer.into_rows();
    let mut entities = BTreeSet::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| closure_error(root, e))?;
        entities.insert(string_col(&row, "l", "belief-tier closure")?);
    }
    if !entities.contains(root) {
        return Err(DbError::Schema(format!(
            "belief-tier root type `{root}` is not in the schema"
        )));
    }

    let what = "belief-tier role players";
    let answer = tx
        .query(
            "match $t plays $r; $rel relates $r; \
             let $tl = label($t); let $rl = label($r); let $reln = label($rel); \
             select $tl, $rl, $reln;",
        )
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    // Scoped role label -> (owning relation label, player type labels).
    let mut roles: HashMap<String, (String, HashSet<String>)> = HashMap::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        let player = string_col(&row, "tl", what)?;
        let role = string_col(&row, "rl", what)?;
        let relation = string_col(&row, "reln", what)?;
        roles
            .entry(role)
            .or_insert_with(|| (relation, HashSet::new()))
            .1
            .insert(player);
    }

    let closure_plays = roles
        .values()
        .any(|(_, players)| players.iter().any(|p| entities.contains(p)));
    let relations: BTreeSet<String> = roles
        .into_values()
        .filter(|(_, players)| !players.is_empty() && players.iter().all(|p| entities.contains(p)))
        .map(|(relation, _)| relation)
        .collect();
    if closure_plays && relations.is_empty() {
        return Err(DbError::Schema(format!(
            "belief-tier root `{root}`: its subtypes play roles but no relation's role is \
             played only by them"
        )));
    }
    Ok(BeliefTierSurface {
        entities,
        relations,
    })
}

/// Runs a `reduce $n = count` read and returns its single integer.
async fn count(
    tx: &Transaction,
    query: String,
    label: &str,
    measure: BeliefTierMeasure,
) -> Result<u64, DbError> {
    let what = format!("belief-tier fingerprint of `{label}` {measure}");
    let answer = tx
        .query(query)
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    let mut value = None;
    let mut seen = 0usize;
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        seen += 1;
        value = row
            .get("n")
            .map_err(|e| DbError::Query(format!("{what}: column `n`: {e}")))?
            .and_then(|c| c.try_get_integer());
    }
    if seen != 1 {
        return Err(DbError::Query(format!(
            "{what}: expected exactly one count row, got {seen}"
        )));
    }
    let n = value
        .ok_or_else(|| DbError::Query(format!("{what}: column `n` is not a bound integer")))?;
    u64::try_from(n).map_err(|_| DbError::Query(format!("{what}: negative count {n}")))
}

/// Every type label that can own an attribute: a type that owns one itself
/// (inherited ownership included) or has a subtype that does. A `has $a` over any
/// other type fails type inference, so its attribute count is 0 without a query.
async fn attribute_owners(tx: &Transaction) -> Result<HashSet<String>, DbError> {
    let what = "belief-tier fingerprint attribute owners";
    let answer = tx
        .query("match $s sub $t; $s owns $at; let $tl = label($t); select $tl;")
        .await
        .map_err(|e| DbError::Query(format!("{what}: {e}")))?;
    if !answer.is_row_stream() {
        return Err(DbError::Query(format!("{what}: expected a row stream")));
    }
    let mut rows = answer.into_rows();
    let mut owners = HashSet::new();
    while let Some(item) = rows.next().await {
        let row = item.map_err(|e| DbError::Query(format!("{what}: {e}")))?;
        owners.insert(string_col(&row, "tl", what)?);
    }
    Ok(owners)
}

/// Counts every surface label's instances and attributes, and each relation's
/// role players, as `tx` sees them. `isa` includes subtypes, so a supertype
/// counts its subtypes' instances. An empty surface is a schema error, since its
/// fingerprint would inspect nothing.
pub async fn belief_tier_fingerprint(
    tx: &Transaction,
    surface: &BeliefTierSurface,
) -> Result<BeliefTierFingerprint, DbError> {
    if surface.entities.is_empty() && surface.relations.is_empty() {
        return Err(DbError::Schema(
            "belief-tier fingerprint: the surface names no type, so it would inspect nothing"
                .to_string(),
        ));
    }
    let owners = attribute_owners(tx).await?;
    let mut counts = BTreeMap::new();
    for label in surface.entities.union(&surface.relations) {
        validate_schema_label(label)?;
        let instances = count(
            tx,
            format!("match $x isa {label}; reduce $n = count;"),
            label,
            BeliefTierMeasure::Instances,
        )
        .await?;
        counts.insert((label.clone(), BeliefTierMeasure::Instances), instances);

        let attributes = if owners.contains(label) {
            count(
                tx,
                format!("match $x isa {label}, has $a; reduce $n = count;"),
                label,
                BeliefTierMeasure::Attributes,
            )
            .await?
        } else {
            0
        };
        counts.insert((label.clone(), BeliefTierMeasure::Attributes), attributes);

        if surface.relations.contains(label) {
            let links = count(
                tx,
                format!("match $x isa {label}, links ($p); reduce $n = count;"),
                label,
                BeliefTierMeasure::Links,
            )
            .await?;
            counts.insert((label.clone(), BeliefTierMeasure::Links), links);
        }
    }
    Ok(BeliefTierFingerprint { counts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_to_reports_each_differing_key() {
        let key = |l: &str, m| (l.to_string(), m);
        let before = BeliefTierFingerprint {
            counts: [
                (key("belief", BeliefTierMeasure::Instances), 1),
                (key("belief", BeliefTierMeasure::Attributes), 2),
            ]
            .into_iter()
            .collect(),
        };
        let after = BeliefTierFingerprint {
            counts: [
                (key("belief", BeliefTierMeasure::Instances), 2),
                (key("belief", BeliefTierMeasure::Attributes), 2),
                (key("rests-on", BeliefTierMeasure::Links), 2),
            ]
            .into_iter()
            .collect(),
        };
        let changes = before.changes_to(&after);
        assert_eq!(
            changes,
            vec![
                BeliefTierChange {
                    label: "belief".to_string(),
                    measure: BeliefTierMeasure::Instances,
                    before: 1,
                    after: 2,
                },
                BeliefTierChange {
                    label: "rests-on".to_string(),
                    measure: BeliefTierMeasure::Links,
                    before: 0,
                    after: 2,
                },
            ]
        );
        assert_eq!(changes[0].delta(), 1);
        assert!(before.changes_to(&before).is_empty());
    }
}
