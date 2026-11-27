use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::codegen_rust::PreludeMode;
use crate::error::{RSharpError, Result};
use crate::parallel::{ProjectFile, ToRustProjectOptions, transpile_project_files};
use crate::ra_oracle::CargoFeaturesMode;

static SHADOW_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Options controlling Stage C shadow `cargo check` verification.
#[derive(Debug, Clone)]
pub struct CargoVerifyOptions {
    pub cargo_bin: String,
    pub features: CargoFeaturesMode,
    pub target_dir: Option<PathBuf>,
}

impl Default for CargoVerifyOptions {
    fn default() -> Self {
        Self {
            cargo_bin: "cargo".to_string(),
            features: CargoFeaturesMode::Default,
            target_dir: None,
        }
    }
}

/// Output summary returned when Stage C shadow `cargo check` succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoVerifyReport {
    pub checked_files: usize,
    pub stdout: String,
    pub stderr: String,
}

struct TempShadowDir(PathBuf);

impl TempShadowDir {
    fn create_in(parent: &Path, prefix: &str) -> Result<Self> {
        let id = SHADOW_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("{prefix}_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| {
            RSharpError::io(format!(
                "Failed to create shadow workspace '{}': {e}",
                dir.display()
            ))
        })?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempShadowDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Verifies Stage C semantic equivalence (`Rust -> R# -> Rust -> cargo check`) by copying
/// `ws_root` into an isolated temporary shadow workspace (never mutating the original repo),
/// transpiling `rsharp_files` back into the shadow workspace with `PreludeMode::None`, and
/// running `cargo check` with matching feature flags and an isolated `CARGO_TARGET_DIR`.
pub fn verify_project_with_cargo(
    project_dir: &Path,
    ws_root: &Path,
    rsharp_files: &[ProjectFile],
    options: &CargoVerifyOptions,
) -> Result<CargoVerifyReport> {
    verify_project_with_cargo_in(
        &std::env::temp_dir(),
        project_dir,
        ws_root,
        rsharp_files,
        options,
    )
}

fn verify_project_with_cargo_in(
    shadow_parent: &Path,
    project_dir: &Path,
    ws_root: &Path,
    rsharp_files: &[ProjectFile],
    options: &CargoVerifyOptions,
) -> Result<CargoVerifyReport> {
    let shadow = TempShadowDir::create_in(shadow_parent, "rsharp_verify_cargo")?;
    let shadow_ws = shadow.path().join("ws");
    copy_dir_filtered(ws_root, &shadow_ws)?;

    let ws_canon = std::fs::canonicalize(ws_root).unwrap();
    let proj_canon = std::fs::canonicalize(project_dir).ok();
    let proj_rel = proj_canon
        .as_deref()
        .and_then(|p| p.strip_prefix(&ws_canon).ok())
        .unwrap_or(Path::new(""));
    let shadow_proj = shadow_ws.join(proj_rel);

    let outcomes = transpile_project_files(
        rsharp_files,
        &ToRustProjectOptions {
            out_dir: Some(shadow_proj.clone()),
            prelude_mode: PreludeMode::None,
        },
    );
    let mut transpile_errs = Vec::new();
    for outcome in outcomes {
        if let Err(e) = outcome.result {
            transpile_errs.push(format!("{}: {}", outcome.rel.display(), e));
        }
    }
    if !transpile_errs.is_empty() {
        return Err(RSharpError::codegen(format!(
            "Failed transpiling {} file(s) into shadow workspace:\n{}",
            transpile_errs.len(),
            transpile_errs.join("\n")
        )));
    }

    let default_target = shadow.path().join("target");
    let target_dir = options.target_dir.as_deref().unwrap_or(&default_target);
    let mut cmd = Command::new(&options.cargo_bin);
    cmd.current_dir(&shadow_proj).arg("check");
    if proj_rel.as_os_str().is_empty() {
        cmd.arg("--workspace");
    }
    cmd.arg("--all-targets");
    for arg in options.features.cargo_args() {
        cmd.arg(arg);
    }
    cmd.env("CARGO_TARGET_DIR", target_dir);

    let output = cmd.output().map_err(|e| {
        RSharpError::io(format!(
            "Failed to launch '{}' for Stage C verification: {e}",
            options.cargo_bin
        ))
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(RSharpError::compile(format!(
            "Shadow cargo check failed in '{}':\n{}",
            project_dir.display(),
            stderr
        )));
    }

    Ok(CargoVerifyReport {
        checked_files: rsharp_files.len(),
        stdout,
        stderr,
    })
}

fn copy_dir_filtered(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| {
        RSharpError::io(format!(
            "Failed creating shadow directory '{}': {e}",
            dst.display()
        ))
    })?;
    let entries = std::fs::read_dir(src).map_err(|e| {
        RSharpError::io(format!(
            "Failed reading source directory '{}': {e}",
            src.display()
        ))
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            if matches!(name_str.as_ref(), ".git" | "target" | "node_modules") {
                continue;
            }
            copy_dir_filtered(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| {
                RSharpError::io(format!(
                    "Failed copying '{}' to shadow workspace: {e}",
                    from.display()
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cargo_shadow_covers_workspace_subcrate_features_and_errors() {
        let base =
            std::env::temp_dir().join(format!("rsharp_verify_cargo_unit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let ws = base.join("ws");
        let sub = ws.join("crates").join("sub");
        std::fs::create_dir_all(sub.join("src")).unwrap();
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::create_dir_all(ws.join("target")).unwrap();
        std::fs::create_dir_all(ws.join("node_modules")).unwrap();
        std::fs::write(ws.join(".git").join("HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/sub\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::write(
            sub.join("Cargo.toml"),
            "[package]\nname = \"sub\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [features]\nbonus = []\n",
        )
        .unwrap();
        std::fs::write(
            sub.join("src").join("lib.rs"),
            "#[cfg(feature = \"bonus\")]\npub fn bonus_val() -> i32 { 7 }\n\
             pub fn base_val() -> i32 { 1 }\n",
        )
        .unwrap();

        let rsh_dir = base.join("rsh");
        std::fs::create_dir_all(rsh_dir.join("src")).unwrap();
        let ok_rsh = rsh_dir.join("src").join("lib.rsharp");
        std::fs::write(
            &ok_rsh,
            "[Cfg(feature = \"bonus\")]\npublic int bonus_val() { return 7; }\n\
             public int base_val() { return 1; }\n",
        )
        .unwrap();
        let rsh_files = vec![ProjectFile {
            path: ok_rsh.clone(),
            rel: PathBuf::from("src/lib.rsharp"),
        }];
        let shared_target = base.join("shared_target");

        // 1. Subcrate check with `--features bonus`
        let rep = verify_project_with_cargo(
            &sub,
            &ws,
            &rsh_files,
            &CargoVerifyOptions {
                features: CargoFeaturesMode::List(vec!["bonus".to_string()]),
                target_dir: Some(shared_target.clone()),
                ..Default::default()
            },
        )
        .expect("subcrate cargo check should pass");
        assert_eq!(rep.checked_files, 1);

        // 2. Whole-workspace check with `--all-features`
        let ws_rsh_files = vec![ProjectFile {
            path: ok_rsh.clone(),
            rel: PathBuf::from("crates/sub/src/lib.rsharp"),
        }];
        let rep_ws = verify_project_with_cargo(
            &ws,
            &ws,
            &ws_rsh_files,
            &CargoVerifyOptions {
                features: CargoFeaturesMode::All,
                target_dir: Some(shared_target.clone()),
                ..Default::default()
            },
        )
        .expect("workspace cargo check should pass");
        assert_eq!(rep_ws.checked_files, 1);

        // 3. Semantic compile failure in shadow cargo check -> Err(rustc)
        let bad_sem_rsh = rsh_dir.join("src").join("bad_sem.rsharp");
        std::fs::write(&bad_sem_rsh, "public int base_val() { return \"nope\"; }\n").unwrap();
        let err_sem = verify_project_with_cargo(
            &sub,
            &ws,
            &[ProjectFile {
                path: bad_sem_rsh,
                rel: PathBuf::from("src/lib.rsharp"),
            }],
            &CargoVerifyOptions {
                target_dir: Some(shared_target.clone()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err_sem.to_string().contains("Shadow cargo check failed"));

        // 4. Transpile failure -> Err(codegen)
        let bad_syn_rsh = rsh_dir.join("src").join("bad_syn.rsharp");
        std::fs::write(&bad_syn_rsh, "public int broken( {\n").unwrap();
        let err_syn = verify_project_with_cargo(
            &sub,
            &ws,
            &[ProjectFile {
                path: bad_syn_rsh,
                rel: PathBuf::from("src/lib.rsharp"),
            }],
            &CargoVerifyOptions::default(),
        )
        .unwrap_err();
        assert!(err_syn.to_string().contains("Failed transpiling"));

        // 5. Missing cargo binary -> Err(io)
        let err_bin = verify_project_with_cargo(
            &sub,
            &ws,
            &rsh_files,
            &CargoVerifyOptions {
                cargo_bin: "nonexistent_cargo_bin_xyz_98765".to_string(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err_bin.to_string().contains("Failed to launch"));

        // 6. Missing source dir, blocked shadow parent, blocked copy dst dir, blocked copy file
        assert!(
            verify_project_with_cargo(
                &base.join("missing"),
                &base.join("missing"),
                &rsh_files,
                &CargoVerifyOptions::default(),
            )
            .is_err()
        );
        let blocker = base.join("blocker_file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(
            verify_project_with_cargo_in(
                &blocker,
                &sub,
                &ws,
                &rsh_files,
                &CargoVerifyOptions::default(),
            )
            .is_err()
        );
        assert!(copy_dir_filtered(&sub, &blocker).is_err());

        // Copy file error when destination path is an existing directory, plus nested dir error
        let dst_conflict = base.join("dst_conflict");
        std::fs::create_dir_all(dst_conflict.join("Cargo.toml")).unwrap();
        assert!(copy_dir_filtered(&sub, &dst_conflict).is_err());
        let nested_conflict = base.join("nested_conflict");
        std::fs::create_dir_all(nested_conflict.join("src").join("lib.rs")).unwrap();
        assert!(copy_dir_filtered(&sub, &nested_conflict).is_err());

        // `project_dir` outside `ws_root` falls back to workspace root
        let rep_out = verify_project_with_cargo(
            &base,
            &ws,
            &ws_rsh_files,
            &CargoVerifyOptions {
                target_dir: Some(shared_target),
                ..Default::default()
            },
        )
        .expect("fallback to ws_root should pass");
        assert_eq!(rep_out.checked_files, 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}
