//! The search box's query language, after GitHub code search.
//!
//! * Terms separated by spaces must all appear in a file, not necessarily on
//!   the same line: `parse config` finds files mentioning both.
//! * `"exact phrase"` keeps spaces and operators literal; `\"` and `\\`
//!   escape inside it.
//! * `/regex/` is a regular expression, which may contain spaces.
//! * `OR` and `AND` (upper case) combine terms, with `AND` binding tighter;
//!   `NOT` negates the term or group after it; parentheses group.
//! * Qualifiers narrow by what a file is rather than what it says:
//!   `path:`, `language:` (or `lang:`), `repo:`, `branch:` and `tag:`, and
//!   `content:` forces a plain term. A qualifier's value can be quoted or a
//!   `/regex/`, and a leading `-` negates it: `-path:tests`.
//!
//! The parser is lenient, since it runs on every keystroke: operators with
//! nothing to combine and unclosed groups are ignored rather than reported.

/// Text to look for, in file contents or in a qualifier's subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Literal(String),
    Regex(String),
}

/// What a qualifier looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Repo,
    Path,
    Language,
    Branch,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// The file's contents contain the pattern on some line.
    Content(Pattern),
    Qualifier(Field, Pattern),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

/// Parse a query, or `None` when it holds nothing to search for.
pub fn parse(input: &str) -> Result<Option<Expr>, String> {
    let tokens = Lexer::new(input).tokens()?;
    let mut parser = Parser { tokens, next: 0 };
    let mut parts = Vec::new();
    // A group's close only comes from the lexer inside a group, so the parser
    // ends each part at the end of input; keep going just in case.
    while parser.next < parser.tokens.len() {
        parts.extend(parser.or_expr());
        parser.eat(&Token::Close);
    }
    Ok(combine(parts, Expr::And))
}

/// GitHub qualifiers that have no meaning over local clones.
const UNSUPPORTED: &[&str] = &["symbol", "is", "org", "user", "enterprise"];

fn field(name: &str) -> Option<Option<Field>> {
    Some(match name {
        "repo" => Some(Field::Repo),
        "path" => Some(Field::Path),
        "language" | "lang" => Some(Field::Language),
        "branch" => Some(Field::Branch),
        "tag" => Some(Field::Tag),
        "content" => None,
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Not,
    Atom(Expr),
}

/// How a value was written, which decides whether `OR` is an operator.
enum Value {
    Bare(String),
    Quoted(String),
    Regex(String),
}

impl Value {
    fn into_pattern(self) -> Pattern {
        match self {
            Self::Bare(text) | Self::Quoted(text) => Pattern::Literal(text),
            Self::Regex(source) => Pattern::Regex(source),
        }
    }
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    /// Groups open at `pos`, so that `)` only closes one when there is one.
    depth: usize,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            pos: 0,
            depth: 0,
        }
    }

    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn tokens(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let skipped = self.rest().len() - self.rest().trim_start().len();
            self.pos += skipped;
            let Some(ch) = self.peek() else {
                return Ok(tokens);
            };
            if ch == '(' {
                self.pos += 1;
                self.depth += 1;
                tokens.push(Token::Open);
                continue;
            }
            if ch == ')' && self.depth > 0 {
                self.pos += 1;
                self.depth -= 1;
                tokens.push(Token::Close);
                continue;
            }
            if let Some((negated, name, length)) = self.qualifier_prefix() {
                if UNSUPPORTED.contains(&name.as_str()) {
                    return Err(format!(
                        "`{name}:` is not supported here; search the text itself instead"
                    ));
                }
                if let Some(field) = field(&name) {
                    self.pos += length;
                    // A qualifier still being typed, like `path:`, selects nothing yet.
                    if let Some(value) = self.value() {
                        let pattern = value.into_pattern();
                        let atom = match field {
                            Some(field) => Expr::Qualifier(field, pattern),
                            None => Expr::Content(pattern),
                        };
                        tokens.push(Token::Atom(if negated {
                            Expr::Not(Box::new(atom))
                        } else {
                            atom
                        }));
                    }
                    continue;
                }
            }
            match self.value() {
                Some(Value::Bare(word)) => tokens.push(match word.as_str() {
                    "AND" => Token::And,
                    "OR" => Token::Or,
                    "NOT" => Token::Not,
                    _ => Token::Atom(Expr::Content(Pattern::Literal(word))),
                }),
                Some(value) => tokens.push(Token::Atom(Expr::Content(value.into_pattern()))),
                None => {}
            }
        }
    }

    /// `name:` or `-name:` at the cursor: whether negated, the lower-cased
    /// name, and the prefix's length.
    fn qualifier_prefix(&self) -> Option<(bool, String, usize)> {
        let rest = self.rest();
        let (negated, body) = match rest.strip_prefix('-') {
            Some(body) => (true, body),
            None => (false, rest),
        };
        let name_len = body
            .bytes()
            .take_while(|byte| byte.is_ascii_alphabetic())
            .count();
        if name_len == 0 || body.as_bytes().get(name_len) != Some(&b':') {
            return None;
        }
        let name = body[..name_len].to_ascii_lowercase();
        Some((negated, name, usize::from(negated) + name_len + 1))
    }

    /// A quoted string, a `/regex/` or a bare word at the cursor, or `None`
    /// when there is nothing there.
    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            ch if ch.is_whitespace() => None,
            '"' => {
                let text = self.quoted();
                (!text.is_empty()).then_some(Value::Quoted(text))
            }
            '/' => match self.regex() {
                Some(source) => Some(Value::Regex(source)),
                None => self.bare().map(Value::Bare),
            },
            _ => self.bare().map(Value::Bare),
        }
    }

    fn quoted(&mut self) -> String {
        let mut text = String::new();
        let mut chars = self.rest()[1..].char_indices();
        let mut end = self.text.len();
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '\\' => match chars.clone().next() {
                    Some((_, next @ ('"' | '\\'))) => {
                        text.push(next);
                        chars.next();
                    }
                    _ => text.push(ch),
                },
                '"' => {
                    end = self.pos + 1 + offset + 1;
                    break;
                }
                _ => text.push(ch),
            }
        }
        self.pos = end;
        text
    }

    /// A `/regex/` whose closing slash ends the token. Without one, the slash
    /// starts an ordinary word, like `/usr/bin`.
    fn regex(&mut self) -> Option<String> {
        let body = &self.rest()[1..];
        let mut source = String::new();
        let mut chars = body.char_indices().peekable();
        while let Some((offset, ch)) = chars.next() {
            match ch {
                '\\' => match chars.next() {
                    Some((_, '/')) => source.push('/'),
                    Some((_, next)) => {
                        source.push('\\');
                        source.push(next);
                    }
                    None => source.push('\\'),
                },
                '/' => {
                    let after = body[offset + 1..].chars().next();
                    let ends_token = match after {
                        None => true,
                        Some(next) => next.is_whitespace() || (next == ')' && self.depth > 0),
                    };
                    if ends_token {
                        if source.is_empty() {
                            return None;
                        }
                        self.pos += 1 + offset + 1;
                        return Some(source);
                    }
                    source.push('/');
                }
                _ => source.push(ch),
            }
        }
        None
    }

    /// Up to the next space, or to a `)` closing an open group. Parentheses
    /// balanced within the word, as in `foo()`, stay part of it, and a
    /// backslash keeps the next character from ending it.
    fn bare(&mut self) -> Option<String> {
        let mut word = String::new();
        let mut balance = 0usize;
        let mut chars = self.rest().char_indices();
        let mut end = self.text.len();
        while let Some((offset, ch)) = chars.next() {
            match ch {
                ch if ch.is_whitespace() => {
                    end = self.pos + offset;
                    break;
                }
                '\\' => {
                    word.push(ch);
                    if let Some((_, next)) = chars.next() {
                        word.push(next);
                    }
                }
                '(' => {
                    balance += 1;
                    word.push(ch);
                }
                ')' if balance > 0 => {
                    balance -= 1;
                    word.push(ch);
                }
                ')' if self.depth > 0 => {
                    end = self.pos + offset;
                    break;
                }
                _ => word.push(ch),
            }
        }
        self.pos = end;
        (!word.is_empty()).then_some(word)
    }
}

struct Parser {
    tokens: Vec<Token>,
    next: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.next)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.next += 1;
            true
        } else {
            false
        }
    }

    fn or_expr(&mut self) -> Option<Expr> {
        let mut branches = Vec::new();
        loop {
            branches.extend(self.and_expr());
            if !self.eat(&Token::Or) {
                break;
            }
        }
        combine(branches, Expr::Or)
    }

    fn and_expr(&mut self) -> Option<Expr> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None | Some(Token::Or | Token::Close) => break,
                Some(Token::And) => self.next += 1,
                Some(_) => items.extend(self.unary()),
            }
        }
        combine(items, Expr::And)
    }

    /// A term, group or negation; called only where one can start.
    fn unary(&mut self) -> Option<Expr> {
        let token = std::mem::replace(self.tokens.get_mut(self.next)?, Token::Close);
        self.next += 1;
        match token {
            Token::Not => match self.peek() {
                None | Some(Token::Or | Token::And | Token::Close) => None,
                Some(_) => self.unary().map(|expr| Expr::Not(Box::new(expr))),
            },
            Token::Open => {
                let inner = self.or_expr();
                self.eat(&Token::Close);
                inner
            }
            Token::Atom(expr) => Some(expr),
            Token::And | Token::Or | Token::Close => None,
        }
    }
}

/// One expression for several, or none for none.
fn combine(mut items: Vec<Expr>, join: fn(Vec<Expr>) -> Expr) -> Option<Expr> {
    match items.len() {
        0 => None,
        1 => items.pop(),
        _ => Some(join(items)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(text: &str) -> Expr {
        Expr::Content(Pattern::Literal(text.into()))
    }

    fn re(source: &str) -> Expr {
        Expr::Content(Pattern::Regex(source.into()))
    }

    fn qualifier(field: Field, value: &str) -> Expr {
        Expr::Qualifier(field, Pattern::Literal(value.into()))
    }

    fn not(expr: Expr) -> Expr {
        Expr::Not(Box::new(expr))
    }

    fn parsed(input: &str) -> Expr {
        parse(input).unwrap().unwrap()
    }

    #[test]
    fn empty_queries_have_nothing_to_search() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("   ").unwrap(), None);
        assert_eq!(parse("path:").unwrap(), None);
        assert_eq!(parse("OR NOT ()").unwrap(), None);
    }

    #[test]
    fn spaces_mean_and() {
        assert_eq!(parsed("parse"), lit("parse"));
        assert_eq!(
            parsed("parse config"),
            Expr::And(vec![lit("parse"), lit("config")])
        );
        assert_eq!(parsed("a AND b"), Expr::And(vec![lit("a"), lit("b")]));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        assert_eq!(
            parsed("a b OR c"),
            Expr::Or(vec![Expr::And(vec![lit("a"), lit("b")]), lit("c")])
        );
        assert_eq!(
            parsed("a (b OR c)"),
            Expr::And(vec![lit("a"), Expr::Or(vec![lit("b"), lit("c")])])
        );
    }

    #[test]
    fn not_negates_the_next_term_or_group() {
        assert_eq!(parsed("a NOT b"), Expr::And(vec![lit("a"), not(lit("b"))]));
        assert_eq!(
            parsed("NOT (a OR b)"),
            not(Expr::Or(vec![lit("a"), lit("b")]))
        );
    }

    #[test]
    fn lower_case_operators_are_words() {
        assert_eq!(
            parsed("this or that"),
            Expr::And(vec![lit("this"), lit("or"), lit("that")])
        );
    }

    #[test]
    fn quotes_keep_spaces_operators_and_escapes() {
        assert_eq!(parsed(r#""fn main()""#), lit("fn main()"));
        assert_eq!(parsed(r#""OR""#), lit("OR"));
        assert_eq!(parsed(r#""say \"hi\" \\ bye""#), lit(r#"say "hi" \ bye"#));
        // Unterminated while typing: up to the end.
        assert_eq!(parsed(r#""half open"#), lit("half open"));
    }

    #[test]
    fn slashes_delimit_regexes() {
        assert_eq!(parsed(r"/fn \w+/"), re(r"fn \w+"));
        assert_eq!(parsed(r"/a\/b/"), re("a/b"));
        assert_eq!(parsed(r"/\d+/ x"), Expr::And(vec![re(r"\d+"), lit("x")]));
        // A slash that doesn't end the token is part of the regex.
        assert_eq!(parsed("/a/b/"), re("a/b"));
        // Paths and comments aren't regexes.
        assert_eq!(parsed("/usr/bin"), lit("/usr/bin"));
        assert_eq!(parsed("//"), lit("//"));
        assert_eq!(parsed("(/x/)"), re("x"));
    }

    #[test]
    fn qualifiers_narrow_by_file() {
        assert_eq!(
            parsed("parse path:src/*.rs lang:Rust"),
            Expr::And(vec![
                lit("parse"),
                qualifier(Field::Path, "src/*.rs"),
                qualifier(Field::Language, "Rust"),
            ])
        );
        assert_eq!(
            parsed(r#"repo:api branch:main tag:owner:alice language:"Visual Basic""#),
            Expr::And(vec![
                qualifier(Field::Repo, "api"),
                qualifier(Field::Branch, "main"),
                qualifier(Field::Tag, "owner:alice"),
                qualifier(Field::Language, "Visual Basic"),
            ])
        );
        assert_eq!(
            parsed("PATH:/test_\\w+/"),
            Expr::Qualifier(Field::Path, Pattern::Regex(r"test_\w+".into()))
        );
        assert_eq!(parsed("content:path:"), lit("path:"));
    }

    #[test]
    fn a_dash_negates_a_qualifier_only() {
        assert_eq!(
            parsed("x -path:tests"),
            Expr::And(vec![lit("x"), not(qualifier(Field::Path, "tests"))])
        );
        assert_eq!(parsed("-1"), lit("-1"));
        assert_eq!(parsed("-foo"), lit("-foo"));
    }

    #[test]
    fn unknown_qualifiers_are_plain_text() {
        assert_eq!(parsed("std::io"), lit("std::io"));
        assert_eq!(parsed("http://x"), lit("http://x"));
    }

    #[test]
    fn unsupported_github_qualifiers_are_reported() {
        assert!(parse("symbol:Foo").unwrap_err().contains("symbol:"));
        assert!(parse("is:archived").is_err());
    }

    #[test]
    fn parentheses_inside_words_stay_literal() {
        assert_eq!(parsed("foo()"), lit("foo()"));
        assert_eq!(parsed("a)"), lit("a)"));
        assert_eq!(
            parsed("(bar(x) OR y)"),
            Expr::Or(vec![lit("bar(x)"), lit("y")])
        );
        assert_eq!(parsed(r"(a\) OR b)"), Expr::Or(vec![lit(r"a\)"), lit("b")]));
    }

    #[test]
    fn dangling_operators_and_groups_are_forgiven() {
        assert_eq!(parsed("a OR"), lit("a"));
        assert_eq!(parsed("OR a"), lit("a"));
        assert_eq!(parsed("a NOT"), lit("a"));
        assert_eq!(parsed("(a OR b"), Expr::Or(vec![lit("a"), lit("b")]));
        assert_eq!(parsed("a () b"), Expr::And(vec![lit("a"), lit("b")]));
    }
}
