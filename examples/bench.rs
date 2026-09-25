//! Time indexing and a few searches over a folder:
//! `cargo run --release --example bench -- <folder> [pattern...]`

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use tgrep_gpui::engine::query::{CompiledQuery, SearchQuery};
use tgrep_gpui::engine::repo::RepoInfo;
use tgrep_gpui::engine::search::{self, SearchLimits, SearchSource};
use tgrep_gpui::engine::workspace::{IndexStatus, Workspace};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let folder = args.next().expect("usage: bench <folder> [pattern...]");
    let mut patterns: Vec<String> = args.collect();
    if patterns.is_empty() {
        patterns = ["fn main", "HashMap", "unsafe impl Send", "TODO", "a"]
            .map(String::from)
            .to_vec();
    }

    let workspace = Workspace::open(folder.as_ref())?;
    if !matches!(workspace.index_status(), IndexStatus::Ready { .. }) {
        let started = Instant::now();
        workspace.build_index()?.publish()?;
        println!("index built in {:.2?}", started.elapsed());
    }
    let started = Instant::now();
    let corpus = std::sync::Arc::new(workspace.load_corpus());
    println!(
        "corpus loaded in {:.2?}: {} files, indexed: {}",
        started.elapsed(),
        corpus.file_count(),
        corpus.is_indexed()
    );

    let source = SearchSource {
        repo: std::sync::Arc::new(RepoInfo {
            id: folder.clone(),
            name: workspace.name(),
            root: workspace.root().to_path_buf(),
            branch: None,
            tags: vec![],
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
