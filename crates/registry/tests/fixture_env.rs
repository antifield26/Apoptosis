//! E-02: the `$MC_FIXTURE_DIR` environment half, tested in a subprocess (P15-07).
//!
//! Audit 08's M2 pinned that the override value wins the *precedence* race;
//! what had no test is that the variable is read at all and that the path it
//! names is the one used. Environment is process-global state, so each case
//! spawns the `fixture_probe` example binary with a controlled environment
//! rather than touching `std::env` from inside the test harness.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The compiled probe binary next to this test binary (`deps/` -> `examples/`).
fn probe_binary() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary path");
    dir.pop(); // strip the test binary name (…/deps/)
    dir.pop(); // …/deps/ -> …/debug/
    dir.push("examples");
    dir.push(format!("fixture_probe{}", std::env::consts::EXE_SUFFIX));
    dir
}

/// This test's build of the probe must exist: `cargo test` builds examples,
/// and a filtered run builds it on demand.
fn run_probe(override_dir: &Path) -> std::process::Output {
    let bin = probe_binary();
    if !bin.is_file() {
        let cargo = option_env!("CARGO").unwrap_or("cargo");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let build = std::process::Command::new(cargo)
            .arg("build")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--example")
            .arg("fixture_probe")
            .output()
            .expect("cargo build runs");
        assert!(
            build.status.success() && bin.is_file(),
            "probe binary missing at {} (build it with: cargo build -p mc-registry --example fixture_probe)",
            bin.display()
        );
    }
    Command::new(&bin)
        .env("MC_FIXTURE_DIR", override_dir)
        .output()
        .expect("probe spawns")
}

/// Unique scratch directory for one case.
fn scratch(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mc-fixture-env-{}-{}", std::process::id(), case));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clears stale scratch");
    }
    std::fs::create_dir_all(&dir).expect("creates scratch");
    dir
}

/// The real fixture set, resolved the same way production documents it.
fn real_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test-support/fixtures/registry")
        .canonicalize()
        .expect("workspace fixtures exist")
}

#[test]
fn garbage_override_fails_instead_of_falling_back() {
    // A `blocks.tsv` that exists but does not parse: `vanilla()` returns the
    // load error without consulting fallbacks, so failure (where ignoring the
    // variable would succeed) proves the variable is read, the path is used,
    // and the override wins the precedence race.
    let dir = scratch("garbage");
    std::fs::write(dir.join("blocks.tsv"), "garbage,not,a,table\n").expect("writes");
    let out = run_probe(&dir);
    assert!(!out.status.success(), "garbage fixtures must not load");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("ERR"),
        "probe reports failure on stdout, got: {text}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_table_under_override_names_the_override_path() {
    // A copy with one table deleted: the read error carries the full path, so
    // the message itself proves which directory was used.
    let dir = scratch("missing");
    let real = real_fixture_dir();
    for name in ["blocks.tsv", "items.tsv", "block_light.tsv"] {
        std::fs::copy(real.join(name), dir.join(name)).expect("copies fixture");
    }
    let out = run_probe(&dir);
    assert!(!out.status.success(), "incomplete fixtures must not load");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&*dir.to_string_lossy()),
        "error names the override directory, got: {text}"
    );
    assert!(text.contains("entity_types.tsv"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn real_override_loads_the_same_registries() {
    // The override path loads successfully end to end, with the same block
    // count as a direct in-process load of the same directory.
    let real = real_fixture_dir();
    let expected = mc_registry::Registries::load(&real)
        .expect("direct load")
        .blocks
        .state_count();
    let out = run_probe(&real);
    assert!(out.status.success(), "real fixtures must load");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("OK blocks={expected} dir={}", real.display())
    );
    // The child must not need anything else from the environment: a second run
    // with a scrubbed environment (minus system essentials) agrees.
    let bin = probe_binary();
    let scrubbed = Command::new(&bin)
        .env_clear()
        .env("MC_FIXTURE_DIR", &real)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(
            "SYSTEMROOT",
            std::env::var_os("SYSTEMROOT").unwrap_or_default(),
        )
        .output()
        .expect("probe spawns scrubbed");
    assert!(scrubbed.status.success(), "no hidden env dependency");
    assert_eq!(
        String::from_utf8_lossy(&scrubbed.stdout).trim(),
        format!("OK blocks={expected} dir={}", real.display())
    );
}
