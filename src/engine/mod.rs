//! The search engine: tgrep's trigram index narrows candidate files, then a
//! regex finds matching lines in each. Nothing here depends on the UI.

pub mod facets;
pub mod index;
pub mod language;
pub mod query;
pub mod registry;
pub mod repo;
pub mod search;
pub mod watch;
