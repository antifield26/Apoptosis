# Phase 01 Report — Engineering Foundation

Date: 2026-09-10. Preflight: re-read AGENTS.md §2/§7/§14 + MASTER-PROMPT §5 +
PHASE-01 prompt; `git status` (branch `master`, no commits — unchanged from P00);
toolchain observed `rustc/cargo 1.98.1, stable-x86_64-pc-windows-msvc`.

## Tasks (P01-01..P01-12)

| Task | Status | Evidence |
|---|---|---|
| P01-01 workspace + package metadata | DONE | Root virtual manifest (`Cargo.toml`): resolver 2, members `crates/*, apps/*`, `[workspace.package]` 0.1.0/edition-2024/rust-1.98.1. Legacy single-crate `src/main.rs` removed (empty stub, no behaviour lost) |
| P01-02 pinned toolchain + reproducible build | DONE | `rust-toolchain.toml` pins **1.98.1** + rustfmt/clippy + win+linux targets; `Cargo.lock` present and pinned (tokio 1.53.1, serde 1.0.229, toml 1.1.5, tracing 0.1.44, ...) — **not committed at Phase 01**; the initial commit landed later, as `b7b1c99`; `.gitignore` keeps `/OpenSourceMinecraftServer` uncommittable |
| P01-03 fmt/clippy/lints + CI | DONE | `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; workspace lints (`missing_docs=warn`, `unsafe=forbid`, clippy all=deny/pedantic=warn); `.github/workflows/ci.yml` |
| P01-04 crate boundaries with real ownership | DONE | `mc-core` (error/ids/tick), `mc-server` (config/logging/lifecycle), `mc-test-support` (dev-only fixtures), `apps/server` binary. Each owns exercised behaviour + tests; future crates (protocol/nbt/...) arrive with P02/P03 code, not as stubs (AGENTS.md §3.4) |
| P01-05 error taxonomy | DONE | `crates/core/src/error.rs`: 5 AGENTS.md §9 variants + Shutdown; 4 config tests assert hostile input maps to `Operational`, never panic |
| P01-06 config TOML | DONE | `crates/server/src/config.rs`: schema + `deny_unknown_fields` + range validation + `config.example.toml`; defaults = contract (10 players, offline, bind 127.0.0.1:25565) |
| P01-07 tracing/logging | DONE | `crates/server/src/logging.rs`: single-install guard, `MC_LOG`/`RUST_LOG` filter, target spans |
| P01-08 lifecycle + graceful shutdown | DONE | `crates/server/src/lifecycle.rs`: `ShutdownHandle` (flag+Notify), `run()` loop, Ctrl-C + unix SIGTERM handlers, `TickHook` seam for P05; binary exits 0 on Shutdown / 1 on failure |
| P01-09 deterministic clock/tick | DONE | `crates/core/src/tick.rs`: `TICKS_PER_SECOND=20`, `TickClock` (clamp 5, saturating math, injectable `now`), `WallClock::manual` for tests; 7 tests incl. determinism + overrun-clamp |
| P01-10 test-support/fixture harness | DONE | `crates/test-support`: `fixture_path` layout contract + `read_fixture` + divergence-reporting `assert_bytes_eq` + `TempDir`; `test-support` in zero production deps |
| P01-11 x86_64/aarch64 verification | DONE (qualified) | x86_64: build+test+clippy+fmt green. aarch64: `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` green; full `cargo build` links `cc` (absent on this Win host) — linking covered by the `ubuntu-24.04-arm` CI job |
| P01-12 foundation review + hardening | DONE | Self-review (EXECUTION-LOOP §5): fixed 2 real test failures (`ResourceId` accepted `..` segments + leading/trailing `//` — traversal hardening, AGENTS.md §10), 5 clippy findings, fmt drift; `third-party.md` §5 records locked deps + licenses |

## Verification (exact commands, this host)

- `cargo test --workspace` → **21 passed, 0 failed** (core 10, server 7, test-support 4).
- `cargo fmt --all -- --check` → clean.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` → clean.
- Binary smoke: `mc-server.exe config.example.toml` alive after 3 s (tick loop running, no bind yet — P02 owns sockets), killed by test harness (exit -1 = SIGKILL by the test, not a crash; cooperative path is unit-tested).

## Exit-gate check (EXIT-GATES.md P01)

- [x] Reproducible workspace + pinned toolchain (`rust-toolchain.toml`, `Cargo.lock`).
- [x] CI for x86_64 and aarch64 (`.github/workflows/ci.yml`; aarch64 = native-arm runner, check+clippy+test).
- [x] Config, logging, error, test infra operational (all exercised by tests above).
- [x] Minimal executable with graceful shutdown (`apps/server`, `Server::run` + signal handlers + Shutdown test).

## Known limitations / carry-over

1. aarch64 **link** not proven on this Windows host (no cross linker) — CI arm runner owns it; tracked, not hidden.
2. `TickHook` is intentionally a seam, not gameplay; scheduler lands in P05.
3. No license file yet (R-09); `third-party.md` §5 keeps the license gate current.
4. `docs/` deltas this phase: `legal/third-party.md` §5 (locked deps), this report.

## Gate status

**Phase 01: PASS.** No BLOCKERs. Unblocks P02-01 (TCP lifecycle); P02-02/03 need `bytes` direct use (already in tree transitively, row added at P02 adoption).
