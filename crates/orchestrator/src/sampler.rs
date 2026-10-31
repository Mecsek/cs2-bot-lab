//! Random matchup sampling, biased toward under-played (team pair, map) combinations.

use rand::RngExt;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matchup {
    /// Starts on CT.
    pub team_a: String,
    pub team_b: String,
    pub map: String,
}

/// Unordered pair key so (A, B) and (B, A) share a count.
pub fn pair_key(x: &str, y: &str, map: &str) -> (String, String, String) {
    if x <= y {
        (x.to_string(), y.to_string(), map.to_string())
    } else {
        (y.to_string(), x.to_string(), map.to_string())
    }
}

pub type Counts = HashMap<(String, String, String), u32>;

/// Picks a combination with weight `1 / (1 + played)^exponent`, then a random side assignment.
pub fn sample<R: rand::Rng>(
    rng: &mut R,
    teams: &[String],
    maps: &[String],
    counts: &Counts,
    exponent: f64,
) -> Option<Matchup> {
    let mut combos = Vec::new();
    let mut weights = Vec::new();
    for (i, x) in teams.iter().enumerate() {
        for y in &teams[i + 1..] {
            for m in maps {
                let played = counts.get(&pair_key(x, y, m)).copied().unwrap_or(0);
                combos.push((x, y, m));
                weights.push(1.0 / (1.0 + played as f64).powf(exponent));
            }
        }
    }
    let dist = WeightedIndex::new(&weights).ok()?;
    let (x, y, m) = combos[dist.sample(rng)];
    let (a, b) = if rng.random_bool(0.5) { (x, y) } else { (y, x) };
    Some(Matchup { team_a: a.clone(), team_b: b.clone(), map: m.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn names(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn never_pairs_team_with_itself_and_balances_sides() {
        let teams = names(&["A", "B", "C"]);
        let maps = names(&["de_dust2"]);
        let mut rng = StdRng::seed_from_u64(1);
        let mut a_first = 0;
        for _ in 0..2000 {
            let m = sample(&mut rng, &teams, &maps, &Counts::new(), 1.0).unwrap();
            assert_ne!(m.team_a, m.team_b);
            if m.team_a < m.team_b {
                a_first += 1;
            }
        }
        assert!((800..1200).contains(&a_first), "side assignment skewed: {a_first}");
    }

    #[test]
    fn prefers_unplayed_combinations() {
        let teams = names(&["A", "B", "C"]);
        let maps = names(&["de_dust2"]);
        let mut counts = Counts::new();
        counts.insert(pair_key("A", "B", "de_dust2"), 50);
        counts.insert(pair_key("A", "C", "de_dust2"), 50);
        let mut rng = StdRng::seed_from_u64(2);
        let bc = (0..1000)
            .filter(|_| {
                let m = sample(&mut rng, &teams, &maps, &counts, 1.0).unwrap();
                pair_key(&m.team_a, &m.team_b, &m.map) == pair_key("B", "C", "de_dust2")
            })
            .count();
        assert!(bc > 900, "B-C should dominate, got {bc}");
    }

    #[test]
    fn exponent_zero_is_uniform() {
        let teams = names(&["A", "B", "C"]);
        let maps = names(&["m"]);
        let mut counts = Counts::new();
        counts.insert(pair_key("A", "B", "m"), 1000);
        let mut rng = StdRng::seed_from_u64(3);
        let ab = (0..3000)
            .filter(|_| {
                let m = sample(&mut rng, &teams, &maps, &counts, 0.0).unwrap();
                pair_key(&m.team_a, &m.team_b, &m.map) == pair_key("A", "B", "m")
            })
            .count();
        assert!((850..1150).contains(&ab), "expected ~1000, got {ab}");
    }

    #[test]
    fn needs_two_teams() {
        let mut rng = StdRng::seed_from_u64(4);
        assert!(sample(&mut rng, &names(&["A"]), &names(&["m"]), &Counts::new(), 1.0).is_none());
    }
}
