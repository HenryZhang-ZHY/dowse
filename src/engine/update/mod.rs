//! Keeping dowse up to date from its GitHub releases: finding out whether a
//! newer version is out ([`check`]), and installing it over the one running
//! ([`install`]).

pub mod client;
pub mod install;
pub mod release;
pub mod state;
#[cfg(test)]
mod test_server;

use std::time::SystemTime;

use anyhow::Result;

use client::{Answer, Client};
use state::UpdateState;

/// Ask GitHub for the latest release and note the answer in `state`.
pub fn check(client: &Client, state: &mut UpdateState, now: SystemTime) -> Result<()> {
    // Without the release it named, an unchanged answer says nothing.
    let etag = state.latest.as_ref().and(state.etag.as_deref());
    match client.latest(etag)? {
        Answer::Unchanged => state.record(None, None, now),
        Answer::Latest { release, etag } => state.record(Some(release), etag, now),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::release::Source;
    use super::test_server::{Reply, TestServer};
    use super::*;

    #[test]
    fn a_check_notes_the_release_and_asks_with_its_etag_next_time() {
        let json = r#"{"tag_name": "v9.0.0", "html_url": "https://example.test", "assets": []}"#;
        let server = TestServer::start(vec![(
            "/repos/me/dowse/releases/latest".into(),
            Reply::ok(json).header("ETag", "\"nine\""),
        )]);
        let client = Client::for_tests(Source {
            api: server.base.clone(),
            web: server.base.clone(),
            repo: "me/dowse".into(),
        });
        let mut state = UpdateState {
            etag: Some("\"stale, no release\"".into()),
            ..Default::default()
        };
        check(&client, &mut state, SystemTime::now()).unwrap();
        assert_eq!(state.latest.as_ref().unwrap().tag, "v9.0.0");
        assert_eq!(state.etag.as_deref(), Some("\"nine\""));
        assert!(!state.is_due(SystemTime::now()));
        assert!(!server.seen()[0].headers.contains_key("if-none-match"));

        check(&client, &mut state, SystemTime::now()).unwrap();
        assert_eq!(server.seen()[1].headers["if-none-match"], "\"nine\"");
    }
}
