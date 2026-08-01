//! Operator plumbing shared by both family drivers.
//!
//! The two engines have different bot vocabularies (`builtin:folder|caller|
//! shover|random` vs `builtin:greedy|filler|random`) and different hello
//! messages, so each driver keeps its own spec-kind parser. What they share
//! is the mechanics underneath that vocabulary: splitting the optional
//! `NAME@` prefix off a `--bot` spec, disambiguating duplicate names once the
//! whole field is known, and resolving the match seed.

use std::collections::{HashMap, HashSet};

/// Split an optional `NAME@` prefix off a `--bot` spec and validate it,
/// returning the name (if any) and the remaining spec string for the
/// caller's own spec-kind parser to interpret.
pub fn split_named_spec(spec: &str) -> Result<(Option<String>, &str), String> {
    let (name, rest) = match spec.split_once('@') {
        // `@` inside a command string is fine: only treat the prefix as a
        // name when it doesn't look like the start of a spec itself.
        Some((n, r)) if !n.contains(':') && !n.is_empty() => (Some(n), r),
        _ => (None, spec),
    };
    if let Some(n) = name {
        let count = n.chars().count();
        if count > 32 || n.chars().any(char::is_control) {
            return Err(format!(
                "invalid bot name {n:?}: 1..=32 characters, no control characters"
            ));
        }
    }
    Ok((name.map(str::to_string), rest))
}

/// Names in `--bot` order, with the second and later use of a base name
/// suffixed `-2`, `-3`, …
pub fn disambiguate(base_names: &[String]) -> Vec<String> {
    // Every base name is reserved up front so a generated suffix can never
    // collide with a name someone chose explicitly (caller, caller,
    // caller-2 must not yield caller-2 twice).
    let mut taken: HashSet<String> = base_names.iter().cloned().collect();
    let mut seen: HashMap<&str, u32> = HashMap::new();
    base_names
        .iter()
        .map(|base| {
            let count = seen.entry(base.as_str()).or_insert(0);
            *count += 1;
            if *count == 1 {
                return base.clone();
            }
            loop {
                let candidate = format!("{base}-{count}");
                if taken.insert(candidate.clone()) {
                    return candidate;
                }
                *count += 1;
            }
        })
        .collect()
}

/// The match seed, generated when `--seed` didn't pin one, announced on
/// stderr either way so it is visible regardless of `--output` and a long or
/// aborted run stays reproducible.
pub fn resolve_seed(pinned: Option<u64>) -> u64 {
    match pinned {
        Some(seed) => {
            eprintln!("seed: {seed}");
            seed
        }
        None => {
            let seed = entropy_seed();
            eprintln!("seed: {seed} (pass --seed {seed} to reproduce this match)");
            seed
        }
    }
}

/// A fresh seed for runs that didn't pin one: system time and PID stirred
/// through splitmix64. Match seeding, not cryptography.
fn entropy_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = nanos ^ ((std::process::id() as u64) << 32);
    // splitmix64 finalizer, same constants as poker-core's RNG seeding.
    state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::disambiguate;

    #[test]
    fn duplicate_names_get_unique_suffixes_without_colliding() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            disambiguate(&names(&["caller", "random", "caller"])),
            names(&["caller", "random", "caller-2"])
        );
        // A generated suffix must never collide with an explicit name.
        assert_eq!(
            disambiguate(&names(&["caller", "caller", "caller-2"])),
            names(&["caller", "caller-3", "caller-2"])
        );
        // All-identical field stays fully distinct.
        let out = disambiguate(&names(&["x", "x", "x", "x"]));
        let set: std::collections::HashSet<_> = out.iter().collect();
        assert_eq!(set.len(), 4);
    }
}
