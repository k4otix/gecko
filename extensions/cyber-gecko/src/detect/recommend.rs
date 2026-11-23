//! The greedy log-source planner behind `recommend_sources`. Each round adds the one
//! analytic's remaining missing log sources that unblind the most (claim, platform)
//! pairs per new source, until every pair can run.

use std::cmp::Ordering;
use std::collections::BTreeSet;

/// One asserted claim's blind declared platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlindPair {
    /// The claim's index in the caller's claim list.
    pub claim: usize,
    pub platform: String,
    /// One entry per live analytic on `platform` that has a requirement: that
    /// analytic's missing log-source keys there. Each set is non-empty.
    pub analytics: Vec<BTreeSet<String>>,
}

/// One recommended addition: log sources on one platform and the pairs they unblind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub platform: String,
    /// The keys new in this step, sorted.
    pub add: Vec<String>,
    /// Indices into the planned `pairs` slice, ascending.
    pub unblinds: Vec<usize>,
    pub cumulative_unblinded: usize,
}

/// The greedy plan over `pairs`. Errs, naming the pair's index and platform, on a
/// pair with no analytics or an analytic with an empty gap set.
pub(crate) fn plan(pairs: &[BlindPair]) -> Result<Vec<Step>, String> {
    for (i, p) in pairs.iter().enumerate() {
        if p.analytics.is_empty() || p.analytics.iter().any(BTreeSet::is_empty) {
            return Err(format!(
                "pair {i} on platform '{}' has no analytic gap to plan for",
                p.platform
            ));
        }
    }

    // Each (platform, key) gets an id in sorted order, so comparing id lists orders
    // candidates exactly as comparing (platform, sorted keys) does, and an id names its
    // platform.
    let sources: BTreeSet<(&str, &str)> = pairs
        .iter()
        .flat_map(|p| {
            p.analytics
                .iter()
                .flatten()
                .map(move |k| (p.platform.as_str(), k.as_str()))
        })
        .collect();
    let names: Vec<(&str, &str)> = sources.into_iter().collect();
    let id = |platform: &str, key: &str| {
        names
            .binary_search(&(platform, key))
            .expect("every key was interned")
    };
    let gaps: Vec<Vec<Vec<usize>>> = pairs
        .iter()
        .map(|p| {
            p.analytics
                .iter()
                .map(|a| a.iter().map(|k| id(&p.platform, k)).collect())
                .collect()
        })
        .collect();

    let mut added = vec![false; names.len()];
    let mut fixed = vec![false; pairs.len()];
    // Per-candidate scratch marks, compared against a round-and-candidate stamp.
    let mut in_candidate = vec![0usize; names.len()];
    let mut counted = vec![0usize; pairs.len()];
    let mut stamp = 0usize;
    let mut steps = Vec::new();
    let mut cumulative = 0;

    while fixed.iter().any(|f| !f) {
        // Each unfixed pair's analytics' remaining ids, indexed by their smallest id:
        // a candidate can only contain a remaining set whose smallest id it holds.
        let mut by_first: Vec<Vec<(usize, Vec<usize>)>> = vec![Vec::new(); names.len()];
        let mut candidates: BTreeSet<Vec<usize>> = BTreeSet::new();
        for (i, analytics) in gaps.iter().enumerate() {
            if fixed[i] {
                continue;
            }
            for a in analytics {
                let remaining: Vec<usize> = a.iter().copied().filter(|&k| !added[k]).collect();
                by_first[remaining[0]].push((i, remaining.clone()));
                candidates.insert(remaining);
            }
        }

        let mut best: Option<(Vec<usize>, Vec<usize>)> = None;
        for candidate in candidates {
            stamp += 1;
            for &k in &candidate {
                in_candidate[k] = stamp;
            }
            let mut fixes = Vec::new();
            for &k in &candidate {
                for (i, remaining) in &by_first[k] {
                    if counted[*i] != stamp && remaining.iter().all(|&r| in_candidate[r] == stamp) {
                        counted[*i] = stamp;
                        fixes.push(*i);
                    }
                }
            }
            let better = match &best {
                None => true,
                Some((bc, bf)) => {
                    // fixes / candidate against bf / bc, by cross-multiplication; then
                    // more pairs fixed; then the smaller (platform, keys).
                    let ratio = (fixes.len() * bc.len()).cmp(&(bf.len() * candidate.len()));
                    ratio
                        .then(fixes.len().cmp(&bf.len()))
                        .then_with(|| bc.cmp(&candidate))
                        == Ordering::Greater
                }
            };
            if better {
                best = Some((candidate, fixes));
            }
        }

        let (add, mut unblinds) =
            best.ok_or_else(|| "no candidate remains for an unfixed pair".to_string())?;
        for &k in &add {
            added[k] = true;
        }
        for &i in &unblinds {
            fixed[i] = true;
        }
        unblinds.sort_unstable();
        cumulative += unblinds.len();
        steps.push(Step {
            platform: names[add[0]].0.to_string(),
            add: add.iter().map(|&k| names[k].1.to_string()).collect(),
            unblinds,
            cumulative_unblinded: cumulative,
        });
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn pair(claim: usize, platform: &str, analytics: &[&[&str]]) -> BlindPair {
        BlindPair {
            claim,
            platform: platform.into(),
            analytics: analytics
                .iter()
                .map(|keys| keys.iter().map(|k| k.to_string()).collect())
                .collect(),
        }
    }

    fn step(platform: &str, add: &[&str], unblinds: &[usize], cumulative: usize) -> Step {
        Step {
            platform: platform.into(),
            add: add.iter().map(|k| k.to_string()).collect(),
            unblinds: unblinds.to_vec(),
            cumulative_unblinded: cumulative,
        }
    }

    #[test]
    fn plan_of_nothing_is_empty() {
        assert_eq!(plan(&[]), Ok(vec![]));
    }

    #[test]
    fn plan_rejects_an_empty_gap_set() {
        let empty_set = plan(&[pair(0, "Windows", &[&[]])]).unwrap_err();
        assert!(
            empty_set.contains("pair 0") && empty_set.contains("Windows"),
            "{empty_set}"
        );
        let no_analytics = plan(&[pair(0, "Linux", &[])]).unwrap_err();
        assert!(
            no_analytics.contains("pair 0") && no_analytics.contains("Linux"),
            "{no_analytics}"
        );
    }

    #[test]
    fn two_sources_useless_alone_are_one_step() {
        assert_eq!(
            plan(&[pair(0, "Windows", &[&["a", "b"]])]),
            Ok(vec![step("Windows", &["a", "b"], &[0], 1)])
        );
    }

    #[test]
    fn two_claims_share_a_step() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["a", "b", "c"]]),
                pair(1, "Windows", &[&["a", "b", "c"]]),
            ]),
            Ok(vec![step("Windows", &["a", "b", "c"], &[0, 1], 2)])
        );
    }

    #[test]
    fn cheaper_ratio_goes_first() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["a"]]),
                pair(1, "Windows", &[&["b", "c", "d"]]),
                pair(2, "Windows", &[&["b", "c", "d"]]),
            ]),
            Ok(vec![
                step("Windows", &["a"], &[0], 1),
                step("Windows", &["b", "c", "d"], &[1, 2], 3),
            ])
        );
    }

    #[test]
    fn shared_source_shrinks_later_steps() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["a"]]),
                pair(1, "Windows", &[&["a", "b", "c"]]),
            ]),
            Ok(vec![
                step("Windows", &["a"], &[0], 1),
                step("Windows", &["b", "c"], &[1], 2),
            ])
        );
    }

    #[test]
    fn ratio_tie_prefers_more_pairs() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["a"]]),
                pair(1, "Windows", &[&["a", "b"]]),
            ]),
            Ok(vec![step("Windows", &["a", "b"], &[0, 1], 2)])
        );
    }

    #[test]
    fn ties_break_by_platform_then_keys() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["x"]]),
                pair(1, "Linux", &[&["y"]]),
                pair(2, "Linux", &[&["a"]]),
            ]),
            Ok(vec![
                step("Linux", &["a"], &[2], 1),
                step("Linux", &["y"], &[1], 2),
                step("Windows", &["x"], &[0], 3),
            ])
        );
    }

    #[test]
    fn same_key_on_two_platforms_is_two_sources() {
        assert_eq!(
            plan(&[
                pair(0, "Windows", &[&["nsm:flow"]]),
                pair(1, "Linux", &[&["nsm:flow"]]),
            ]),
            Ok(vec![
                step("Linux", &["nsm:flow"], &[1], 1),
                step("Windows", &["nsm:flow"], &[0], 2),
            ])
        );
    }

    #[test]
    fn either_analytic_fixes_the_pair() {
        assert_eq!(
            plan(&[pair(0, "Windows", &[&["a", "b", "c"], &["d"]])]),
            Ok(vec![step("Windows", &["d"], &[0], 1)])
        );
    }

    /// The caller passes only analytics that have a requirement, so a pair whose other
    /// analytic has no requirements reaches the planner as its fixable analytic alone.
    #[test]
    fn zero_requirement_analytic_beside_a_fixable_one() {
        assert_eq!(
            plan(&[pair(0, "macOS", &[&["a"]])]),
            Ok(vec![step("macOS", &["a"], &[0], 1)])
        );
    }

    /// About ATT&CK's scale: 500 claims on distinct techniques, each blind on three
    /// platforms with a 3-5 source gap drawn from 400 keys per platform.
    #[test]
    fn plans_a_large_inventory_quickly() {
        let platforms = ["Linux", "Windows", "macOS"];
        let mut pairs = Vec::new();
        for claim in 0..500usize {
            for (p, platform) in platforms.iter().enumerate() {
                let n = 3 + (claim + p) % 3;
                let keys = (0..n)
                    .map(|j| format!("k{}", (claim * 7 + j * 13 + p) % 400))
                    .collect();
                pairs.push(BlindPair {
                    claim,
                    platform: platform.to_string(),
                    analytics: vec![keys],
                });
            }
        }
        let start = std::time::Instant::now();
        let steps = plan(&pairs).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(steps.last().unwrap().cumulative_unblinded, pairs.len());
        assert!(elapsed.as_secs_f64() < 1.0, "planning took {elapsed:?}");
    }

    // ── properties ─────────────────────────────────────────────────────────────

    const PLATFORMS: [&str; 2] = ["Linux", "Windows"];
    const KEYS: [&str; 6] = ["k0", "k1", "k2", "k3", "k4", "k5"];

    fn pairs_strategy() -> impl Strategy<Value = Vec<BlindPair>> {
        let analytic = proptest::sample::subsequence(KEYS.to_vec(), 1..=4)
            .prop_map(|ks| ks.into_iter().map(String::from).collect::<BTreeSet<_>>());
        let one = (
            0..PLATFORMS.len(),
            proptest::collection::vec(analytic, 1..=3),
        );
        proptest::collection::vec(one, 0..=8).prop_map(|raw| {
            raw.into_iter()
                .enumerate()
                .map(|(i, (p, analytics))| BlindPair {
                    claim: i,
                    platform: PLATFORMS[p].into(),
                    analytics,
                })
                .collect()
        })
    }

    fn runs(pair: &BlindPair, added: &BTreeSet<(String, String)>) -> bool {
        pair.analytics.iter().any(|a| {
            a.iter()
                .all(|k| added.contains(&(pair.platform.clone(), k.clone())))
        })
    }

    proptest! {
        #[test]
        fn every_listed_pair_is_fixed_by_the_sources_so_far(pairs in pairs_strategy()) {
            let steps = plan(&pairs).unwrap();
            let mut added = BTreeSet::new();
            for s in &steps {
                for &i in &s.unblinds {
                    prop_assert!(!runs(&pairs[i], &added), "pair {i} ran before its step");
                }
                for k in &s.add {
                    added.insert((s.platform.clone(), k.clone()));
                }
                for &i in &s.unblinds {
                    prop_assert!(runs(&pairs[i], &added), "pair {i} does not run after its step");
                }
            }
        }

        #[test]
        fn no_step_re_adds_a_source(pairs in pairs_strategy()) {
            let mut added = BTreeSet::new();
            for s in plan(&pairs).unwrap() {
                prop_assert!(!s.add.is_empty());
                for k in &s.add {
                    prop_assert!(added.insert((s.platform.clone(), k.clone())), "{k} re-added");
                }
            }
        }

        #[test]
        fn every_pair_listed_exactly_once(pairs in pairs_strategy()) {
            let steps = plan(&pairs).unwrap();
            let mut listed: Vec<usize> = steps.iter().flat_map(|s| s.unblinds.clone()).collect();
            listed.sort_unstable();
            prop_assert_eq!(listed, (0..pairs.len()).collect::<Vec<_>>());
            prop_assert_eq!(steps.last().map_or(0, |s| s.cumulative_unblinded), pairs.len());
        }

        #[test]
        fn steps_stay_on_their_pairs_platform(pairs in pairs_strategy()) {
            for s in plan(&pairs).unwrap() {
                for &i in &s.unblinds {
                    prop_assert_eq!(&pairs[i].platform, &s.platform);
                }
            }
        }

        #[test]
        fn analytic_order_does_not_matter(pairs in pairs_strategy(), seed in any::<u64>()) {
            let mut shuffled = pairs.clone();
            for (i, p) in shuffled.iter_mut().enumerate() {
                let n = p.analytics.len();
                p.analytics.rotate_left((seed as usize).wrapping_add(i) % n);
                if (seed >> (i % 64)) & 1 == 1 {
                    p.analytics.reverse();
                }
            }
            let view = |steps: Vec<Step>| -> Vec<(String, Vec<String>)> {
                steps.into_iter().map(|s| (s.platform, s.add)).collect()
            };
            prop_assert_eq!(view(plan(&pairs).unwrap()), view(plan(&shuffled).unwrap()));
        }

        #[test]
        fn first_step_has_the_best_ratio(pairs in pairs_strategy()) {
            let steps = plan(&pairs).unwrap();
            let Some(first) = steps.first() else { return Ok(()); };
            let none = BTreeSet::new();
            for p in &pairs {
                for a in &p.analytics {
                    let with: BTreeSet<(String, String)> =
                        a.iter().map(|k| (p.platform.clone(), k.clone())).collect();
                    let fixed = pairs
                        .iter()
                        .filter(|q| q.platform == p.platform && !runs(q, &none) && runs(q, &with))
                        .count();
                    // first.unblinds / first.add >= fixed / a.len()
                    prop_assert!(
                        first.unblinds.len() * a.len() >= fixed * first.add.len(),
                        "candidate {a:?} on {} beats step 0", p.platform
                    );
                }
            }
        }
    }
}
