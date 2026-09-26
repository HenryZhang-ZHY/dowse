//! Turning what the user typed into a line matcher, a trigram plan and a path filter.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use tgrep_core::query::{self, QueryPlan};

/// Everything the search bar describes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Space-separated path terms. See [`PathFilter`].
    pub path_filter: String,
}

impl SearchQuery {
    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }
}

/// A query ready to run: the regex used on each line, the trigram plan used to
/// narrow candidate files, and the path filter.
pub struct CompiledQuery {
    pub matcher: Regex,
    pub plan: QueryPlan,
    pub path_filter: PathFilter,
}

impl CompiledQuery {
    pub fn new(query: &SearchQuery) -> Result<Self, String> {
        let case_insensitive = !query.case_sensitive;
        let source = if query.regex {
            query.pattern.clone()
        } else {
            regex::escape(&query.pattern)
        };
        let source = if query.whole_word {
            format!(r"\b(?:{source})\b")
        } else {
            source
        };
        // `multi_line` lets `^` and `$` anchor at line boundaries, because the
        // matcher first runs over whole files before narrowing to a line.
        let matcher = RegexBuilder::new(&source)
            .case_insensitive(case_insensitive)
            .multi_line(true)
            .size_limit(64 * 1024 * 1024)
            .build()
            .map_err(|error| error.to_string())?;

        let plan = if query.regex {
            query::build_query_plan(&query.pattern, case_insensitive)?
        } else {
            query::build_literal_plan(&query.pattern, case_insensitive)
        };

        Ok(Self {
            matcher,
            plan,
            path_filter: PathFilter::new(&query.path_filter)?,
        })
    }
}

/// Filters files by their repository-relative path.
///
/// The filter is a list of space-separated terms:
///
/// * a plain term (`src/engine`) keeps paths containing it, ignoring case;
/// * a term with glob characters (`*.rs`, `src/**/test_*`) keeps paths that
///   match it; a glob without `/` is matched against any path suffix;
/// * a term prefixed with `!` or `-` removes paths that it would keep.
///
/// A path passes when it matches at least one include term (or there are none)
/// and no exclude term.
#[derive(Default)]
pub struct PathFilter {
    include: TermSet,
    exclude: TermSet,
}

#[derive(Default)]
struct TermSet {
    substrings: Vec<String>,
    globs: Option<GlobSet>,
}

impl TermSet {
    fn is_empty(&self) -> bool {
        self.substrings.is_empty() && self.globs.is_none()
    }

    fn matches(&self, path: &str, lowered: &str) -> bool {
        self.substrings
            .iter()
            .any(|term| lowered.contains(term.as_str()))
            || self
                .globs
                .as_ref()
                .is_some_and(|globs| globs.is_match(path))
    }
}

impl PathFilter {
    pub fn new(spec: &str) -> Result<Self, String> {
        let mut include = (Vec::new(), GlobSetBuilder::new(), false);
        let mut exclude = (Vec::new(), GlobSetBuilder::new(), false);
        for raw in spec.split_whitespace() {
            let (target, term) = match raw.strip_prefix('!').or_else(|| raw.strip_prefix('-')) {
                Some(rest) => (&mut exclude, rest),
                None => (&mut include, raw),
            };
            if term.is_empty() {
                continue;
            }
            let term = term.replace('\\', "/");
            if term.contains(['*', '?', '[', '{']) {
                let pattern = if term.contains('/') {
                    term.trim_start_matches('/').to_string()
                } else {
                    format!("**/{term}")
                };
                let glob = GlobBuilder::new(&pattern)
                    .case_insensitive(true)
                    .literal_separator(true)
                    .build()
                    .map_err(|error| format!("invalid path glob `{term}`: {error}"))?;
                target.1.add(glob);
                target.2 = true;
            } else {
                target.0.push(term.to_lowercase());
            }
        }
        let finish = |(substrings, builder, has_globs): (Vec<String>, GlobSetBuilder, bool)| {
            let globs = if has_globs {
                Some(builder.build().map_err(|error| error.to_string())?)
            } else {
                None
            };
            Ok::<_, String>(TermSet { substrings, globs })
        };
        Ok(Self {
            include: finish(include)?,
            exclude: finish(exclude)?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    pub fn matches(&self, path: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let lowered = path.to_lowercase();
        (self.include.is_empty() || self.include.matches(path, &lowered))
            && !self.exclude.matches(path, &lowered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.into(),
            ..Default::default()
        }
    }

    #[test]
    fn literal_queries_escape_regex_syntax() {
        let compiled = CompiledQuery::new(&query("a.b(")).unwrap();
        assert!(compiled.matcher.is_match("x a.b( y"));
        assert!(!compiled.matcher.is_match("axb("));
    }

    #[test]
    fn case_sensitivity_is_opt_in() {
        let insensitive = CompiledQuery::new(&query("Foo")).unwrap();
        assert!(insensitive.matcher.is_match("foo"));
        let sensitive = CompiledQuery::new(&SearchQuery {
            case_sensitive: true,
            ..query("Foo")
        })
        .unwrap();
        assert!(!sensitive.matcher.is_match("foo"));
    }

    #[test]
    fn whole_word_requires_boundaries() {
        let compiled = CompiledQuery::new(&SearchQuery {
            whole_word: true,
            ..query("parse")
        })
        .unwrap();
        assert!(compiled.matcher.is_match("fn parse()"));
        assert!(!compiled.matcher.is_match("fn parser()"));
    }

    #[test]
    fn invalid_regex_is_reported() {
        let error = CompiledQuery::new(&SearchQuery {
            regex: true,
            ..query("(unclosed")
        })
        .err()
        .unwrap();
        assert!(!error.is_empty());
    }

    #[test]
    fn path_filter_combines_substrings_globs_and_exclusions() {
        let filter = PathFilter::new("*.rs !tests").unwrap();
        assert!(filter.matches("src/main.rs"));
        assert!(!filter.matches("tests/it.rs"));
        assert!(!filter.matches("Cargo.toml"));

        let filter = PathFilter::new("Engine").unwrap();
        assert!(filter.matches("src/engine/query.rs"));
        assert!(!filter.matches("src/ui/app.rs"));

        let filter = PathFilter::new("src/**/*.rs").unwrap();
        assert!(filter.matches("src/engine/query.rs"));
        assert!(!filter.matches("benches/src/x.rs"));

        assert!(PathFilter::new("").unwrap().matches("anything"));
    }
}
