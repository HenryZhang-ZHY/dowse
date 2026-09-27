//! Time indexing and a few searches over a folder:
//! `cargo run --release --example bench -- <folder> [--update] [pattern...]`
//!
//! With `--update`, an existing index is brought up to date first.

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use dowse::engine::index::{IndexStatus, IndexUpdate, RepoIndex};
use dowse::engine::query::{CompiledQuery, SearchQuery};
use dowse::engine::repo::RepoInfo;
use dowse::engine::search::{self, SearchLimits, SearchSource};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let folder = args
        .next()
        .expect("usage: bench <folder> [--update] [pattern...]");
    let mut patterns: Vec<String> = args.collect();
    let update = patterns.first().is_some_and(|arg| arg == "--update");
    if update {
        patterns.remove(0);
    }
    if patterns.is_empty() {
        patterns = ["fn main", "HashMap", "unsafe impl Send", "TODO", "a"]
            .map(String::from)
            .to_vec();
    }

    let index = RepoIndex::open(folder.as_ref())?;
    if !matches!(index.index_status(), IndexStatus::Ready { .. }) {
        let started = Instant::now();
        index.build_index()?.publish()?;
        println!("index built in {:.2?}", started.elapsed());
    } else {
        let started = Instant::now();
        let stale = index.stale_files().unwrap_or_default();
        println!(
            "{} files differ from the index, found in {:.2?}",
            stale.len(),
            started.elapsed()
        );
        if update {
            let started = Instant::now();
            match index.update_index()? {
                IndexUpdate::UpToDate => println!("index up to date"),
                IndexUpdate::Merged { staged, changes } => {
                    staged.publish()?;
                    println!("index updated: {changes:?}");
                }
                IndexUpdate::Rebuilt { staged, reason } => {
                    staged.publish()?;
                    println!("index rebuilt: {reason}");
                }
            }
            println!("in {:.2?}", started.elapsed());
        }
    }
    let started = Instant::now();
    let corpus = std::sync::Arc::new(index.load_corpus());
    println!(
        "corpus loaded in {:.2?}: {} files, indexed: {}",
        started.elapsed(),
        corpus.file_count(),
        corpus.is_indexed()
    );

    let source = SearchSource {
        repo: std::sync::Arc::new(RepoInfo {
            id: folder.clone(),
            name: index.name(),
            root: index.root().to_path_buf(),
            branch: None,
            tags: vec![],
            pull_every: None,
        }),
        corpus,
        changed: vec![],
    };
    for pattern in patterns {
        let query = SearchQuery {
            pattern: pattern.clone(),
            ..Default::default()
        };
        let compiled = CompiledQuery::new(&query).map_err(anyhow::Error::msg)?;
        let outcome = search::search(
            std::slice::from_ref(&source),
            &compiled,
            &SearchLimits::default(),
            &AtomicBool::new(false),
        );
        println!(
            "{pattern:>20}: {:>6} lines in {:>5} files, searched {:>6}, {:>8.2?}{}",
            outcome.matched_lines,
            outcome.files.len(),
            outcome.searched_files,
            outcome.elapsed,
            if outcome.truncated {
                " (truncated)"
            } else {
                ""
            }
        );
    }
    Ok(())
}
