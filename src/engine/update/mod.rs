//! Keeping dowse up to date from its GitHub releases: finding out whether a
//! newer version is out, and installing it over the one running.

pub mod client;
pub mod release;
pub mod state;
#[cfg(test)]
mod test_server;
