//! Installing a release over the dowse that is running: download this
//! platform's archive and check it against its SHA-256, unpack it, make
//! sure its program starts here, then put it where the running one is.
//!
//! A running program cannot be overwritten on Windows, but it can be
//! renamed: each file or bundle being replaced moves aside to `<name>.old`
//! first, the new one takes its place, and the old one is removed when it
//! can be, on the next start at the latest. Everything new is copied beside
//! its target before anything is moved, so a full disk or a folder dowse
//! cannot write to fails before the running version is touched.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use semver::Version;
use sha2::{Digest as _, Sha256};

use super::client::Client;
use super::release::{Asset, Release, current_platform, parse_checksums};

/// How long the new program may take to say its version.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Where an install is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installation {
    /// Programs in a folder: `dowse.exe` beside `dowse.com` on Windows,
    /// `dowse` elsewhere. `program` is the one running.
    Programs { program: PathBuf },
    /// `dowse.app` on macOS.
    Bundle { app: PathBuf },
}

/// How an install is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// Bytes downloaded, of how many when known.
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Unpacking,
    Installing,
}

/// One file or bundle to put in place.
#[derive(Debug)]
struct Replacement {
    new: PathBuf,
    target: PathBuf,
}

impl Installation {
    /// The install `exe`, the running program, belongs to.
    pub fn detect(exe: &Path) -> Result<Self> {
        let exe = fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
        if let Some(app) = bundle_of(&exe) {
            if app
                .components()
                .any(|part| part.as_os_str() == "AppTranslocation")
            {
                bail!(
                    "macOS runs dowse from a read-only copy; move dowse.app to Applications and \
                     start it from there to install updates"
                );
            }
            return Ok(Self::Bundle { app });
        }
        Ok(Self::Programs { program: exe })
    }

    /// The program to start once installed.
    pub fn program(&self) -> PathBuf {
        match self {
            Self::Programs { program } => program.clone(),
            Self::Bundle { app } => app.join("Contents/MacOS/dowse"),
        }
    }

    /// What in `unpacked`, a release's unpacked archive, goes where.
    fn replacements(&self, unpacked: &Path) -> Result<Vec<Replacement>> {
        let replacements = match self {
            Self::Bundle { app } => vec![Replacement {
                new: unpacked.join("dowse.app"),
                target: app.clone(),
            }],
            Self::Programs { program } if cfg!(windows) => {
                let dir = program.parent().unwrap_or(Path::new("."));
                vec![
                    Replacement {
                        new: unpacked.join("dowse.exe"),
                        target: program.clone(),
                    },
                    // The console program, which terminals run for `dowse`.
                    Replacement {
                        new: unpacked.join("dowse.com"),
                        target: dir.join("dowse.com"),
                    },
                ]
            }
            Self::Programs { program } => {
                let bundled = unpacked.join("dowse.app/Contents/MacOS/dowse");
                vec![Replacement {
                    new: if bundled.exists() {
                        bundled
                    } else {
                        unpacked.join("dowse")
                    },
                    target: program.clone(),
                }]
            }
        };
        if let Some(missing) = replacements.iter().find(|r| !r.new.exists()) {
            bail!(
                "the release's archive has no {}",
                missing
                    .new
                    .strip_prefix(unpacked)
                    .unwrap_or(&missing.new)
                    .display()
            );
        }
        Ok(replacements)
    }

    /// The program in `unpacked` to ask for its version: the console
    /// program on Windows, since the app has no console to answer on.
    fn probe(&self, unpacked: &Path) -> PathBuf {
        if cfg!(windows) {
            unpacked.join("dowse.com")
        } else if cfg!(target_os = "macos") {
            unpacked.join("dowse.app/Contents/MacOS/dowse")
        } else {
            unpacked.join("dowse")
        }
    }

    /// Remove what earlier installs left: the files they moved aside.
    pub fn remove_leftovers(&self) {
        let targets = match self {
            Self::Bundle { app } => vec![app.clone()],
            Self::Programs { program } => {
                let mut targets = vec![program.clone()];
                if cfg!(windows)
                    && let Some(dir) = program.parent()
                {
                    targets.push(dir.join("dowse.com"));
                }
                targets
            }
        };
        for target in targets {
            for leftover in [aside(&target), staged(&target)] {
                if leftover.exists() && remove(&leftover).is_ok() {
                    log::info!("removed {} left by an update", leftover.display());
                }
            }
        }
    }
}

/// `…/dowse.app` when `exe` is `…/dowse.app/Contents/MacOS/<program>`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    is_bundle.then(|| app.to_path_buf())
}

/// Install `release` over `installation`, working in `work_dir` (emptied
/// first, removed after). Stops with an error soon after `cancel` is set,
/// up to the moment files start moving.
pub fn install(
    client: &Client,
    release: &Release,
    installation: &Installation,
    work_dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<()> {
    let platform = current_platform()
        .ok_or_else(|| anyhow!("no build of dowse is released for this platform"))?;
    let asset = release
        .archive_for(platform)
        .ok_or_else(|| anyhow!("dowse {} has no build for {platform}", release.version))?;
    let sha256 = expected_sha256(client, release, asset)?;

    if work_dir.exists() {
        fs::remove_dir_all(work_dir)
            .with_context(|| format!("cannot empty {}", work_dir.display()))?;
    }
    fs::create_dir_all(work_dir)?;
    let result = (|| {
        let archive = work_dir.join(&asset.name);
        download(client, asset, &sha256, &archive, cancel, &mut progress)?;
        progress(Progress::Unpacking);
        let unpacked = unpack(&archive, &work_dir.join("unpacked"))?;
        let replacements = installation.replacements(&unpacked)?;
        ensure_runs(&installation.probe(&unpacked), &release.version)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        progress(Progress::Installing);
        replace(&replacements)
    })();
    fs::remove_dir_all(work_dir).ok();
    result
}

/// The archive's SHA-256: from GitHub's digest, or the release's checksums.
fn expected_sha256(client: &Client, release: &Release, asset: &Asset) -> Result<String> {
    if let Some(sha256) = &asset.sha256 {
        return Ok(sha256.clone());
    }
    let sums = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS")
        .ok_or_else(|| anyhow!("dowse {} has no checksums to check it by", release.version))?;
    let text = client
        .get(&sums.url)?
        .body_mut()
        .read_to_string()
        .context("cannot read the release's checksums")?;
    parse_checksums(&text)
        .into_iter()
        .find(|(_, name)| *name == asset.name)
        .map(|(sha256, _)| sha256)
        .ok_or_else(|| anyhow!("the release's checksums leave out {}", asset.name))
}

/// Download `asset` to `file`, failing unless its SHA-256 is `sha256`.
pub fn download(
    client: &Client,
    asset: &Asset,
    sha256: &str,
    file: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    let mut response = client.get(&asset.url)?;
    let total = asset.size.or_else(|| {
        response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok()?.parse().ok())
    });
    let partial = file.with_extension("part");
    let result = (|| {
        let mut body = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut out = io::BufWriter::new(File::create(&partial)?);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut done = 0u64;
        let mut reported = Instant::now();
        progress(Progress::Downloading { done, total });
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let read = body
                .read(&mut buffer)
                .with_context(|| format!("the download of {} broke off", asset.name))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
            done += read as u64;
            if reported.elapsed() >= Duration::from_millis(100) {
                reported = Instant::now();
                progress(Progress::Downloading { done, total });
            }
        }
        out.flush()?;
        progress(Progress::Downloading { done, total });
        let actual = format!("{:x}", hasher.finalize());
        if actual != sha256 {
            bail!(
                "{} is not what was released: its SHA-256 is {actual}, not {sha256}",
                asset.name
            );
        }
        Ok(())
    })();
    match result {
        Ok(()) => fs::rename(&partial, file).map_err(Into::into),
        Err(error) => {
            fs::remove_file(&partial).ok();
            Err(error)
        }
    }
}

/// Unpack a `.zip` or `.tar.gz` into `dir`. Returns the folder holding the
/// release's files: the one folder the archive holds, as the release
/// workflow packs them, or `dir` itself.
pub fn unpack(archive: &Path, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let what = || format!("cannot unpack {name}");
    if name.ends_with(".zip") {
        zip::ZipArchive::new(File::open(archive)?)
            .and_then(|mut zip| zip.extract(dir))
            .with_context(what)?;
    } else if name.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(File::open(archive)?);
        let mut tar = tar::Archive::new(gz);
        tar.set_preserve_permissions(true);
        tar.unpack(dir).with_context(what)?;
    } else {
        bail!("dowse cannot unpack {name}");
    }
    let entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    Ok(match entries.as_slice() {
        [only] if only.is_dir() && only.extension().is_none_or(|ext| ext != "app") => only.clone(),
        _ => dir.to_path_buf(),
    })
}

/// Fail unless `program` starts here and says it is `version`: a build for
/// another processor, or needing a newer C library than this Linux has,
/// fails before it replaces anything.
fn ensure_runs(program: &Path, version: &Version) -> Result<()> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "the new version does not start here ({})",
                program.display()
            )
        })?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > PROBE_TIMEOUT {
            child.kill().ok();
            bail!("the new version did not answer when asked for its version");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_string(&mut out).ok();
    }
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut err).ok();
    }
    if !status.success() {
        bail!("the new version does not run here: {}", err.trim());
    }
    if !out
        .split_whitespace()
        .any(|word| word == version.to_string())
    {
        bail!("the new program says it is {:?}, not {version}", out.trim());
    }
    Ok(())
}

/// Where `target` moves aside to.
fn aside(target: &Path) -> PathBuf {
    with_suffix(target, ".old")
}

/// Where the new `target` is copied before it takes its place.
fn staged(target: &Path) -> PathBuf {
    with_suffix(target, ".new")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Put each new file or bundle in place of its target, all or none.
fn replace(replacements: &[Replacement]) -> Result<()> {
    // Copy everything beside its target first: the step that fails when
    // the folder cannot be written or the disk is full.
    for replacement in replacements {
        let staged = staged(&replacement.target);
        if staged.exists() {
            remove(&staged)?;
        }
        copy(&replacement.new, &staged).map_err(|error| {
            let folder = replacement.target.parent().unwrap_or(Path::new("."));
            anyhow!(
                "dowse cannot write to {} ({error}); download the new version from its release \
                 page instead",
                folder.display()
            )
        })?;
    }
    let mut done: Vec<&Replacement> = Vec::new();
    for replacement in replacements {
        if let Err(error) = swap(&replacement.target) {
            for undone in done.into_iter().rev() {
                unswap(&undone.target).ok();
            }
            for replacement in replacements {
                remove(&staged(&replacement.target)).ok();
            }
            return Err(error);
        }
        done.push(replacement);
    }
    // Running programs cannot be removed on Windows; the next start does it.
    for replacement in replacements {
        remove(&aside(&replacement.target)).ok();
    }
    Ok(())
}

/// Move `target` aside and its staged copy into its place.
fn swap(target: &Path) -> Result<()> {
    let aside = aside(target);
    if aside.exists() {
        remove(&aside).with_context(|| format!("cannot remove {}", aside.display()))?;
    }
    let existed = target.exists();
    if existed {
        fs::rename(target, &aside)
            .with_context(|| format!("cannot move {} aside", target.display()))?;
    }
    if let Err(error) = fs::rename(staged(target), target) {
        if existed {
            fs::rename(&aside, target).ok();
        }
        return Err(error).with_context(|| format!("cannot replace {}", target.display()));
    }
    Ok(())
}

/// Undo [`swap`]: put back what was moved aside.
fn unswap(target: &Path) -> io::Result<()> {
    let aside = aside(target);
    if aside.exists() {
        remove(target).ok();
        fs::rename(&aside, target)?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        // Keeps the permissions, so programs stay executable.
        fs::copy(from, to).map(|_| ())
    }
}

fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::super::release::Source;
    use super::super::test_server::{Reply, TestServer};
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn zip_of(files: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, text, mode) in files {
            let options = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            zip.start_file(*name, options).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    fn tar_gz_of(files: &[(&str, &str, u32)]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, text, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            tar.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn detects_a_bundle_and_loose_programs() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("Apps/dowse.app/Contents/MacOS/dowse");
        write(&bundled, "");
        let Installation::Bundle { app } = Installation::detect(&bundled).unwrap() else {
            panic!("expected a bundle");
        };
        assert!(app.ends_with("dowse.app"));

        let loose = dir.path().join("bin/dowse");
        write(&loose, "");
        let installation = Installation::detect(&loose).unwrap();
        assert!(matches!(installation, Installation::Programs { .. }));
        assert!(installation.program().ends_with("bin/dowse"));

        let translocated = dir
            .path()
            .join("AppTranslocation/ABC/d/dowse.app/Contents/MacOS/dowse");
        write(&translocated, "");
        let error = Installation::detect(&translocated).unwrap_err();
        assert!(error.to_string().contains("Applications"), "{error}");
    }

    #[test]
    fn unpacks_a_zip_into_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("dowse-v1.2.0-windows-x86_64.zip");
        fs::write(
            &archive,
            zip_of(&[
                ("dowse-v1.2.0-windows-x86_64/dowse.exe", "new exe", 0o755),
                ("dowse-v1.2.0-windows-x86_64/README.md", "readme", 0o644),
            ]),
        )
        .unwrap();
        let root = unpack(&archive, &dir.path().join("out")).unwrap();
        assert!(root.ends_with("dowse-v1.2.0-windows-x86_64"));
        assert_eq!(read(&root.join("dowse.exe")), "new exe");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(root.join("dowse.exe"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn unpacks_a_tar_gz_keeping_programs_executable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("dowse-v1.2.0-linux-x86_64.tar.gz");
        fs::write(
            &archive,
            tar_gz_of(&[("dowse-v1.2.0-linux-x86_64/dowse", "new program", 0o755)]),
        )
        .unwrap();
        let root = unpack(&archive, &dir.path().join("out")).unwrap();
        assert_eq!(read(&root.join("dowse")), "new program");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(root.join("dowse"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn a_zip_reaching_outside_its_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.zip");
        fs::write(&archive, zip_of(&[("../escaped", "gotcha", 0o644)])).unwrap();
        assert!(unpack(&archive, &dir.path().join("out")).is_err());
        assert!(!dir.path().join("escaped").exists());
    }

    #[test]
    fn replaces_files_and_bundles_moving_the_old_aside_then_removing_it() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new");
        write(&new.join("dowse.exe"), "new exe");
        write(&new.join("dowse.app/Contents/MacOS/dowse"), "new app");
        let install = dir.path().join("install");
        write(&install.join("dowse.exe"), "old exe");
        write(&install.join("dowse.app/Contents/MacOS/dowse"), "old app");
        write(&install.join("dowse.app/Contents/Stale"), "only in the old");

        replace(&[
            Replacement {
                new: new.join("dowse.exe"),
                target: install.join("dowse.exe"),
            },
            Replacement {
                new: new.join("dowse.app"),
                target: install.join("dowse.app"),
            },
            // A file the old install did not have.
            Replacement {
                new: new.join("dowse.exe"),
                target: install.join("dowse.com"),
            },
        ])
        .unwrap();

        assert_eq!(read(&install.join("dowse.exe")), "new exe");
        assert_eq!(read(&install.join("dowse.com")), "new exe");
        assert_eq!(
            read(&install.join("dowse.app/Contents/MacOS/dowse")),
            "new app"
        );
        assert!(!install.join("dowse.app/Contents/Stale").exists());
        let mut left: Vec<String> = fs::read_dir(&install)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["dowse.app", "dowse.com", "dowse.exe"]);
    }

    #[test]
    fn a_failed_copy_leaves_the_old_install_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new");
        write(&new.join("dowse.exe"), "new exe");
        let install = dir.path().join("install");
        write(&install.join("dowse.exe"), "old exe");
        let error = replace(&[
            Replacement {
                new: new.join("dowse.exe"),
                target: install.join("dowse.exe"),
            },
            Replacement {
                new: new.join("missing"),
                target: install.join("dowse.com"),
            },
        ])
        .unwrap_err();
        assert!(error.to_string().contains("cannot write"), "{error}");
        assert_eq!(read(&install.join("dowse.exe")), "old exe");
        assert!(!install.join("dowse.com").exists());
    }

    /// Windows will not overwrite or remove a running program, but renames it.
    #[cfg(windows)]
    #[test]
    fn replaces_a_program_while_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("dowse.exe");
        let system = std::env::var_os("SystemRoot").unwrap();
        fs::copy(Path::new(&system).join("System32/PING.EXE"), &program).unwrap();
        let mut running = Command::new(&program)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            fs::remove_file(&program).is_err(),
            "Windows let a running program go"
        );
        write(&dir.path().join("new/dowse.exe"), "new exe");

        replace(&[Replacement {
            new: dir.path().join("new/dowse.exe"),
            target: program.clone(),
        }])
        .unwrap();
        assert_eq!(read(&program), "new exe");
        // Still running, so still there.
        assert!(aside(&program).exists());

        running.kill().unwrap();
        running.wait().unwrap();
        Installation::Programs {
            program: program.clone(),
        }
        .remove_leftovers();
        assert!(!aside(&program).exists());
    }

    #[test]
    fn leftovers_of_an_earlier_install_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("dowse.exe");
        write(&program, "running");
        write(&aside(&program), "the version before");
        write(&staged(&program), "half an install");
        Installation::Programs {
            program: program.clone(),
        }
        .remove_leftovers();
        assert!(program.exists());
        assert!(!aside(&program).exists());
        assert!(!staged(&program).exists());
    }

    fn served_asset(server: &TestServer, bytes: &[u8]) -> Asset {
        Asset {
            name: "dowse-v1.2.0-linux-x86_64.tar.gz".into(),
            url: format!("{}/archive", server.base),
            size: Some(bytes.len() as u64),
            sha256: Some(sha256(bytes)),
        }
    }

    fn client(server: &TestServer) -> Client {
        Client::for_tests(Source {
            api: server.base.clone(),
            web: server.base.clone(),
            repo: "me/dowse".into(),
        })
    }

    #[test]
    fn downloads_and_checks_the_archive() {
        let bytes = vec![7u8; 300_000];
        let server = TestServer::start(vec![("/archive".into(), Reply::ok(bytes.clone()))]);
        let asset = served_asset(&server, &bytes);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(&asset.name);
        let mut seen = Vec::new();
        download(
            &client(&server),
            &asset,
            asset.sha256.as_deref().unwrap(),
            &file,
            &AtomicBool::new(false),
            &mut |progress| seen.push(progress),
        )
        .unwrap();
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert_eq!(
            seen.last(),
            Some(&Progress::Downloading {
                done: 300_000,
                total: Some(300_000)
            })
        );
    }

    #[test]
    fn an_archive_with_the_wrong_checksum_is_thrown_away() {
        let server = TestServer::start(vec![("/archive".into(), Reply::ok("tampered"))]);
        let asset = served_asset(&server, b"what was released");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(&asset.name);
        let error = download(
            &client(&server),
            &asset,
            asset.sha256.as_deref().unwrap(),
            &file,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("not what was released"),
            "{error}"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_checksum_comes_from_sha256sums_when_github_gives_none() {
        let bytes = b"archive";
        let server = TestServer::start(vec![(
            "/sums".into(),
            Reply::ok(format!(
                "{}  dowse-v1.2.0-linux-x86_64.tar.gz\n",
                sha256(bytes)
            )),
        )]);
        let mut asset = served_asset(&server, bytes);
        asset.sha256 = None;
        let release = Release {
            version: Version::new(1, 2, 0),
            tag: "v1.2.0".into(),
            page: String::new(),
            notes: String::new(),
            assets: vec![
                asset.clone(),
                Asset {
                    name: "SHA256SUMS".into(),
                    url: format!("{}/sums", server.base),
                    size: None,
                    sha256: None,
                },
            ],
        };
        assert_eq!(
            expected_sha256(&client(&server), &release, &asset).unwrap(),
            sha256(bytes)
        );
    }

    /// Installs the real latest release into a scratch folder:
    /// `cargo test -- --ignored installs_the_latest_release`.
    #[test]
    #[ignore]
    fn installs_the_latest_release() {
        let client = Client::new(Source::github());
        let super::super::client::Answer::Latest { release, .. } = client.latest(None).unwrap()
        else {
            panic!("expected a release");
        };
        let dir = tempfile::tempdir().unwrap();
        let program = if cfg!(windows) {
            dir.path().join("install/dowse.exe")
        } else {
            dir.path().join("install/dowse")
        };
        write(&program, "old");
        let installation = Installation::Programs {
            program: program.clone(),
        };
        let mut steps = Vec::new();
        install(
            &client,
            &release,
            &installation,
            &dir.path().join("work"),
            &AtomicBool::new(false),
            |progress| {
                if !matches!(progress, Progress::Downloading { .. }) {
                    steps.push(progress);
                }
            },
        )
        .unwrap();
        assert_eq!(steps, [Progress::Unpacking, Progress::Installing]);
        assert!(fs::metadata(&program).unwrap().len() > 1_000_000);
        if cfg!(windows) {
            ensure_runs(&program.with_file_name("dowse.com"), &release.version).unwrap();
        } else {
            ensure_runs(&program, &release.version).unwrap();
        }
        assert!(!dir.path().join("work").exists());
    }
}
