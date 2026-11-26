use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use crate::codegen_rust::{PreludeMode, TranspileOptions};
use crate::from_rust::{FromRustOptions, convert_from_rust_with_hints};
use crate::ra_oracle::{RustAnalyzerOracle, TypeHintMap};
use crate::{count_raw_rust_blocks, transpile_to_rust};

// Match the CLI main thread stack so deeply nested Rust expressions never overflow Windows' 1 MB default stack.
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Configuration for parallel Rust -> R# project conversion.
#[derive(Debug, Clone, Default)]
pub struct ProjectConvertOptions {
    pub out_dir: Option<PathBuf>,
    pub verify: bool,
    pub strict_unnameable: bool,
}

/// Input entry for a single Rust source file within a project.
#[derive(Debug, Clone)]
pub struct ProjectFile {
    pub path: PathBuf,
    pub rel: PathBuf,
}

/// Outcome of converting a single project file, preserving original input order.
#[derive(Debug, Clone)]
pub struct ProjectFileOutcome {
    pub rel: PathBuf,
    pub result: std::result::Result<usize, String>,
}

fn rayon_pool() -> &'static rayon::ThreadPool {
    static POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("rsharp-rayon-{i}"))
            .stack_size(WORKER_STACK_SIZE)
            .build()
            .expect("failed to build rayon worker pool")
    });
    &POOL
}

fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("rsharp-tokio")
            .thread_stack_size(WORKER_STACK_SIZE)
            .build()
            .expect("failed to build tokio runtime")
    });
    &RT
}

fn default_max_in_flight() -> usize {
    (rayon_pool().current_num_threads() * 2).max(4)
}

/// Converts a batch of Rust source files to R# in parallel using Tokio for async I/O
/// and Rayon for CPU-bound parsing, conversion, and optional round-trip verification.
///
/// Returned outcomes strictly preserve the input order of `files`.
pub fn convert_project_files(
    files: &[ProjectFile],
    oracle: Option<&mut RustAnalyzerOracle>,
    options: &ProjectConvertOptions,
) -> Vec<ProjectFileOutcome> {
    tokio_runtime().block_on(convert_project_files_async(files, oracle, options))
}

/// Async variant of [`convert_project_files`] for callers already executing inside a Tokio runtime.
pub async fn convert_project_files_async(
    files: &[ProjectFile],
    oracle: Option<&mut RustAnalyzerOracle>,
    options: &ProjectConvertOptions,
) -> Vec<ProjectFileOutcome> {
    convert_project_files_async_bounded(files, oracle, options, default_max_in_flight()).await
}

async fn convert_project_files_async_bounded(
    files: &[ProjectFile],
    oracle: Option<&mut RustAnalyzerOracle>,
    options: &ProjectConvertOptions,
    max_in_flight: usize,
) -> Vec<ProjectFileOutcome> {
    match oracle {
        Some(oracle) => convert_with_oracle_pipelined(files, oracle, options, max_in_flight).await,
        None => convert_without_oracle(files, options, max_in_flight).await,
    }
}

async fn convert_without_oracle(
    files: &[ProjectFile],
    options: &ProjectConvertOptions,
    max_in_flight: usize,
) -> Vec<ProjectFileOutcome> {
    let mut join_set = JoinSet::new();
    let mut results: Vec<Option<std::result::Result<usize, String>>> = vec![None; files.len()];

    for (idx, file) in files.iter().enumerate() {
        if join_set.len() >= max_in_flight {
            drain_one(&mut join_set, &mut results).await;
        }
        let path = file.path.clone();
        let rel = file.rel.clone();
        let out_dir = options.out_dir.clone();
        let verify = options.verify;
        let strict = options.strict_unnameable;
        join_set.spawn(async move {
            let res = match tokio::fs::read_to_string(&path).await {
                Ok(src) => {
                    convert_and_write_async(
                        src,
                        TypeHintMap::default(),
                        rel,
                        out_dir,
                        verify,
                        strict,
                    )
                    .await
                }
                Err(e) => Err(e.to_string()),
            };
            (idx, res)
        });
    }

    drain_all(&mut join_set, &mut results).await;
    collect_outcomes(files, results)
}

async fn convert_with_oracle_pipelined(
    files: &[ProjectFile],
    oracle: &mut RustAnalyzerOracle,
    options: &ProjectConvertOptions,
    max_in_flight: usize,
) -> Vec<ProjectFileOutcome> {
    // Prefetch .rs sources asynchronously so rust-analyzer never stalls on disk reads.
    let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    let (tx, mut rx) = mpsc::channel::<(usize, std::io::Result<String>)>(max_in_flight);
    let prefetch_handle = tokio::spawn(async move {
        for (idx, path) in paths.into_iter().enumerate() {
            let read_res = tokio::fs::read_to_string(&path).await;
            let _ = tx.send((idx, read_res)).await;
        }
    });

    // Query rust-analyzer one file at a time (avoiding VFS -32801 invalidation) while
    // dispatching CPU conversion, --verify, and async file writes onto Rayon + Tokio in parallel.
    let mut join_set = JoinSet::new();
    let mut results: Vec<Option<std::result::Result<usize, String>>> = vec![None; files.len()];

    while let Some((idx, read_res)) = rx.recv().await {
        let file = &files[idx];
        let src = match read_res {
            Ok(s) => s,
            Err(e) => {
                results[idx] = Some(Err(e.to_string()));
                continue;
            }
        };
        let hints = match oracle.type_hints_for_file(&file.path, &src) {
            Ok(h) => h,
            Err(e) => {
                results[idx] = Some(Err(e.to_string()));
                continue;
            }
        };

        if join_set.len() >= max_in_flight {
            drain_one(&mut join_set, &mut results).await;
        }
        let rel = file.rel.clone();
        let out_dir = options.out_dir.clone();
        let verify = options.verify;
        let strict = options.strict_unnameable;
        join_set.spawn(async move {
            let res = convert_and_write_async(src, hints, rel, out_dir, verify, strict).await;
            (idx, res)
        });
    }

    prefetch_handle.await.expect("prefetch task panicked");
    drain_all(&mut join_set, &mut results).await;
    collect_outcomes(files, results)
}

async fn drain_one(
    join_set: &mut JoinSet<(usize, std::result::Result<usize, String>)>,
    results: &mut [Option<std::result::Result<usize, String>>],
) {
    let (done_idx, res) = join_set
        .join_next()
        .await
        .unwrap()
        .expect("conversion task panicked");
    results[done_idx] = Some(res);
}

async fn drain_all(
    join_set: &mut JoinSet<(usize, std::result::Result<usize, String>)>,
    results: &mut [Option<std::result::Result<usize, String>>],
) {
    while let Some(joined) = join_set.join_next().await {
        let (done_idx, res) = joined.expect("conversion task panicked");
        results[done_idx] = Some(res);
    }
}

fn collect_outcomes(
    files: &[ProjectFile],
    results: Vec<Option<std::result::Result<usize, String>>>,
) -> Vec<ProjectFileOutcome> {
    files
        .iter()
        .zip(results)
        .map(|(file, res)| ProjectFileOutcome {
            rel: file.rel.clone(),
            result: res.unwrap(),
        })
        .collect()
}

async fn convert_and_write_async(
    src: String,
    hints: TypeHintMap,
    rel: PathBuf,
    out_dir: Option<PathBuf>,
    verify: bool,
    strict_unnameable: bool,
) -> std::result::Result<usize, String> {
    let (rsharp, raw_count) =
        convert_source_on_rayon(src, hints, verify, strict_unnameable).await?;
    write_converted_file(&rel, out_dir.as_deref(), &rsharp).await?;
    Ok(raw_count)
}

async fn convert_source_on_rayon(
    src: String,
    hints: TypeHintMap,
    verify: bool,
    strict_unnameable: bool,
) -> std::result::Result<(String, usize), String> {
    let (tx, rx) = oneshot::channel();
    rayon_pool().spawn(move || {
        let res = convert_source_sync(&src, &hints, verify, strict_unnameable);
        let _ = tx.send(res);
    });
    rx.await.expect("rayon conversion task panicked")
}

fn convert_source_sync(
    src: &str,
    hints: &TypeHintMap,
    verify: bool,
    strict_unnameable: bool,
) -> std::result::Result<(String, usize), String> {
    let rsharp = convert_from_rust_with_hints(
        src,
        FromRustOptions {
            use_rustc_inference: false,
        },
        hints,
        strict_unnameable,
    )
    .map_err(|e| e.to_string())?;
    if verify {
        transpile_to_rust(
            &rsharp,
            TranspileOptions {
                prelude_mode: PreludeMode::None,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    let raw_count = count_raw_rust_blocks(&rsharp);
    Ok((rsharp, raw_count))
}

async fn write_converted_file(
    rel: &Path,
    out_dir: Option<&Path>,
    rsharp: &str,
) -> std::result::Result<(), String> {
    if let Some(base_out) = out_dir {
        let mut dest = base_out.join(rel);
        dest.set_extension("rsharp");
        tokio::fs::create_dir_all(dest.parent().unwrap_or(base_out))
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(&dest, rsharp)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mock_oracle(inlay_responses: usize) -> RustAnalyzerOracle {
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut buf = Vec::new();
        buf.extend(frame(r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#).as_bytes());
        buf.extend(
            frame(r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"quiescent":true}}"#)
                .as_bytes(),
        );
        for k in 0..inlay_responses {
            let id = 2 + k as i64;
            let body = format!(r#"{{"jsonrpc":"2.0","id":{},"result":[]}}"#, id);
            buf.extend(frame(&body).as_bytes());
        }
        let ws = std::env::temp_dir();
        RustAnalyzerOracle::from_io(Cursor::new(buf), Vec::new(), &ws).unwrap()
    }

    #[test]
    fn parallel_conversion_covers_all_paths_and_preserves_order() {
        let base =
            std::env::temp_dir().join(format!("rsharp_core_parallel_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let default_opts = ProjectConvertOptions::default();
        assert!(!format!("{:?}", default_opts.clone()).is_empty());

        // 0. Empty batch in both modes
        let mut empty_oracle = mock_oracle(0);
        assert!(convert_project_files(&[], None, &default_opts).is_empty());
        assert!(convert_project_files(&[], Some(&mut empty_oracle), &default_opts).is_empty());

        let good_file = base.join("good.rs");
        std::fs::write(&good_file, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
        let bad_verify = base.join("bad_verify.rs");
        std::fs::write(
            &bad_verify,
            "fn f(x: i32) { match x { const { let a = 1; a } => {} _ => {} } }\n",
        )
        .unwrap();
        let bad_syntax = base.join("bad_syntax.rs");
        std::fs::write(&bad_syntax, "fn broken( {").unwrap();

        let pfile = |path: PathBuf, rel: &str| ProjectFile {
            path,
            rel: PathBuf::from(rel),
        };
        let sample = pfile(good_file.clone(), "good.rs");
        assert!(!format!("{:?}", sample.clone()).is_empty());

        // 1. Oracle mode: read failure + hint EOF failure
        let mut eof_oracle = mock_oracle(0);
        let outcomes = convert_project_files(
            &[
                pfile(base.join("missing.rs"), "missing.rs"),
                pfile(good_file.clone(), "good.rs"),
            ],
            Some(&mut eof_oracle),
            &default_opts,
        );
        assert_eq!(outcomes.len(), 2);
        assert!(!format!("{:?}", outcomes[0].clone()).is_empty());
        assert!(outcomes[0].result.is_err());
        assert!(outcomes[1].result.is_err());

        // 2. Oracle mode: verify failure + success with out_dir = None
        let mut verify_oracle = mock_oracle(2);
        let verify_res = convert_project_files(
            &[pfile(bad_verify.clone(), "bad_verify.rs")],
            Some(&mut verify_oracle),
            &ProjectConvertOptions {
                out_dir: None,
                verify: true,
                strict_unnameable: false,
            },
        );
        assert!(verify_res[0].result.is_err());

        let no_out_res = convert_project_files(
            &[pfile(good_file.clone(), "good.rs")],
            Some(&mut verify_oracle),
            &default_opts,
        );
        assert_eq!(no_out_res[0].result, Ok(0));

        // 3. Oracle mode: mkdir failure (out_dir is a file) and write failure (dest is a dir)
        let blocking = base.join("blocking");
        std::fs::write(&blocking, "file").unwrap();
        let mut err_write_oracle = mock_oracle(2);
        let mkdir_res = convert_project_files(
            &[pfile(good_file.clone(), "sub/good.rs")],
            Some(&mut err_write_oracle),
            &ProjectConvertOptions {
                out_dir: Some(blocking),
                verify: false,
                strict_unnameable: false,
            },
        );
        assert!(mkdir_res[0].result.is_err());

        let dir_dest = base.join("out_dir_conflict");
        std::fs::create_dir_all(dir_dest.join("good.rsharp")).unwrap();
        let write_res = convert_project_files(
            &[pfile(good_file.clone(), "good.rs")],
            Some(&mut err_write_oracle),
            &ProjectConvertOptions {
                out_dir: Some(dir_dest),
                verify: false,
                strict_unnameable: false,
            },
        );
        assert!(write_res[0].result.is_err());

        // 4. Oracle mode bounded backpressure (`max_in_flight = 2` with 5 files)
        let out_ra = base.join("out_ra");
        let ra_batch: Vec<ProjectFile> = (0..5)
            .map(|i| pfile(good_file.clone(), &format!("f_{i}.rs")))
            .collect();
        let mut batch_oracle = mock_oracle(5);
        let ra_outcomes = tokio_runtime().block_on(convert_project_files_async_bounded(
            &ra_batch,
            Some(&mut batch_oracle),
            &ProjectConvertOptions {
                out_dir: Some(out_ra.clone()),
                verify: true,
                strict_unnameable: false,
            },
            2,
        ));
        assert_eq!(ra_outcomes.len(), 5);
        for (i, o) in ra_outcomes.iter().enumerate() {
            assert_eq!(o.rel, PathBuf::from(format!("f_{i}.rs")));
            assert_eq!(o.result, Ok(0));
            assert!(out_ra.join(format!("f_{i}.rsharp")).is_file());
        }

        // 5. No-RA mode bounded backpressure (`max_in_flight = 2`) with mixed ok, read error, and syntax error
        let out_nora = base.join("out_nora");
        let nora_batch = vec![
            pfile(good_file.clone(), "00_ok.rs"),
            pfile(base.join("missing.rs"), "01_missing.rs"),
            pfile(bad_syntax.clone(), "02_bad.rs"),
            pfile(good_file.clone(), "03_ok.rs"),
            pfile(good_file.clone(), "04_ok.rs"),
        ];
        let nora_outcomes = tokio_runtime().block_on(convert_project_files_async_bounded(
            &nora_batch,
            None,
            &ProjectConvertOptions {
                out_dir: Some(out_nora.clone()),
                verify: true,
                strict_unnameable: false,
            },
            2,
        ));
        assert_eq!(nora_outcomes.len(), 5);
        assert_eq!(nora_outcomes[0].rel, PathBuf::from("00_ok.rs"));
        assert_eq!(nora_outcomes[0].result, Ok(0));
        assert_eq!(nora_outcomes[1].rel, PathBuf::from("01_missing.rs"));
        assert!(nora_outcomes[1].result.is_err());
        assert_eq!(nora_outcomes[2].rel, PathBuf::from("02_bad.rs"));
        assert!(nora_outcomes[2].result.is_err());
        assert_eq!(nora_outcomes[3].rel, PathBuf::from("03_ok.rs"));
        assert_eq!(nora_outcomes[3].result, Ok(0));
        assert_eq!(nora_outcomes[4].rel, PathBuf::from("04_ok.rs"));
        assert_eq!(nora_outcomes[4].result, Ok(0));
        assert!(out_nora.join("00_ok.rsharp").is_file());
        assert!(out_nora.join("03_ok.rsharp").is_file());
        assert!(out_nora.join("04_ok.rsharp").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }
}
