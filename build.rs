//! Build script: man pages and the version string.
//!
//! `PWSW_VERSION` is the crate version plus, when built from a git checkout of this
//! crate, the short commit hash and a `-dirty` marker (`0.6.0+abc1234-dirty`). A
//! crates.io package falls back to the commit recorded in `.cargo_vcs_info.json`;
//! with neither (a plain source tarball, or `git` missing) it is just the version.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    generate_man_pages();

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let full = match git_revision(&manifest_dir).or_else(|| vcs_info_revision(&manifest_dir)) {
        Some(revision) => format!("{version}+{revision}"),
        None => version,
    };
    println!("cargo:rustc-env=PWSW_VERSION={full}");
}

fn generate_man_pages() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let man_out_dir = Path::new(&out_dir).join("man");
    fs::create_dir_all(&man_out_dir).unwrap();

    let man_sources = [("man/pwsw.1.md", "pwsw", 1), ("man/pwsw.5.md", "pwsw", 5)];

    for (src_path, title, section) in man_sources {
        println!("cargo:rerun-if-changed={src_path}");

        let markdown =
            fs::read_to_string(src_path).expect("Failed to read man page markdown source");

        // mandown 1.1.0: convert(markup, title, section) -> String
        let roff = mandown::convert(&markdown, title, section);

        let dst_filename = format!("{title}.{section}");
        fs::write(man_out_dir.join(dst_filename), roff)
            .expect("Failed to write generated man page");
    }
}

/// Run `git` in `dir`; `None` if it is missing or fails
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Never take the index lock: a build must not race the user's own git commands
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

/// Emit `rerun-if-changed` for `path` if it exists (Cargo re-runs the script on
/// every build for a missing one)
fn watch_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Short commit hash (plus `-dirty`) of the git checkout this crate is the root of
fn git_revision(manifest_dir: &Path) -> Option<String> {
    // Only a repository rooted at this crate counts: a tarball unpacked inside some
    // other checkout (e.g. an AUR package repository) must not report that commit.
    let toplevel = git(manifest_dir, &["rev-parse", "--show-toplevel"])?;
    if fs::canonicalize(toplevel).ok()? != fs::canonicalize(manifest_dir).ok()? {
        return None;
    }

    // Re-run when HEAD moves (checkout, commit, reset) or the index changes. Paths
    // come from `--git-path` so linked worktrees resolve correctly.
    let mut watched = vec![
        "HEAD".to_owned(),
        "packed-refs".to_owned(),
        "index".to_owned(),
    ];
    if let Some(head_ref) = git(manifest_dir, &["symbolic-ref", "-q", "HEAD"]) {
        watched.push(head_ref);
    }
    for name in &watched {
        if let Some(path) = git(manifest_dir, &["rev-parse", "--git-path", name]) {
            watch_if_exists(&manifest_dir.join(path));
        }
    }
    // Edits to the sources change the dirty marker
    for path in ["src", "build.rs", "Cargo.toml", "Cargo.lock"] {
        watch_if_exists(&manifest_dir.join(path));
    }

    let hash = git(manifest_dir, &["rev-parse", "--short", "HEAD"])?;
    let dirty = git(manifest_dir, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}

/// Commit recorded by `cargo package` (crates.io builds have no `.git`)
fn vcs_info_revision(manifest_dir: &Path) -> Option<String> {
    let path = manifest_dir.join(".cargo_vcs_info.json");
    watch_if_exists(&path);
    let text = fs::read_to_string(path).ok()?;
    let after_key = &text[text.find("\"sha1\"")? + "\"sha1\"".len()..];
    let value = &after_key[after_key.find('"')? + 1..];
    let sha = &value[..value.find('"')?];
    (sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha[..7].to_owned())
}
