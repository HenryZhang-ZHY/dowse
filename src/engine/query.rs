//! Turning what the user typed into matchers, trigram plans and a path filter.
//!
//! The search box speaks the query language of [`super::syntax`]: its terms
//! combine per file, so `parse config` finds files mentioning both, and
//! qualifiers such as `path:` or `lang:` test the file itself. With the
//! regular expression option on, the whole box is one regex instead.

use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use tgrep_core::query::{self, QueryPlan};

use super::index::Corpus;
use super::language;
use super::repo::RepoInfo;
use super::syntax::{self, Expr, Field, Pattern};

/// Everything the search bar describes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// The whole pattern is one regular expression, matched line by line,
    /// rather than a query of terms.
    pub regex: bool,
    /// Space-separated path terms. See [`PathFilter`].
    pub path_filter: String,
}

impl SearchQuery {
    /// Nothing to search for yet, such as a qualifier still being typed.
    pub fn is_empty(&self) -> bool {
        if self.regex {
            self.pattern.is_empty()
        } else {
            matches!(syntax::parse(&self.pattern), Ok(None))
        }
    }
}

/// A query ready to run.
pub struct CompiledQuery {
    /// Finds the lines to show: those matching any term the query looks for,
    /// that is, one not under `NOT`. Results and the preview highlight with it.
    pub matcher: Regex,
    pub path_filter: PathFilter,
    terms: Vec<ContentTerm>,
    root: Node,
}

/// A pattern looked for in file contents.
struct ContentTerm {
    regex: Regex,
    plan: QueryPlan,
    /// Outside any `NOT`, so its matches are what the results show.
    wanted: bool,
}

/// The query with its terms compiled; content terms are indexes into
/// [`CompiledQuery::terms`].
enum Node {
    Content(usize),
    File(FileTest),
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
}

enum FileTest {
    Repo(TextTest),
    Path(TextTest),
    Language(String),
    Branch(TextTest),
    Tag(TextTest),
}

/// How a qualifier's value is compared, ignoring case.
enum TextTest {
    /// A plain `path:` value, found anywhere in the path.
    Contains(String),
    /// Other plain values, which must match the whole subject.
    Equals(String),
    Glob(GlobMatcher),
    Regex(Regex),
}

impl TextTest {
    fn matches(&self, subject: &str) -> bool {
        match self {
            Self::Contains(value) => subject.to_lowercase().contains(value.as_str()),
            Self::Equals(value) => subject.to_lowercase() == *value,
            Self::Glob(glob) => glob.is_match(subject),
            Self::Regex(regex) => regex.is_match(subject),
        }
    }
}

/// Candidate files of one corpus: all of them, or these sorted paths.
enum Candidates {
    All,
    Some(Vec<String>),
}

impl CompiledQuery {
    pub fn new(query: &SearchQuery) -> Result<Self, String> {
        let mut compiler = Compiler {
            query,
            terms: Vec::new(),
        };
        let root = if query.regex {
            compiler.content(&Pattern::Regex(query.pattern.clone()), true)?
        } else {
            match syntax::parse(&query.pattern)? {
                Some(expr) => compiler.node(&expr, true)?,
                // Nothing to look for matches nothing.
                None => Node::Or(Vec::new()),
            }
        };
        let terms = compiler.terms;

        let wanted: Vec<&str> = terms
            .iter()
            .filter(|term| term.wanted)
            .map(|term| term.regex.as_str())
            .collect();
        let source = match wanted.as_slice() {
            // A class of no characters: a query of only qualifiers or
            // exclusions finds files but marks no lines.
            [] => r"[^\s\S]".to_string(),
            [single] => single.to_string(),
            several => several
                .iter()
                .map(|source| format!("(?:{source})"))
                .collect::<Vec<_>>()
                .join("|"),
        };
        let matcher = line_regex(&source, !query.case_sensitive)?;

        Ok(Self {
            matcher,
            path_filter: PathFilter::new(&query.path_filter)?,
            terms,
            root,
        })
    }

    /// Whether any term looks at file contents, so files must be read.
    pub fn reads_content(&self) -> bool {
        !self.terms.is_empty()
    }

    /// Whether the query looks for lines to show, rather than only for files.
    pub fn wants_lines(&self) -> bool {
        self.wanted_terms() > 0
    }

    /// How many content terms [`Self::matcher`] combines.
    pub(crate) fn wanted_terms(&self) -> usize {
        self.terms.iter().filter(|term| term.wanted).count()
    }

    /// The regex of the content term at `index`, with whether it is wanted.
    pub(crate) fn term(&self, index: usize) -> (&Regex, bool) {
        let term = &self.terms[index];
        (&term.regex, term.wanted)
    }

    /// Whether a repository could hold matching files at all, judged by the
    /// qualifiers about repositories.
    pub fn may_match_repo(&self, repo: &RepoInfo) -> bool {
        self.verdict(repo, None) != Some(false)
    }

    /// What the qualifiers say about a file before it is read: `Some` when
    /// they settle it whatever it contains, `None` when its contents decide.
    /// Without a `path`, only the repository is known.
    pub fn verdict(&self, repo: &RepoInfo, path: Option<&str>) -> Option<bool> {
        evaluate(&self.root, repo, path, &mut |_| None)
    }

    /// Whether a file matches, given whether each content term matches it.
    /// `content` is asked lazily and at most once per term.
    pub fn matches_file(
        &self,
        repo: &RepoInfo,
        path: &str,
        mut content: impl FnMut(usize) -> bool,
    ) -> bool {
        let mut known: Vec<Option<bool>> = vec![None; self.terms.len()];
        let mut lookup = |index: usize| Some(*known[index].get_or_insert_with(|| content(index)));
        evaluate(&self.root, repo, Some(path), &mut lookup) == Some(true)
    }

    /// Paths in `corpus` that can match, narrowed by each content term's
    /// trigram plan: terms that must all match intersect, alternatives unite,
    /// and a negated term narrows nothing. Sorted.
    pub fn candidates(&self, corpus: &Corpus) -> Vec<String> {
        match self.narrow(&self.root, corpus) {
            Candidates::All => corpus.candidates(&QueryPlan::MatchAll),
            Candidates::Some(paths) => paths,
        }
    }

    fn narrow(&self, node: &Node, corpus: &Corpus) -> Candidates {
        match node {
            Node::Content(index) => {
                let plan = &self.terms[*index].plan;
                if plan.is_match_all() {
                    Candidates::All
                } else {
                    Candidates::Some(corpus.candidates(plan))
                }
            }
            Node::File(_) | Node::Not(_) => Candidates::All,
            Node::And(children) => {
                let mut narrowed: Option<Vec<String>> = None;
                for child in children {
                    if let Candidates::Some(paths) = self.narrow(child, corpus) {
                        narrowed = Some(match narrowed {
                            None => paths,
                            Some(so_far) => intersect(&so_far, &paths),
                        });
                        if narrowed.as_ref().is_some_and(Vec::is_empty) {
                            break;
                        }
                    }
                }
                narrowed.map_or(Candidates::All, Candidates::Some)
            }
            Node::Or(children) => {
                let mut united = Vec::new();
                for child in children {
                    match self.narrow(child, corpus) {
                        Candidates::All => return Candidates::All,
                        Candidates::Some(paths) => united.extend(paths),
                    }
                }
                united.sort();
                united.dedup();
                Candidates::Some(united)
            }
        }
    }
}

/// Three-valued: `None` when something the answer needs is unknown.
fn evaluate(
    node: &Node,
    repo: &RepoInfo,
    path: Option<&str>,
    content: &mut dyn FnMut(usize) -> Option<bool>,
) -> Option<bool> {
    match node {
        Node::Content(index) => content(*index),
        Node::File(test) => match test {
            FileTest::Repo(test) => Some(test.matches(&repo.name)),
            FileTest::Branch(test) => Some(repo.branch.as_deref().is_some_and(|b| test.matches(b))),
            FileTest::Tag(test) => Some(repo.effective_tags().iter().any(|tag| test.matches(tag))),
            FileTest::Path(test) => path.map(|path| test.matches(path)),
            FileTest::Language(wanted) => {
                path.map(|path| language::matches(language::detect(path), wanted))
            }
        },
        Node::Not(inner) => evaluate(inner, repo, path, content).map(|value| !value),
        // Qualifiers come first in both, so that they can settle the answer
        // before any contents are read.
        Node::And(children) => {
            let mut result = Some(true);
            for child in ordered(children) {
                match evaluate(child, repo, path, content) {
                    Some(false) => return Some(false),
                    None => result = None,
                    Some(true) => {}
                }
            }
            result
        }
        Node::Or(children) => {
            let mut result = Some(false);
            for child in ordered(children) {
                match evaluate(child, repo, path, content) {
                    Some(true) => return Some(true),
                    None => result = None,
                    Some(false) => {}
                }
            }
            result
        }
    }
}

/// Children without content terms first.
fn ordered(children: &[Node]) -> impl Iterator<Item = &Node> {
    let reads = |node: &&Node| reads_content(node);
    children
        .iter()
        .filter(move |node| !reads(node))
        .chain(children.iter().filter(reads))
}

fn reads_content(node: &Node) -> bool {
    match node {
        Node::Content(_) => true,
        Node::File(_) => false,
        Node::Not(inner) => reads_content(inner),
        Node::And(children) | Node::Or(children) => children.iter().any(reads_content),
    }
}

fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i].clone());
                i += 1;
                j += 1;
            }
        }
    }
    out
}

struct Compiler<'a> {
    query: &'a SearchQuery,
    terms: Vec<ContentTerm>,
}

impl Compiler<'_> {
    /// `wanted` is false under an odd number of `NOT`s.
    fn node(&mut self, expr: &Expr, wanted: bool) -> Result<Node, String> {
        Ok(match expr {
            Expr::Content(pattern) => self.content(pattern, wanted)?,
            Expr::Qualifier(field, pattern) => Node::File(qualifier(*field, pattern)?),
            Expr::And(items) => Node::And(
                items
                    .iter()
                    .map(|item| self.node(item, wanted))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Or(items) => Node::Or(
                items
                    .iter()
                    .map(|item| self.node(item, wanted))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Not(inner) => Node::Not(Box::new(self.node(inner, !wanted)?)),
        })
    }

    fn content(&mut self, pattern: &Pattern, wanted: bool) -> Result<Node, String> {
        let case_insensitive = !self.query.case_sensitive;
        let source = match pattern {
            Pattern::Literal(text) => regex::escape(text),
            Pattern::Regex(source) => source.clone(),
        };
        let source = if self.query.whole_word {
            format!(r"\b(?:{source})\b")
        } else {
            source
        };
        // Built before the plan: the regex crate explains mistakes better.
        let regex = line_regex(&source, case_insensitive)?;
        let plan = match pattern {
            Pattern::Literal(text) => query::build_literal_plan(text, case_insensitive),
            Pattern::Regex(source) => query::build_query_plan(source, case_insensitive)?,
        };
        self.terms.push(ContentTerm {
            regex,
            plan,
            wanted,
        });
        Ok(Node::Content(self.terms.len() - 1))
    }
}

/// `multi_line` lets `^` and `$` anchor at line boundaries, because matchers
/// first run over whole files before narrowing to a line.
fn line_regex(source: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(source)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .size_limit(64 * 1024 * 1024)
        .build()
        .map_err(|error| error.to_string())
}

fn qualifier(field: Field, pattern: &Pattern) -> Result<FileTest, String> {
    let name = match field {
        Field::Repo => "repo",
        Field::Path => "path",
        Field::Language => "language",
        Field::Branch => "branch",
        Field::Tag => "tag",
    };
    let test = match pattern {
        Pattern::Regex(source) => TextTest::Regex(
            RegexBuilder::new(source)
                .case_insensitive(true)
                .build()
                .map_err(|error| format!("invalid `{name}:` regex: {error}"))?,
        ),
        Pattern::Literal(value) if field == Field::Language => {
            return Ok(FileTest::Language(value.clone()));
        }
        Pattern::Literal(value) if value.contains(['*', '?', '[', '{']) => {
            let value = value.replace('\\', "/");
            let glob = if field == Field::Path {
                path_glob(&value)
            } else {
                value.clone()
            };
            TextTest::Glob(
                glob_builder(&glob)
                    .build()
                    .map_err(|error| format!("invalid `{name}:` glob `{value}`: {error}"))?
                    .compile_matcher(),
            )
        }
        Pattern::Literal(value) if field == Field::Path => {
            TextTest::Contains(value.replace('\\', "/").to_lowercase())
        }
        Pattern::Literal(value) => TextTest::Equals(value.to_lowercase()),
    };
    Ok(match field {
        Field::Repo => FileTest::Repo(test),
        Field::Path => FileTest::Path(test),
        Field::Branch => FileTest::Branch(test),
        Field::Tag => FileTest::Tag(test),
        Field::Language => unreachable!("languages are compared by name"),
    })
}

/// A path glob as the path filter reads it: one without `/` matches any
/// path suffix, one with `/` is anchored at the repository root.
fn path_glob(term: &str) -> String {
    if term.contains('/') {
        term.trim_start_matches('/').to_string()
    } else {
        format!("**/{term}")
    }
}

fn glob_builder(pattern: &str) -> GlobBuilder<'_> {
    let mut builder = GlobBuilder::new(pattern);
    builder.case_insensitive(true).literal_separator(true);
    builder
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
                let pattern = path_glob(&term);
                let glob = glob_builder(&pattern)
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

    fn repo() -> RepoInfo {
        RepoInfo {
            id: "/src/api".into(),
            name: "api".into(),
            root: "/src/api".into(),
            branch: Some("main".into()),
            tags: vec!["owner:alice".into(), "mirror".into()],
            pull_every: None,
        }
    }

    /// Whether a file at `path` holding `text` matches, deciding each content
    /// term by whether some line matches it, as searching does.
    fn finds(pattern: &str, path: &str, text: &str) -> bool {
        let compiled = CompiledQuery::new(&query(pattern)).unwrap();
        compiled.matches_file(&repo(), path, |index| {
            let (regex, _) = compiled.term(index);
            text.lines().any(|line| regex.is_match(line))
        })
    }

    #[test]
    fn terms_combine_per_file_not_per_line() {
        assert!(finds("parse config", "a.rs", "fn parse()\nlet config;"));
        assert!(!finds("parse config", "a.rs", "fn parse()"));
        assert!(finds("parse OR config", "a.rs", "let config;"));
        assert!(finds("parse NOT config", "a.rs", "fn parse()"));
        assert!(!finds("parse NOT config", "a.rs", "fn parse()\nconfig"));
        assert!(finds(
            r#""fn parse" /con\w+/"#,
            "a.rs",
            "fn parse\nconfigure"
        ));
        assert!(!finds(r#""fn parse""#, "a.rs", "fn\nparse"));
    }

    #[test]
    fn qualifiers_test_the_file() {
        let verdict = |pattern: &str, path: &str| {
            CompiledQuery::new(&query(pattern))
                .unwrap()
                .verdict(&repo(), Some(path))
        };
        assert_eq!(verdict("path:*.rs", "src/a.rs"), Some(true));
        assert_eq!(verdict("path:*.rs", "a.md"), Some(false));
        assert_eq!(verdict("path:Engine", "src/engine/x.rs"), Some(true));
        assert_eq!(verdict("path:src/*.rs", "lib/src/a.rs"), Some(false));
        assert_eq!(verdict(r"path:/_test\.go$/", "x/a_test.go"), Some(true));
        assert_eq!(verdict("-path:tests", "tests/a.rs"), Some(false));
        assert_eq!(verdict("lang:rust", "src/a.rs"), Some(true));
        assert_eq!(verdict("language:python", "src/a.rs"), Some(false));
        assert_eq!(verdict("repo:API branch:main", "a"), Some(true));
        assert_eq!(verdict("repo:api-*", "a"), Some(false));
        assert_eq!(verdict("tag:owner:alice tag:mirror", "a"), Some(true));
        assert_eq!(verdict("tag:branch:main", "a"), Some(true));
        assert_eq!(verdict("tag:dev", "a"), Some(false));
        // Contents still decide unless the qualifiers settle it.
        assert_eq!(verdict("x lang:rust", "a.rs"), None);
        assert_eq!(verdict("x lang:rust", "a.py"), Some(false));
        assert_eq!(verdict("x OR lang:rust", "a.rs"), Some(true));
    }

    #[test]
    fn repositories_are_ruled_out_before_their_files() {
        let compiled = CompiledQuery::new(&query("x repo:web")).unwrap();
        assert!(!compiled.may_match_repo(&repo()));
        let compiled = CompiledQuery::new(&query("x path:src")).unwrap();
        assert!(compiled.may_match_repo(&repo()));
    }

    #[test]
    fn the_matcher_finds_wanted_terms_only() {
        let compiled = CompiledQuery::new(&query("foo NOT bar OR baz")).unwrap();
        assert!(compiled.matcher.is_match("foo"));
        assert!(compiled.matcher.is_match("baz"));
        assert!(!compiled.matcher.is_match("bar"));
        assert!(compiled.wants_lines() && compiled.reads_content());

        let compiled = CompiledQuery::new(&query("path:*.rs")).unwrap();
        assert!(!compiled.matcher.is_match("anything at all"));
        assert!(!compiled.wants_lines() && !compiled.reads_content());

        let compiled = CompiledQuery::new(&query("NOT foo")).unwrap();
        assert!(!compiled.wants_lines() && compiled.reads_content());
    }

    #[test]
    fn options_apply_to_every_term() {
        let compiled = CompiledQuery::new(&SearchQuery {
            case_sensitive: true,
            whole_word: true,
            ..query("Parse config")
        })
        .unwrap();
        assert!(compiled.matcher.is_match("fn Parse"));
        assert!(!compiled.matcher.is_match("fn parse"));
        assert!(!compiled.matcher.is_match("configure"));
        assert!(compiled.matcher.is_match("config"));
    }

    #[test]
    fn regex_mode_takes_the_whole_box_as_one_regex() {
        let compiled = CompiledQuery::new(&SearchQuery {
            regex: true,
            ..query(r"fn \w+ path:x")
        })
        .unwrap();
        assert!(compiled.matcher.is_match("fn main path:x"));
        assert!(!compiled.matcher.is_match("fn main"));
    }

    #[test]
    fn mistakes_in_terms_and_qualifiers_are_reported() {
        assert!(CompiledQuery::new(&query("/(unclosed/")).is_err());
        assert!(CompiledQuery::new(&query("path:/(x/")).is_err());
        assert!(CompiledQuery::new(&query("path:[")).is_err());
    }

    #[test]
    fn a_query_still_being_typed_is_empty() {
        assert!(query("").is_empty());
        assert!(query("path:").is_empty());
        assert!(query("OR").is_empty());
        assert!(!query("path:src").is_empty());
        assert!(
            !SearchQuery {
                regex: true,
                ..query(" ")
            }
            .is_empty()
        );
    }
}
