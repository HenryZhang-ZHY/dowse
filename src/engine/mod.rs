//! The search engine: tgrep's trigram index narrows candidate files, then a
//! regex finds matching lines in each. Also the saved settings: the library
//! of repositories, workspace files and the session. Nothing here depends on
//! the UI.

pub mod config;
pub mod facets;
pub mod index;
pub mod language;
pub mod library;
pub mod query;
pub mod registry;
pub mod repo;
pub mod search;
pub mod session;
mod store;
pub mod watch;
pub mod workspace;
