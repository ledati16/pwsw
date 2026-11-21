# Maintenance checklist

PWSW is in maintenance mode: visits are for dependency and security updates and bug
fixes. Work on `dev`, merge to `main` when the checks pass.

## 1. Look at what is waiting

- Open Dependabot PRs (weekly cargo group, separate majors, monthly Actions group) and
  the last runs of **Scheduled checks** (weekly `cargo audit` + build/test) in the
  Actions tab. If the schedule shows as disabled (GitHub turns it off after 60 days of
  inactivity), re-enable it there.

## 2. Update dependencies

```bash
cargo update                 # semver-compatible updates to Cargo.lock
cargo tree --duplicates      # new duplicate versions worth chasing?
```

Majors (and 0.x minors, which may break): compare `Cargo.toml` with
`cargo search <crate>` or crates.io, read the crate's CHANGELOG, update one crate at a
time and fix the fallout before the next. Merge or close the Dependabot PRs to match.

## 3. Bump the pinned toolchain (when a newer stable is wanted)

All of these together, in one commit:

- `rust-toolchain.toml` `channel`
- `.github/workflows/ci.yml` and `.github/workflows/scheduled.yml`:
  `dtolnay/rust-toolchain@<version>` and the step names
- `Cargo.toml` `rust-version`
- README Rust badge
- Then fix any new clippy lints (`cargo clippy --all-targets -- -D warnings`).

Pinned nightly for rustfmt (`rustfmt.toml` uses nightly-only options): replace every
hit of `git grep -n 'nightly-20'` (`ci.yml`, `rustfmt.toml`,
`scripts/install_git_hook.sh`, `CLAUDE.md`, this file), run
`cargo +nightly-YYYY-MM-DD fmt`, and commit any reformatting separately. Re-run
`bash scripts/install_git_hook.sh` so the installed hooks pick up the new nightly.

## 4. Verify

The verification cycle (same steps as CI):

```bash
cargo +nightly-2026-11-14 fmt --check
cargo clippy --all-targets --locked -- -D warnings
bash scripts/verify_tests_safe.sh --locked
cargo check --no-default-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo audit
```

`cargo audit` must report no vulnerabilities. For an unmaintained-crate warning, look
for a replacement but don't block a release on it.

## 5. Smoke-test the real thing

```bash
mkdir -p ~/build/pwsw-pkg && cp contrib/PKGBUILD ~/build/pwsw-pkg/
cd ~/build/pwsw-pkg && makepkg -si --cleanbuild   # clones, builds, runs the tests
systemctl --user restart pwsw
pwsw doctor                  # every check should pass
pwsw status
```

Open and close a window that a rule matches and check that the sink follows.
The PKGBUILD clones the GitHub repo's default branch, so it tests what is already
merged and pushed; before that, test a local tree with `cargo install --locked --path .`.

## 6. Release

1. Add a section to `CHANGELOG.md` (Keep a Changelog; "Security" for advisories fixed).
   If a change touches `Request`/`Response` incompatibly, bump `PROTOCOL_VERSION` in
   `src/ipc.rs` and tell users to restart the daemon; the `status --json` and
   `status --watch --json` schemas in `man/pwsw.1.md` only ever gain fields.
2. Bump `version` in `Cargo.toml` (and in `contrib/noctalia/plugin.toml`), run
   `cargo build` so `Cargo.lock` follows.
3. Commit, merge to `main`, then tag and push:
   `git tag -a vX.Y.Z -m "pwsw X.Y.Z" && git push origin main vX.Y.Z`
4. Optionally create a GitHub release from the tag with the CHANGELOG section.
