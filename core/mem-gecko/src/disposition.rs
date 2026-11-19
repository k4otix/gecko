//! The host-side disposition ranker (SPEC-W2-W3 §2.3 / INVARIANTS §8.2 item 2).
//!
//! A *divergence* is two or more beliefs competing to describe the same thing.
//! §8.2 mandates a **host-side** function that ranks the competitors by
//! `(derivation-method, confidence, entrenchment, valid-from)` — most-significant
//! first — and returns either a [`Disposition::Resolved`] head or
//! [`Disposition::Contested`] when the top two tie on every key. This module is
//! that ranker, and *only* the ranker: it is pure (no graph, no writes). The write
//! path that acts on a `Contested` outcome lives in [`crate::writer`].
//!
//! # Conservative evidence grading
//! The four ranking keys are all absence-able ([`DispKeys`] holds `Option`s), and a
//! belief can carry zero, one, or several derivations. The ranker grades what is
//! *recorded*, conservatively — absent or conflicting evidence pushes a competitor
//! toward the **weakest** posture, never an invented or over-claimed grade:
//!
//! - **method** — the belief's derivation-method(s) reduced to one *effective*
//!   method (see [`effective_method`]): no derivation → `None` (sorts below every
//!   real method); multiple derivations → the **weakest** of them. A belief is only
//!   as provable as its weakest derivation.
//! - **confidence** — an absent confidence defaults off the effective method:
//!   `provable → 1.0` (invariant 7 — a provable belief may carry 1.0, so an omitted
//!   confidence is read as 1.0), everything else → `0.5` (a chosen midpoint
//!   default — **not** a derivation). Never a global 0.0.
//! - **entrenchment** — absent → strength 0 (matching the schema's
//!   `entrenchment_rank` floor).
//! - **valid-from** — absent → sorts oldest.
//!
//! Tie detection uses the *same* comparator as the sort: a divergence is
//! `Contested` exactly when the top two competitors compare `Equal` over the
//! defaulted projection, so the sort and the tie-check can never disagree.

use std::cmp::Ordering;

use gecko_extension_api::{DateTime, DerivationMethod, Entrenchment, MemId};

/// The four ranking keys a host-side disposition reads for one competing belief.
///
/// Every key is optional because every one is absence-able in the graph:
/// `confidence`/`entrenchment`/`valid-from` are `@card(0..1)`, and `method` rides
/// the `derivation` relation, which an evidence-less belief simply lacks. Absence
/// stays visible here (the raw `Option`s); the comparator does the defaulting.
///
/// `method` is the **effective** method — the belief's derivation-method(s) already
/// reduced to one via [`effective_method`] (weakest of several, `None` for none).
#[derive(Clone, Debug, PartialEq)]
pub struct DispKeys {
    /// Effective derivation-method (weakest of several; `None` if no derivation).
    pub method: Option<DerivationMethod>,
    /// Recorded confidence in `[0.0, 1.0]`, or `None` if unrecorded.
    pub confidence: Option<f64>,
    /// Recorded entrenchment tier, or `None` if unrecorded.
    pub entrenchment: Option<Entrenchment>,
    /// Recorded `valid-from`, or `None` if unrecorded.
    pub valid_from: Option<DateTime>,
}

/// The outcome of ranking a divergence's competitors.
#[derive(Clone, Debug, PartialEq)]
pub enum Disposition {
    /// A unique winner. `head` is the winning belief, `confidence` is the
    /// (method-defaulted) confidence the ranker used for it, and `reason` names the
    /// first key that discriminated the head from the runner-up.
    Resolved {
        /// The winning belief.
        head: MemId,
        /// The winner's confidence (recorded, or the method default).
        confidence: f64,
        /// A pure, deterministic description of why the head won (the first
        /// discriminating key + values). Deterministic in the ranked keys alone —
        /// no timestamps, run ids, or iteration-order-dependent content.
        reason: String,
    },
    /// No unique winner — the top two competitors tie on all four keys.
    Contested,
}

/// Reduces a belief's derivation-method(s) to a single **effective** method: the
/// **weakest** of them (lowest [`DerivationMethod::rank`]), or `None` for an empty
/// list. A belief is only as provable as its weakest derivation, so a competitor
/// carrying both a provable and a probabilistic derivation grades as probabilistic.
pub fn effective_method(methods: &[DerivationMethod]) -> Option<DerivationMethod> {
    methods.iter().copied().min_by_key(|m| m.rank())
}

/// Ranks a divergence's competitors by the 4-key order, most-significant first, and
/// returns the [`Disposition`].
///
/// Generalizes the pairwise §8.2 disposition to N competitors: a unique maximum
/// under the comparator is [`Disposition::Resolved`]; a top-two tie on every
/// defaulted key is [`Disposition::Contested`]. Callers guard `competitors.len() >=
/// 2` (a divergence needs at least two parties); an empty slice degenerates to
/// `Contested` and a single competitor trivially resolves.
pub fn rank(competitors: &[(MemId, DispKeys)]) -> Disposition {
    // Order indices strongest-first under the shared comparator.
    let mut idx: Vec<usize> = (0..competitors.len()).collect();
    idx.sort_by(|&i, &j| cmp_keys(&competitors[i].1, &competitors[j].1).reverse());

    match idx.as_slice() {
        [] => Disposition::Contested,
        [only] => {
            let (head, keys) = &competitors[*only];
            Disposition::Resolved {
                head: head.clone(),
                confidence: confidence_key(keys),
                reason: "sole competitor".to_string(),
            }
        }
        [top, second, ..] => {
            let (head, tkeys) = &competitors[*top];
            let skeys = &competitors[*second].1;
            if cmp_keys(tkeys, skeys) == Ordering::Equal {
                Disposition::Contested
            } else {
                Disposition::Resolved {
                    head: head.clone(),
                    confidence: confidence_key(tkeys),
                    reason: discriminating_reason(tkeys, skeys),
                }
            }
        }
    }
}

/// The shared 4-key comparator: `method-rank → confidence(method-defaulted) →
/// entrenchment-strength → valid-from`, most-significant first. `Greater` means the
/// left competitor is the stronger.
fn cmp_keys(a: &DispKeys, b: &DispKeys) -> Ordering {
    method_key(a)
        .cmp(&method_key(b))
        .then_with(|| confidence_key(a).total_cmp(&confidence_key(b)))
        .then_with(|| entrenchment_key(a).cmp(&entrenchment_key(b)))
        .then_with(|| a.valid_from.cmp(&b.valid_from))
}

/// Key #1: effective-method rank. `None` (no derivation) sorts below every real
/// method (`Option<u8>`: `None < Some(0)`), so evidence-less/revised beliefs land
/// last on the method key.
fn method_key(k: &DispKeys) -> Option<u8> {
    k.method.map(|m| m.rank())
}

/// Key #2: confidence, method-defaulted when absent — `provable → 1.0` (invariant
/// 7), else → `0.5` (the chosen midpoint); a NaN confidence reads as absent. Only
/// bites on a method-class tie.
fn confidence_key(k: &DispKeys) -> f64 {
    match k.confidence.filter(|c| !c.is_nan()) {
        Some(c) => c,
        None if k.method.is_some_and(|m| m.provable()) => 1.0,
        None => 0.5,
    }
}

/// Key #3: entrenchment strength, `0` when absent (the `entrenchment_rank` floor).
fn entrenchment_key(k: &DispKeys) -> u8 {
    k.entrenchment.map_or(0, |e| e.strength())
}

/// Names the first key on which the winner out-ranks the runner-up, with its
/// values. Pure in the ranked keys — no timestamps beyond the ranked `valid-from`,
/// no ids, no iteration order — so a downstream assertion on it cannot flake.
fn discriminating_reason(winner: &DispKeys, runner: &DispKeys) -> String {
    if method_key(winner) != method_key(runner) {
        return format!(
            "method {} > {}",
            fmt_method(winner.method),
            fmt_method(runner.method)
        );
    }
    let (wc, rc) = (confidence_key(winner), confidence_key(runner));
    if wc.partial_cmp(&rc) != Some(Ordering::Equal) {
        return format!("confidence {wc} > {rc}");
    }
    if entrenchment_key(winner) != entrenchment_key(runner) {
        return format!(
            "entrenchment {} > {}",
            entrenchment_key(winner),
            entrenchment_key(runner)
        );
    }
    format!(
        "valid-from {} > {}",
        fmt_valid_from(winner.valid_from),
        fmt_valid_from(runner.valid_from)
    )
}

/// Deterministic rendering of an effective method for `reason`.
fn fmt_method(m: Option<DerivationMethod>) -> &'static str {
    m.map_or("none", |m| m.as_str())
}

/// Deterministic rendering of a `valid-from` for `reason` (RFC 3339, or `none`).
fn fmt_valid_from(vf: Option<DateTime>) -> String {
    vf.map_or_else(|| "none".to_string(), |t| t.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime {
        s.parse().expect("rfc3339 datetime")
    }

    /// Builds `DispKeys` from an already-effective method.
    fn keys(
        method: Option<DerivationMethod>,
        confidence: Option<f64>,
        entrenchment: Option<Entrenchment>,
        valid_from: Option<DateTime>,
    ) -> DispKeys {
        DispKeys {
            method,
            confidence,
            entrenchment,
            valid_from,
        }
    }

    fn head_of(d: &Disposition) -> &str {
        match d {
            Disposition::Resolved { head, .. } => head.as_str(),
            Disposition::Contested => panic!("expected Resolved, got Contested"),
        }
    }

    // ── effective_method reduction ──────────────────────────────────────────────

    #[test]
    fn effective_method_reduces_to_weakest_or_none() {
        assert_eq!(effective_method(&[]), None);
        assert_eq!(
            effective_method(&[DerivationMethod::TypeJoin]),
            Some(DerivationMethod::TypeJoin)
        );
        // A provable AND a probabilistic derivation → the weakest (probabilistic).
        assert_eq!(
            effective_method(&[DerivationMethod::TypeJoin, DerivationMethod::LlmSynthesis]),
            Some(DerivationMethod::LlmSynthesis)
        );
        // Order-independent.
        assert_eq!(
            effective_method(&[DerivationMethod::LlmSynthesis, DerivationMethod::TypeJoin]),
            Some(DerivationMethod::LlmSynthesis)
        );
        // Provable + attested → the attested (weaker) one.
        assert_eq!(
            effective_method(&[
                DerivationMethod::Cardinality,
                DerivationMethod::HumanAssertion
            ]),
            Some(DerivationMethod::HumanAssertion)
        );
    }

    #[test]
    fn nan_confidence_is_method_defaulted_and_order_independent() {
        let m = Some(DerivationMethod::HumanAssertion);
        let nan = (
            "mem/bel/nan".to_string(),
            keys(m, Some(f64::NAN), None, None),
        );
        let low = ("mem/bel/low".to_string(), keys(m, Some(0.4), None, None));
        let high = ("mem/bel/high".to_string(), keys(m, Some(0.9), None, None));
        for order in [
            [nan.clone(), low.clone(), high.clone()],
            [high.clone(), nan.clone(), low.clone()],
            [low.clone(), high.clone(), nan.clone()],
        ] {
            let c: Vec<_> = order.into_iter().map(|(id, k)| (MemId(id), k)).collect();
            assert_eq!(head_of(&rank(&c)), "mem/bel/high");
        }
        // NaN reads as the attested default 0.5, which out-ranks 0.4.
        let c = vec![(MemId(nan.0), nan.1), (MemId(low.0), low.1)];
        assert_eq!(head_of(&rank(&c)), "mem/bel/nan");
    }

    // ── SEP / TIE (the §8.2 prototype cases) ────────────────────────────────────

    #[test]
    fn sep_separated_evidence_resolves_to_the_stronger() {
        // A: human-assertion (rank 1) out-ranks B: llm-synthesis (rank 0) on key #1.
        let competitors = vec![
            (
                MemId::new("A"),
                keys(
                    Some(DerivationMethod::HumanAssertion),
                    Some(0.9),
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-06-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("B"),
                keys(
                    Some(DerivationMethod::LlmSynthesis),
                    Some(0.2),
                    Some(Entrenchment::Llm),
                    Some(dt("2026-01-01T00:00:00Z")),
                ),
            ),
        ];
        let d = rank(&competitors);
        assert_eq!(head_of(&d), "A");
        match d {
            Disposition::Resolved { confidence, .. } => assert_eq!(confidence, 0.9),
            Disposition::Contested => unreachable!(),
        }
    }

    #[test]
    fn tie_all_four_keys_equal_is_contested() {
        let k = keys(
            Some(DerivationMethod::LlmSynthesis),
            Some(0.5),
            Some(Entrenchment::Llm),
            Some(dt("2026-03-01T00:00:00Z")),
        );
        let competitors = vec![(MemId::new("TA"), k.clone()), (MemId::new("TB"), k)];
        assert_eq!(rank(&competitors), Disposition::Contested);
    }

    // ── The discriminating fixture (the load-bearing 3-class proof) ─────────────

    #[test]
    fn discriminating_provable_beats_human_assertion() {
        // Provable (type-join, rank 2) vs human-assertion (attested, rank 1), tied
        // on the other three keys. Under the shipped 3-class order the PROVABLE
        // belief wins. This is the one input where the 3-class order and an
        // authorship-weighted 7-way order (which floats human-assertion above
        // provable) disagree: swap that order in and this assertion FLIPS to "H" and
        // fails. That is what makes this fixture a proof of the order, not a coincidence.
        let shared_ent = Some(Entrenchment::UserStated);
        let shared_vf = Some(dt("2026-05-01T00:00:00Z"));
        let competitors = vec![
            (
                MemId::new("P"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    Some(0.7),
                    shared_ent,
                    shared_vf,
                ),
            ),
            (
                MemId::new("H"),
                keys(
                    Some(DerivationMethod::HumanAssertion),
                    Some(0.7),
                    shared_ent,
                    shared_vf,
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "P");
    }

    // ── missing-key — survival + count preserved ────────────────────────────────

    #[test]
    fn missing_key_evidence_less_competitor_ranks_below_provable_no_drop() {
        // An evidence-less competitor (no method, no other keys) does not win, and
        // both competitors are still considered (the ranker never silently drops the
        // absent-key one — its `head` is the provable competitor, computed over both).
        let competitors = vec![
            (
                MemId::new("E"), // evidence-less: every key absent
                keys(None, None, None, None),
            ),
            (
                MemId::new("P"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    Some(0.6),
                    Some(Entrenchment::ToolDerived),
                    Some(dt("2026-02-01T00:00:00Z")),
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "P");
    }

    // ── missing-key — the default's OUTCOME, on a genuine method-class tie ──────

    #[test]
    fn absent_confidence_defaults_to_one_for_provable_and_wins() {
        // Two provable (type-join) competitors — a genuine tie on key #1, so
        // confidence (key #2) is actually consulted. The confidence-absent one
        // defaults to 1.0 (invariant-7 reading: omitted ≡ recorded 1.0 for provable)
        // and BEATS a sibling that recorded 0.9.
        let competitors = vec![
            (
                MemId::new("absent"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    None, // → default 1.0
                    Some(Entrenchment::ToolDerived),
                    Some(dt("2026-04-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("recorded_09"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    Some(0.9),
                    Some(Entrenchment::ToolDerived),
                    Some(dt("2026-04-01T00:00:00Z")),
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "absent");
    }

    #[test]
    fn absent_confidence_defaults_to_midpoint_for_non_provable_and_wins() {
        // Two human-assertion (attested) competitors — again a genuine method-class
        // tie, so confidence bites. The confidence-absent one defaults to the chosen
        // 0.5 midpoint and BEATS a sibling that recorded 0.3.
        let competitors = vec![
            (
                MemId::new("absent"),
                keys(
                    Some(DerivationMethod::HumanAssertion),
                    None, // → default 0.5
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-04-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("recorded_03"),
                keys(
                    Some(DerivationMethod::HumanAssertion),
                    Some(0.3),
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-04-01T00:00:00Z")),
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "absent");
    }

    // ── supersede-born — revisions rank last on method ──────────────────────────

    #[test]
    fn supersede_born_method_none_ranks_below_llm() {
        // A supersede-born (revised) belief carries no derivation → method `None`,
        // which sorts BELOW even llm-synthesis (rank 0). So a raw llm competitor
        // out-ranks the revision on the method key. This is the visible symptom of
        // the upstream defect where supersede drops derivation provenance; the ranker
        // grades only recorded evidence and does not paper over it.
        let competitors = vec![
            (
                MemId::new("revision"), // method None (supersede-born)
                keys(
                    None,
                    Some(0.99), // even a high confidence cannot rescue the method key
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-06-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("llm"),
                keys(
                    Some(DerivationMethod::LlmSynthesis),
                    Some(0.2),
                    Some(Entrenchment::Llm),
                    Some(dt("2026-01-01T00:00:00Z")),
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "llm");
    }

    // ── multi-derivation — grades at the weakest, not the strongest ─────────────

    #[test]
    fn multi_derivation_grades_at_weakest_and_loses_to_plain_provable() {
        // A competitor carrying BOTH a provable and an llm derivation reduces to the
        // weakest (llm) and loses the method key to a plain provable competitor.
        let multi = effective_method(&[DerivationMethod::TypeJoin, DerivationMethod::LlmSynthesis]);
        assert_eq!(multi, Some(DerivationMethod::LlmSynthesis));
        let competitors = vec![
            (
                MemId::new("multi"),
                keys(
                    multi,
                    Some(0.95), // high confidence cannot rescue the weakest method
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-06-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("plain_provable"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    Some(0.4),
                    Some(Entrenchment::Inferred),
                    Some(dt("2026-01-01T00:00:00Z")),
                ),
            ),
        ];
        assert_eq!(head_of(&rank(&competitors)), "plain_provable");
    }

    // ── reason determinism ──────────────────────────────────────────────────────

    #[test]
    fn reason_is_deterministic_and_names_the_first_discriminating_key() {
        let competitors = vec![
            (
                MemId::new("P"),
                keys(
                    Some(DerivationMethod::TypeJoin),
                    Some(0.7),
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-05-01T00:00:00Z")),
                ),
            ),
            (
                MemId::new("H"),
                keys(
                    Some(DerivationMethod::HumanAssertion),
                    Some(0.7),
                    Some(Entrenchment::UserStated),
                    Some(dt("2026-05-01T00:00:00Z")),
                ),
            ),
        ];
        // Method is the first discriminating key; the reason is stable across runs.
        match rank(&competitors) {
            Disposition::Resolved { reason, .. } => {
                assert_eq!(reason, "method type-join > human-assertion");
            }
            Disposition::Contested => unreachable!(),
        }
    }
}
