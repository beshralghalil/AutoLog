//! Guessing which of a tracker's names an entry belongs to.
//!
//! Each destination field chooses how far it is willing to guess. A Kimai
//! project is a hard fact — the entry either names it or it does not — while
//! an activity is free text somebody typed twice and is worth matching
//! loosely. [`MatchStrategy`] is that dial, and it is settable per field.

use serde::{Deserialize, Serialize};

/// How hard to try before leaving a field for the user to fill in.
///
/// The variants widen in order: each accepts everything the one above it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchStrategy {
    /// Never guess. Every row waits for a choice.
    Manual,
    /// Only a name that reads identically once punctuation and case are set
    /// aside, so `ACME-6055` finds the project `ACME` but never
    /// `WIDGET`.
    Exact,
    /// Identical, or either name appearing whole inside the other.
    Contains,
    /// Everything above, then the closest name scoring over the threshold.
    Fuzzy,
}

impl MatchStrategy {
    pub const ALL: [Self; 4] = [Self::Manual, Self::Exact, Self::Contains, Self::Fuzzy];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Exact => "Exact",
            Self::Contains => "Contains",
            Self::Fuzzy => "Similar",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Manual => "Never guess; every row is picked by hand.",
            Self::Exact => "Only names that read identically.",
            Self::Contains => "Identical, or one name contained whole in the other.",
            Self::Fuzzy => "The closest name, as long as it is close enough.",
        }
    }
}

/// How alike two names must read before [`MatchStrategy::Fuzzy`] pairs them.
pub const DEFAULT_THRESHOLD: f64 = 0.6;

/// Pick the candidate that best describes any hint, strongest hint first.
///
/// `hints` are ordered by how much they are trusted: for Kimai that is the
/// issue's project key, then the raw work item, then the activity text. A
/// weaker hint is only consulted once the stronger ones find nothing.
pub fn best_match<'a, I>(
    candidates: I,
    hints: &[&str],
    strategy: MatchStrategy,
    threshold: f64,
) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    if strategy == MatchStrategy::Manual {
        return None;
    }

    let candidates: Vec<(&str, String)> = candidates
        .into_iter()
        .map(|candidate| {
            let normalized = normalize(candidate);
            (candidate, normalized)
        })
        .filter(|(_, normalized)| !normalized.is_empty())
        .collect();

    if candidates.is_empty() {
        return None;
    }

    for hint in hints {
        let target = normalize(hint);
        if target.is_empty() {
            continue;
        }

        if let Some(found) = exact(&candidates, &target) {
            return Some(found);
        }
        if strategy == MatchStrategy::Exact {
            continue;
        }

        if let Some(found) = contains(&candidates, &target) {
            return Some(found);
        }
        if strategy == MatchStrategy::Contains {
            continue;
        }

        if let Some(found) = closest(&candidates, &target, threshold) {
            return Some(found);
        }
    }

    None
}

fn exact(candidates: &[(&str, String)], target: &str) -> Option<String> {
    candidates
        .iter()
        .find(|(_, normalized)| normalized == target)
        .map(|(original, _)| (*original).to_owned())
}

/// Either name appearing as a run of whole words inside the other, so the
/// activity `BaseImage` is found inside `WIDGET-BaseImage` but `age` is not.
fn contains(candidates: &[(&str, String)], target: &str) -> Option<String> {
    let target_words: Vec<&str> = target.split(' ').collect();

    candidates
        .iter()
        .find(|(_, normalized)| {
            let words: Vec<&str> = normalized.split(' ').collect();
            contains_slice(&target_words, &words) || contains_slice(&words, &target_words)
        })
        .map(|(original, _)| (*original).to_owned())
}

fn contains_slice(haystack: &[&str], needle: &[&str]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn closest(candidates: &[(&str, String)], target: &str, threshold: f64) -> Option<String> {
    candidates
        .iter()
        .filter(|(_, normalized)| comparable(target, normalized))
        .map(|(original, normalized)| (similarity(target, normalized), *original))
        .filter(|(score, _)| *score >= threshold)
        .max_by(|(left, _), (right, _)| left.total_cmp(right))
        .map(|(_, original)| original.to_owned())
}

/// Guard against pairing names that differ only in their numbers.
///
/// `ABC-6055` and `ABC-6061` read as nearly the same string, but the number is
/// the whole meaning: quietly logging one against the other would be wrong in
/// a way the user would not notice. Where the letters are the same and the
/// digits are not, the two are declared incomparable.
fn comparable(target: &str, candidate: &str) -> bool {
    if digits(target) == digits(candidate) {
        return true;
    }
    words_without_digits(target) != words_without_digits(candidate)
}

fn digits(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn words_without_digits(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|ch| !ch.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Lower-case, with every run of punctuation and separators flattened to a
/// single space, so `[WIDGET] Base-Image` and `widget base image` agree.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_space = true;
        }
    }

    out
}

/// How alike two strings read, from 0.0 to 1.0.
///
/// This is Ratcliff/Obershelp — twice the length of all recursively matched
/// blocks over the combined length — which is the measure Python's `difflib`
/// uses. Keeping the same measure keeps the matches the Python tool made.
pub fn similarity(left: &str, right: &str) -> f64 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let total = left.len() + right.len();
    if total == 0 {
        return 1.0;
    }

    #[allow(clippy::cast_precision_loss)]
    let ratio = (2 * matched_len(&left, &right)) as f64 / total as f64;
    ratio
}

fn matched_len(left: &[char], right: &[char]) -> usize {
    let (left_at, right_at, length) = longest_match(left, right);
    if length == 0 {
        return 0;
    }

    length
        + matched_len(&left[..left_at], &right[..right_at])
        + matched_len(&left[left_at + length..], &right[right_at + length..])
}

/// The earliest longest run the two share, as `(index in left, index in right,
/// length)`.
fn longest_match(left: &[char], right: &[char]) -> (usize, usize, usize) {
    let mut best = (0, 0, 0);
    if left.is_empty() || right.is_empty() {
        return best;
    }

    // Length of the run ending at the current `left` index for each index in
    // `right`; one row of the usual suffix table, rolled forward.
    let mut previous = vec![0_usize; right.len()];
    let mut current = vec![0_usize; right.len()];

    for (i, left_char) in left.iter().enumerate() {
        for (j, right_char) in right.iter().enumerate() {
            current[j] = if left_char == right_char {
                if i == 0 || j == 0 {
                    1
                } else {
                    previous[j - 1] + 1
                }
            } else {
                0
            };

            if current[j] > best.2 {
                best = (i + 1 - current[j], j + 1 - current[j], current[j]);
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTS: [&str; 3] = ["ACME", "WIDGET", "Internal"];

    fn pick(strategy: MatchStrategy, hints: &[&str]) -> Option<String> {
        best_match(PROJECTS, hints, strategy, DEFAULT_THRESHOLD)
    }

    #[test]
    fn exact_takes_the_project_key_and_nothing_looser() {
        assert_eq!(
            pick(MatchStrategy::Exact, &["ACME", "ACME-6055"]),
            Some("ACME".to_owned())
        );
        // The work item alone is not the project's name, so exact declines.
        assert_eq!(pick(MatchStrategy::Exact, &["ACME-6055"]), None);
        assert_eq!(pick(MatchStrategy::Exact, &["Meetings"]), None);
    }

    #[test]
    fn contains_finds_the_project_inside_the_work_item() {
        assert_eq!(
            pick(MatchStrategy::Contains, &["ACME-6055"]),
            Some("ACME".to_owned())
        );
    }

    #[test]
    fn manual_never_guesses() {
        assert_eq!(pick(MatchStrategy::Manual, &["ACME"]), None);
    }

    #[test]
    fn stronger_hints_are_consulted_first() {
        assert_eq!(
            pick(MatchStrategy::Fuzzy, &["WIDGET", "ACME-6055"]),
            Some("WIDGET".to_owned())
        );
    }

    #[test]
    fn names_differing_only_in_their_numbers_are_never_paired() {
        let activities = ["ABC-6055", "Code review"];
        assert_eq!(
            best_match(
                activities,
                &["ABC-6061"],
                MatchStrategy::Fuzzy,
                DEFAULT_THRESHOLD
            ),
            None
        );
        assert_eq!(
            best_match(
                activities,
                &["ABC-6055"],
                MatchStrategy::Fuzzy,
                DEFAULT_THRESHOLD
            ),
            Some("ABC-6055".to_owned())
        );
    }

    #[test]
    fn fuzzy_still_reaches_a_name_that_is_merely_close() {
        let activities = ["Base Image", "Release notes"];
        assert_eq!(
            best_match(
                activities,
                &["BaseImages"],
                MatchStrategy::Fuzzy,
                DEFAULT_THRESHOLD
            ),
            Some("Base Image".to_owned())
        );
        assert_eq!(
            best_match(
                activities,
                &["BaseImages"],
                MatchStrategy::Contains,
                DEFAULT_THRESHOLD
            ),
            None
        );
    }

    #[test]
    fn normalizing_flattens_punctuation_and_case() {
        assert_eq!(normalize("[WIDGET] Base-Image"), "widget base image");
        assert_eq!(normalize("  ---  "), "");
    }

    #[test]
    fn similarity_agrees_with_pythons_difflib() {
        // Values taken from difflib.SequenceMatcher(None, a, b).ratio().
        assert!((similarity("abcd", "abcd") - 1.0).abs() < 1e-9);
        assert!((similarity("", "") - 1.0).abs() < 1e-9);
        assert!((similarity("abcd", "") - 0.0).abs() < 1e-9);
        assert!((similarity("new york mets", "new york meats") - 0.962_962_96).abs() < 1e-6);
        assert!((similarity("tide", "diet") - 0.25).abs() < 1e-9);
        assert!((similarity("abcde", "fghij") - 0.0).abs() < 1e-9);
        assert!((similarity("baseimages", "base image") - 0.9).abs() < 1e-9);
        assert!((similarity("acme 6055", "acme") - 0.615_384_61).abs() < 1e-6);
    }
}
