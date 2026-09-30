//! Asking GitHub for the latest release. The API is asked first, with the
//! last answer's ETag so an unchanged answer costs nothing. Without signing
//! in it allows 60 requests an hour per address, which people behind one
//! office address share; when it refuses, the website's redirect to the
//! latest release and the release's `SHA256SUMS` tell the same.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

use super::release::{Release, Source};

/// How long a question about the latest release may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// What GitHub said of the latest release.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// The same as when it gave the ETag it was asked with.
    Unchanged,
    Latest {
        release: Release,
        etag: Option<String>,
    },
}

pub struct Client {
    agent: Agent,
    source: Source,
}

impl Client {
    /// A client for `source`, through the proxy the environment or the
    /// system names, trusting the certificates the system trusts (so an
    /// office proxy that inspects TLS works as it does in the browser).
    pub fn new(source: Source) -> Self {
        let agent = Agent::config_builder()
            .user_agent(format!("dowse/{}", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Self { agent, source }
    }

    /// A client for a test server: no proxy.
    #[cfg(test)]
    pub fn for_tests(source: Source) -> Self {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .into();
        Self { agent, source }
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Ask for the latest release. `etag` is the last answer's.
    pub fn latest(&self, etag: Option<&str>) -> Result<Answer> {
        match self.latest_from_api(etag)? {
            Some(answer) => Ok(answer),
            None => self.latest_from_website(),
        }
    }

    /// The API's answer, or `None` when it refuses to give one.
    fn latest_from_api(&self, etag: Option<&str>) -> Result<Option<Answer>> {
        let mut request = self
            .agent
            .get(self.source.latest_api_url())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request
            .config()
            .timeout_global(Some(CHECK_TIMEOUT))
            .build()
            .call()
            .context("cannot reach GitHub")?;
        match response.status().as_u16() {
            200 => {
                let etag = header(&response, "etag");
                let json = response.body_mut().read_to_string()?;
                Ok(Some(Answer::Latest {
                    release: Release::from_api(&json)?,
                    etag,
                }))
            }
            304 => Ok(Some(Answer::Unchanged)),
            // Rate limited.
            403 | 429 => {
                log::info!("GitHub's API refused to answer; asking its website");
                Ok(None)
            }
            404 => bail!("{} has no releases yet", self.source.repo),
            status => bail!("GitHub answered {status} when asked for the latest release"),
        }
    }

    /// The website's answer: the tag its latest release page redirects to,
    /// and that release's checksums.
    fn latest_from_website(&self) -> Result<Answer> {
        let response = self
            .agent
            .get(self.source.latest_page_url())
            .config()
            .max_redirects(0)
            .timeout_global(Some(CHECK_TIMEOUT))
            .build()
            .call()
            .context("cannot reach GitHub")?;
        let location = header(&response, "location")
            .ok_or_else(|| anyhow!("GitHub did not say which release is the latest"))?;
        let tag = location
            .rsplit_once("/releases/tag/")
            .map(|(_, tag)| tag.trim_end_matches('/'))
            .filter(|tag| !tag.is_empty() && !tag.contains('/'))
            .ok_or_else(|| anyhow!("GitHub named no release for the latest one ({location})"))?;
        let checksums = self
            .get(&self.source.download_url(tag, "SHA256SUMS"))?
            .body_mut()
            .read_to_string()
            .context("cannot read the release's checksums")?;
        Ok(Answer::Latest {
            release: Release::from_tag(&self.source, tag, &checksums)?,
            etag: None,
        })
    }

    /// Get `url`, following redirects, failing unless it answers 200.
    pub fn get(&self, url: &str) -> Result<Response<Body>> {
        let response = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("cannot download {url}"))?;
        match response.status().as_u16() {
            200 => Ok(response),
            status => bail!("GitHub answered {status} for {url}"),
        }
    }
}

fn header(response: &Response<Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::test_server::{Reply, TestServer};
    use super::*;

    const HASH: &str = "8bfba7d5fdf2c3aaf8a21fa6c90f3ede7e952be4f4b2e52c158ce316057b6061";

    fn release_json(tag: &str) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "html_url": "https://example.test/{tag}", "body": "notes",
                "assets": [{{"name": "dowse-{tag}-linux-x86_64.tar.gz", "size": 3,
                "digest": "sha256:{HASH}", "browser_download_url": "https://example.test/a"}}]}}"#
        )
    }

    fn client(server: &TestServer) -> Client {
        Client::for_tests(Source {
            api: format!("{}/api", server.base),
            web: format!("{}/web", server.base),
            repo: "me/dowse".into(),
        })
    }

    const API: &str = "/api/repos/me/dowse/releases/latest";

    #[test]
    fn reads_the_latest_release_and_its_etag() {
        let server = TestServer::start(vec![(
            API.into(),
            Reply::ok(release_json("v1.2.0")).header("ETag", "W/\"one\""),
        )]);
        let answer = client(&server).latest(None).unwrap();
        let Answer::Latest { release, etag } = answer else {
            panic!("expected a release, got {answer:?}");
        };
        assert_eq!(release.tag, "v1.2.0");
        assert_eq!(release.notes, "notes");
        assert_eq!(etag.as_deref(), Some("W/\"one\""));
        let seen = server.seen();
        assert_eq!(seen[0].headers["accept"], "application/vnd.github+json");
        assert!(!seen[0].headers.contains_key("if-none-match"));
    }

    #[test]
    fn asks_with_the_etag_and_hears_unchanged() {
        let server = TestServer::start(vec![(API.into(), Reply::status(304))]);
        let answer = client(&server).latest(Some("W/\"one\"")).unwrap();
        assert_eq!(answer, Answer::Unchanged);
        assert_eq!(server.seen()[0].headers["if-none-match"], "W/\"one\"");
    }

    #[test]
    fn asks_the_website_when_the_api_is_rate_limited() {
        let server = TestServer::start(vec![
            (API.into(), Reply::status(403)),
            (
                "/web/me/dowse/releases/latest".into(),
                Reply::status(302).header(
                    "Location",
                    "https://github.com/me/dowse/releases/tag/v1.3.0",
                ),
            ),
            (
                "/web/me/dowse/releases/download/v1.3.0/SHA256SUMS".into(),
                Reply::ok(format!("{HASH}  dowse-v1.3.0-windows-x86_64.zip\n")),
            ),
        ]);
        let Answer::Latest { release, etag } = client(&server).latest(None).unwrap() else {
            panic!("expected a release");
        };
        assert_eq!(release.tag, "v1.3.0");
        assert_eq!(etag, None);
        let archive = release.archive_for("windows-x86_64").unwrap();
        assert_eq!(
            archive.url,
            format!(
                "{}/web/me/dowse/releases/download/v1.3.0/dowse-v1.3.0-windows-x86_64.zip",
                server.base
            )
        );
        assert_eq!(archive.sha256.as_deref(), Some(HASH));
        let paths: Vec<String> = server.seen().into_iter().map(|seen| seen.path).collect();
        assert_eq!(paths.len(), 3, "{paths:?}");
    }

    /// Asks the real GitHub: `cargo test -- --ignored latest_from_github`.
    #[test]
    #[ignore]
    fn latest_from_github() {
        let client = Client::new(Source::github());
        let Answer::Latest { release, etag } = client.latest(None).unwrap() else {
            panic!("expected a release");
        };
        assert!(etag.is_some());
        assert!(
            release
                .archive_for("windows-x86_64")
                .unwrap()
                .sha256
                .is_some()
        );
        let Answer::Latest {
            release: from_website,
            ..
        } = client.latest_from_website().unwrap()
        else {
            panic!("expected a release");
        };
        assert_eq!(from_website.version, release.version);
        assert_eq!(client.latest(etag.as_deref()).unwrap(), Answer::Unchanged);
    }

    #[test]
    fn a_repository_without_releases_is_an_error() {
        let server = TestServer::start(vec![]);
        let error = client(&server).latest(None).unwrap_err();
        assert!(format!("{error:#}").contains("no releases"), "{error:#}");
    }

    #[test]
    fn a_redirect_to_no_release_is_an_error() {
        let server = TestServer::start(vec![
            (API.into(), Reply::status(429)),
            (
                "/web/me/dowse/releases/latest".into(),
                Reply::status(302).header("Location", "https://github.com/me/dowse/releases"),
            ),
        ]);
        assert!(client(&server).latest(None).is_err());
    }
}
