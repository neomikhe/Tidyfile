use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};

use super::condition::ConditionError;

const REGEX_SIZE_LIMIT: usize = 64 * 1024;
const MAX_CACHED: usize = 256;

type Cache<T> = OnceLock<Mutex<HashMap<String, T>>>;

static GLOBS: Cache<GlobMatcher> = OnceLock::new();
static REGEXES: Cache<Regex> = OnceLock::new();

pub fn glob(pattern: &str) -> Result<GlobMatcher, ConditionError> {
    if let Some(ready) = borrow(&GLOBS, pattern) {
        return Ok(ready);
    }
    let built = build_glob(pattern)?;
    remember(&GLOBS, pattern, &built);
    Ok(built)
}

pub fn regex(pattern: &str) -> Result<Regex, ConditionError> {
    if let Some(ready) = borrow(&REGEXES, pattern) {
        return Ok(ready);
    }
    let built = build_regex(pattern)?;
    remember(&REGEXES, pattern, &built);
    Ok(built)
}

fn build_glob(pattern: &str) -> Result<GlobMatcher, ConditionError> {
    Ok(GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
        .map_err(|_| ConditionError::InvalidGlob)?
        .compile_matcher())
}

fn build_regex(pattern: &str) -> Result<Regex, ConditionError> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| ConditionError::InvalidRegex)
}

fn borrow<T: Clone>(cache: &Cache<T>, pattern: &str) -> Option<T> {
    open(cache)?.get(pattern).cloned()
}

fn remember<T: Clone>(cache: &Cache<T>, pattern: &str, compiled: &T) {
    let Some(mut entries) = open(cache) else {
        return;
    };
    if entries.len() >= MAX_CACHED {
        entries.clear();
    }
    entries.insert(pattern.to_owned(), compiled.clone());
}

fn open<T>(cache: &Cache<T>) -> Option<MutexGuard<'_, HashMap<String, T>>> {
    cache.get_or_init(|| Mutex::new(HashMap::new())).lock().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_glob_compiles_and_matches() {
        assert!(
            glob("Screenshot*.png")
                .unwrap()
                .is_match("Screenshot 1.png")
        );
    }

    #[test]
    fn a_regex_compiles_and_matches() {
        assert!(regex(r"^IMG_\d+$").unwrap().is_match("IMG_0421"));
    }

    #[test]
    fn repeated_requests_return_equivalent_matchers() {
        let first = glob("*.pdf").unwrap();
        let second = glob("*.pdf").unwrap();

        assert_eq!(first.is_match("a.pdf"), second.is_match("a.pdf"));
        assert!(
            second.is_match("b.PDF"),
            "case insensitivity survived caching"
        );
    }

    #[test]
    fn an_invalid_pattern_is_reported_and_not_cached() {
        assert!(matches!(
            regex("(unclosed"),
            Err(ConditionError::InvalidRegex)
        ));
        assert!(matches!(
            regex("(unclosed"),
            Err(ConditionError::InvalidRegex)
        ));
    }

    #[test]
    fn the_cache_stays_bounded() {
        for index in 0..(MAX_CACHED + 20) {
            let _ = regex(&format!("^unique-{index}$"));
        }

        let held = open(&REGEXES).map(|entries| entries.len()).unwrap_or(0);
        assert!(held <= MAX_CACHED, "cache grew to {held} entries");
    }
}
