/// A normalized query prepared for simple matching and token inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    normalized: String,
    terms: Vec<String>,
}

impl Query {
    /// Builds a query by trimming, folding whitespace, and lowercasing terms.
    pub fn new(input: &str) -> Self {
        let normalized = normalize_query(input);
        let terms = parse_terms(&normalized);

        Self { normalized, terms }
    }

    /// Returns the normalized query text.
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Returns normalized query terms.
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Reports whether the query contains any searchable terms.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Checks whether every query term appears in the candidate string.
    pub fn matches(&self, candidate: &str) -> bool {
        if self.is_empty() {
            return true;
        }

        let normalized_candidate = normalize_query(candidate);
        self.terms
            .iter()
            .all(|term| normalized_candidate.contains(term))
    }
}

/// Trims, lowercases, and folds runs of whitespace to a single ASCII space.
pub fn normalize_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits an input string into normalized query terms.
pub fn split_terms(input: &str) -> Vec<String> {
    parse_terms(&normalize_query(input))
}

/// Convenience helper for one-off candidate checks.
pub fn matches_query(query: &str, candidate: &str) -> bool {
    Query::new(query).matches(candidate)
}

fn parse_terms(normalized: &str) -> Vec<String> {
    if normalized.is_empty() {
        return Vec::new();
    }

    normalized.split(' ').map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_query_trims_and_folds_whitespace() {
        assert_eq!(normalize_query("  Foo\tbar\nBAZ  "), "foo bar baz");
    }

    #[test]
    fn split_terms_uses_normalized_tokens() {
        assert_eq!(split_terms("  zg   Search "), ["zg", "search"]);
    }

    #[test]
    fn empty_query_matches_any_candidate() {
        let query = Query::new("   ");
        assert!(query.matches("anything"));
    }

    #[test]
    fn query_matches_all_terms_case_insensitively() {
        let query = Query::new("zg rust");
        assert!(query.matches("Building ZG tools with Rust nightly"));
        assert!(!query.matches("Building zg tools with Python"));
    }

    #[test]
    fn matches_query_supports_one_off_checks() {
        assert!(matches_query("mini grep", "A tiny mini GREP-like utility"));
    }
}
