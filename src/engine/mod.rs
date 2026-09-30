//! The search engine: tgrep's trigram index narrows candidate files, then a
//! regex finds matching lines in each. Also the saved settings: the library
//! of repositories, workspace files and the session. Nothing here depends on
//! the UI.

pub mod config;
pub mod facets;
pub mod github;
pub mod index;
pub mod language;
pub mod library;
pub mod preview;
pub mod process;
pub mod query;
pub mod repo;
pub mod search;
pub mod session;
pub mod settings;
mod store;
pub mod sync;
pub mod syntax;
pub mod table;
pub mod tasks;
pub mod watch;
pub mod workspace;
