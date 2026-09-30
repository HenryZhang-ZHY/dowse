//! A release on GitHub: its version, its page and notes, and the archive
//! built for each platform with its checksum.

use anyhow::{Context as _, Result, anyhow, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Where releases are published: GitHub's API, its website and the
/// repository, apart so tests can serve them from elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub api: String,
    pub web: String,
    /// `owner/name`.
    pub repo: String,
}

impl Source {
    /// dowse's own releases.
    pub fn github() -> Self {
        let repo = env!("CARGO_PKG_REPOSITORY")
            .trim_start_matches("https://github.com/")
            .trim_end_matches('/');
        Self {
            api: "https://api.github.com".into(),
            web: "https://github.com".into(),
            repo: repo.into(),
        }
    }

    /// The API's answer for the latest release, which leaves out drafts and
    /// prereleases.
    pub fn latest_api_url(&self) -> String {
        format!("{}/repos/{}/releases/latest", self.api, self.repo)
    }

    /// The website's page for the latest release, which redirects to the
    /// release's own page.
    pub fn latest_page_url(&self) -> String {
        format!("{}/{}/releases/latest", self.web, self.repo)
    }

    pub fn page_url(&self, tag: &str) -> String {
        format!("{}/{}/releases/tag/{tag}", self.web, self.repo)
    }

    pub fn download_url(&self, tag: &str, file: &str) -> String {
        format!("{}/{}/releases/download/{tag}/{file}", self.web, self.repo)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    /// `v<version>`.
    pub tag: String,
    /// The release's page on GitHub.
    pub page: String,
    /// The release notes, in Markdown; empty when unknown.
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// In bytes; `None` when unknown.
    #[serde(default)]
    pub size: Option<u64>,
    /// Lowercase hex; `None` when GitHub did not say.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl Release {
    /// Read the GitHub API's description of a release.
    pub fn from_api(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct ApiRelease {
            tag_name: String,
            html_url: String,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            assets: Vec<ApiAsset>,
        }
        #[derive(Deserialize)]
        struct ApiAsset {
            name: String,
            browser_download_url: String,
            #[serde(default)]
            size: Option<u64>,
            #[serde(default)]
            digest: Option<String>,
        }
        let release: ApiRelease =
            serde_json::from_str(json).context("GitHub's answer is not a release")?;
        Ok(Self {
            version: version_of_tag(&release.tag_name)?,
            tag: release.tag_name,
            page: release.html_url,
            notes: release.body.unwrap_or_default(),
            assets: release
                .assets
                .into_iter()
                .map(|asset| Asset {
                    sha256: asset
                        .digest
                        .as_deref()
                        .and_then(|digest| digest.strip_prefix("sha256:"))
                        .map(str::to_ascii_lowercase),
                    name: asset.name,
                    url: asset.browser_download_url,
                    size: asset.size,
                })
                .collect(),
        })
    }

    /// A release known only by its tag and its `SHA256SUMS`, for when the
    /// API is out of reach: every file the checksums list, downloaded from
    /// the website.
    pub fn from_tag(source: &Source, tag: &str, checksums: &str) -> Result<Self> {
        Ok(Self {
            version: version_of_tag(tag)?,
            tag: tag.into(),
            page: source.page_url(tag),
            notes: String::new(),
            assets: parse_checksums(checksums)
                .into_iter()
                .map(|(sha256, name)| Asset {
                    url: source.download_url(tag, &name),
                    name,
                    size: None,
                    sha256: Some(sha256),
                })
                .collect(),
        })
    }

    /// The archive built for `platform` (see [`current_platform`]).
    pub fn archive_for(&self, platform: &str) -> Option<&Asset> {
        let suffix = format!("-{platform}.");
        self.assets
            .iter()
            .find(|asset| asset.name.starts_with("dowse-") && asset.name.contains(&suffix))
    }
}

/// The version a tag names: `v1.2.0` is `1.2.0`.
pub fn version_of_tag(tag: &str) -> Result<Version> {
    let version = tag
        .strip_prefix('v')
        .ok_or_else(|| anyhow!("the release tag {tag} does not start with v"))?;
    Version::parse(version).with_context(|| format!("the release tag {tag} is not a version"))
}

/// The version running.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is a version")
}

/// Read `sha256sum`'s output: `<hex>  <name>` per line, `*` before the name
/// for binary mode. Returns `(hex, name)` pairs, hex in lowercase.
pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*');
            let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            (valid && !name.is_empty()).then(|| (hash.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

/// The name the release workflow gives this platform's archive, between
/// `dowse-<tag>-` and the extension; `None` where no archive is built.
pub fn current_platform() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(target_os = "macos") {
        Some("macos-universal")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else {
        None
    }
}

/// Fail unless `release` is newer than `current`.
pub fn ensure_newer(release: &Release, current: &Version) -> Result<()> {
    if release.version <= *current {
        bail!("dowse {current} is up to date (the latest release is {})", release.version);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "77224eb0a327cd43fa70b8655445b5969e867c1caef24a2d38fa8994895a6862";

    /// Trimmed from what the API answered for v1.1.1.
    fn api_json() -> String {
        format!(
            r##"{{
                "tag_name": "v1.1.1",
                "html_url": "https://github.com/HenryZhang-ZHY/dowse/releases/tag/v1.1.1",
                "draft": false,
                "prerelease": false,
                "body": "What's Changed\n* fix(ui): round result card header corners",
                "assets": [
                    {{
                        "name": "dowse-v1.1.1-windows-x86_64.zip",
                        "size": 16166848,
                        "digest": "sha256:{}",
                        "browser_download_url": "https://github.com/HenryZhang-ZHY/dowse/releases/download/v1.1.1/dowse-v1.1.1-windows-x86_64.zip"
                    }},
                    {{
                        "name": "dowse-v1.1.1-linux-x86_64.tar.gz",
                        "size": 21899891,
                        "digest": null,
                        "browser_download_url": "https://example.test/linux.tar.gz"
                    }},
                    {{
                        "name": "SHA256SUMS",
                        "size": 396,
                        "browser_download_url": "https://example.test/SHA256SUMS"
                    }}
                ]
            }}"##,
            HASH.to_uppercase()
        )
    }

    #[test]
    fn reads_the_apis_release() {
        let release = Release::from_api(&api_json()).unwrap();
        assert_eq!(release.version, Version::new(1, 1, 1));
        assert_eq!(release.tag, "v1.1.1");
        assert!(release.page.ends_with("/releases/tag/v1.1.1"));
        assert!(release.notes.starts_with("What's Changed"));
        assert_eq!(release.assets.len(), 3);

        let windows = release.archive_for("windows-x86_64").unwrap();
        assert_eq!(windows.name, "dowse-v1.1.1-windows-x86_64.zip");
        assert_eq!(windows.size, Some(16166848));
        assert_eq!(windows.sha256.as_deref(), Some(HASH));

        let linux = release.archive_for("linux-x86_64").unwrap();
        assert_eq!(linux.sha256, None);
        assert!(release.archive_for("linux-aarch64").is_none());
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_an_error() {
        let json = api_json().replace("\"v1.1.1\"", "\"nightly\"");
        assert!(Release::from_api(&json).is_err());
        assert!(version_of_tag("1.1.1").is_err());
        assert!(version_of_tag("v1.1").is_err());
        assert_eq!(
            version_of_tag("v2.0.0-rc.1").unwrap(),
            Version::parse("2.0.0-rc.1").unwrap()
        );
    }

    #[test]
    fn reads_checksums_as_sha256sum_writes_them() {
        let text = format!(
            "{HASH}  dowse-v1.1.1-windows-x86_64.zip\n\
             {}  *dowse-v1.1.1-macos-universal.zip\n\
             not a checksum line\n\
             \n",
            HASH.to_uppercase()
        );
        assert_eq!(
            parse_checksums(&text),
            vec![
                (HASH.to_string(), "dowse-v1.1.1-windows-x86_64.zip".to_string()),
                (HASH.to_string(), "dowse-v1.1.1-macos-universal.zip".to_string()),
            ]
        );
    }

    #[test]
    fn a_release_from_its_tag_downloads_from_the_website() {
        let source = Source {
            api: "http://api.test".into(),
            web: "http://web.test".into(),
            repo: "me/dowse".into(),
        };
        let checksums = format!("{HASH}  dowse-v1.2.0-macos-universal.zip\n");
        let release = Release::from_tag(&source, "v1.2.0", &checksums).unwrap();
        assert_eq!(release.version, Version::new(1, 2, 0));
        assert_eq!(release.page, "http://web.test/me/dowse/releases/tag/v1.2.0");
        let archive = release.archive_for("macos-universal").unwrap();
        assert_eq!(
            archive.url,
            "http://web.test/me/dowse/releases/download/v1.2.0/dowse-v1.2.0-macos-universal.zip"
        );
        assert_eq!(archive.sha256.as_deref(), Some(HASH));
    }

    #[test]
    fn dowses_own_releases_come_from_its_repository() {
        let source = Source::github();
        assert_eq!(source.repo, "HenryZhang-ZHY/dowse");
        assert_eq!(
            source.latest_api_url(),
            "https://api.github.com/repos/HenryZhang-ZHY/dowse/releases/latest"
        );
    }

    #[test]
    fn only_a_newer_release_is_offered() {
        let release = Release::from_api(&api_json()).unwrap();
        assert!(ensure_newer(&release, &Version::new(1, 1, 0)).is_ok());
        assert!(ensure_newer(&release, &Version::new(1, 1, 1)).is_err());
        assert!(ensure_newer(&release, &Version::new(1, 2, 0)).is_err());
    }

    #[test]
    fn this_platform_has_an_archive_name() {
        // Every platform dowse is built for on CI.
        if cfg!(any(windows, target_os = "macos", target_os = "linux")) {
            assert!(current_platform().is_some());
        }
    }
}
