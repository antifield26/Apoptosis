# Contributing

Thanks for looking at this project. This document covers the build, the quality
gates, the test conventions, and the repository's documentation rules. The
one-sentence summary of the house style: **every claim carries evidence, and a
test that cannot fail proves nothing.**

## Environment

- Rust **1.98.1** — pinned in [rust-toolchain.toml](rust-toolchain.toml);
  rustup selects it automatically. A different toolchain is a reproducibility
  failure, not a configuration.
- Development happens on x86_64 (Windows or Linux); the production target is
  aarch64 (Raspberry Pi 5, Debian 13). CI runs the gates on both host platforms
  plus an aarch64 cross-check.
- The repository normalises everything to LF (see `.gitattributes`); an
  `.editorconfig` is provided for editors that honour it.

## Build

```sh
cargo build --workspace --release --locked
cargo run --release -p mc-server-app        # no argument = built-in defaults
```

`config.example.toml` documents every setting. Registry tables are read at
runtime from `fixtures/registry/` next to the binary; in-tree runs resolve them
automatically, deployed layouts must install them (see the systemd unit header
and [docs/operations/RUNBOOK.md](docs/operations/RUNBOOK.md)).

## Quality gates (all must pass before you push)

| Gate | Command | Current state (2026-09-12) |
|---|---|---|
| Tests | `cargo test --workspace --no-fail-fast` | 1 207 passed / 0 failed / 21 ignored, 78 suites |
| Formatting | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| aarch64 | `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | clean |
| Licences/deps | `cargo deny check licenses bans sources` | clean |

The 21 ignored tests are on-demand: the benchmark harness (`pi_profile`,
`tick_baseline`) and seven differential suites that need real 26.1.2 jar data
(next section). CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs
all of this on every push; a red CI is a stop for everything else.

### Differential tests (need a real vanilla 26.1.2 jar)

The ignored differential suites compare behaviour against the official server
jar and its extracted data. They skip cleanly (as ignored) without the data.

```sh
export MC_VANILLA_DATA="$PWD/target/vanilla-26.1.2/extract/data/minecraft"
export MC_VANILLA_JAR="$PWD/target/vanilla-26.1.2/server.jar"
export MC_VANILLA_WORLD="$PWD/target/vanilla-26.1.2/vanilla-world-26.1.2/world"
cargo test -p mc-data -p mc-container -p mc-persistence -p mc-server -p mc-worldgen -- --ignored
```

`MC_VANILLA_DATA` feeds the pack/differential data suites;
`MC_VANILLA_JAR` + `MC_VANILLA_WORLD` let `vanilla_differential` boot the real
26.1.2 server on a world this codebase wrote. Producing those assets is a
one-time local step (download the server jar, run it once to generate a world,
extract the data pack) — the repository never vendors jar-derived data.

## Commit style

- Imperative subject, then a body that says **why**, names the evidence
  (commands, counts, falsification probes) and the honest limitations. The
  history is the record; review a few commits to calibrate.
- Multi-line messages go via `git commit -F <file>` rather than shell quoting
  (Windows consoles mangle UTF-8 here — see the encoding rules below).
- Direct pushes to `main` are the current practice for the maintainer; keep
  each commit coherent (one concern) and never mix a behaviour change into a
  documentation-governance commit.

## Conventions that reviews enforce

[docs/CONVENTIONS.md](docs/CONVENTIONS.md) is the in-repo statement of the
engineering contract: evidence before claims, no fake completeness,
deterministic simulation, the error and security contracts, the test-level
vocabulary (L1 unit … L6 differential), and the rule that compatibility claims
cite a test, a measurement or a fixture. ADRs
([docs/adr/](docs/adr/)) record architectural decisions; new decisions get a
new ADR rather than an edit to an accepted one.

## Documentation health (reproducible, not manual)

Four audit scripts guard the docs; they are the mechanical definition of
"documentation healthy" and are run in CI:

```sh
python tools/docs-audit/check_encoding.py   # every tracked text file: strict UTF-8, zero mojibake
python tools/docs-audit/check_links.py      # every tracked markdown file: zero broken links, zero
                                            # dead backticked paths, zero references to untracked files
python tools/docs-audit/check_gate_totals.py  # the workspace test total is stated identically in every
                                            # document that restates it (owner: docs/testing/TEST-MATRIX.md)
python tools/docs-audit/check_line_endings.py # no tracked file violates its declared eol=lf attribute
                                            # (local guard: a Linux checkout already yields LF)
```

Rules the scripts help enforce:

- Every tracked text file is valid UTF-8. On Windows consoles, a `—` may
  *render* as mojibake while the file is fine — verify with the script before
  "fixing" anything.
- Edits touching non-ASCII content are best done with a small Python script
  (read bytes → replace → write bytes), never by inline shell quoting.
- Historical statements are annotated, not silently rewritten; deletions need
  the maintainer's explicit approval (git history is the archive — the
  pre-governance documentation snapshot is the tag `phase-09-final`).

## Releases

Two artifacts ship per release, and only one of them can be built in CI.

| Artifact | Built where | Why |
|---|---|---|
| `mc-server-x86_64-windows.exe` | CI, by [`.github/workflows/release.yml`](.github/workflows/release.yml) on the tag | an ordinary runner build, and the gates have already run on the same commit |
| `mc-server-aarch64` | **on the Raspberry Pi 5, by hand** | the production target is a Pi; a cross-build cannot be *run* from CI, so it cannot be verified there. `ci.yml`'s `check-aarch64` cross-*checks* only, and says so |

Because of that split, **a release exists before it is complete**. The workflow says so in the release
body it writes, so the page is honest at every moment rather than only once the manual step lands.

### Checklist

1. **Land the change and let CI go green on `main`.** A release is cut from a commit that passed the five
   gates, not from one that is about to.
2. **Bump the version** in `Cargo.toml` (`[workspace.package] version`) and add the CHANGELOG entry. The
   tag must start with `v` + that version; the workflow refuses to build otherwise. A pre-release suffix
   is allowed and is *not* modelled by cargo — `v0.1.0-rc.1` was released from workspace version `0.1.0`.
3. **Tag and push the tag.** CI runs again on the tag, and `release.yml` builds the x86_64 artifact,
   writes `SHA256SUMS`, creates the GitHub Release if the tag has none, and attaches both with
   `--clobber` so a re-run of the same tag is idempotent.
4. **Build the aarch64 artifact on the device and attach it.** On the Pi, from a clean checkout of the
   tag:

   ```sh
   cargo build --workspace --release --locked
   sha256sum target/release/mc-server
   ```

   Then attach it to the same release and **add its line to the checksums**:

   ```sh
   gh release upload vX.Y.Z ./mc-server-aarch64 --clobber
   ```

   The checksums file must end up covering **both** artifacts. If it still lists only the x86_64 build,
   the release is incomplete — say so rather than treating the release as done.
5. **Record the build on the device** in `/srv/mc-server/BUILD-INFO` (commit, date, profile) so the
   deployed server can be traced to a tag, and verify the deployed binary's SHA-256 matches the attached
   artifact. For `v0.1.0-rc.1` it does, byte for byte.
6. **Update the CHANGELOG's release block** with the final asset list and hashes, so the CHANGELOG and the
   Release page agree. `docs/release/RELEASE-CANDIDATE.md` carries the operator-facing summary.

Do **not** describe a release as complete, or as verified on aarch64, until step 4 has run on real
hardware. "The workflow is green" only ever means the x86_64 half.

## Reporting issues

Bugs: open a GitHub issue with the smallest reproduction and the gate output.
Security vulnerabilities: do **not** open a public issue — see
[SECURITY.md](SECURITY.md).
