# AUDIT-08 Remediation — every finding dispositioned (2026-09-12)

Each AUDIT-08 finding, the fix, and how the fix was verified. The audit's
verdict was PASS WITH FINDINGS; after this remediation the same checks were
re-run (final section).

| ID | Fix | Verification |
|---|---|---|
| H1 (2 unpushed commits, CI gap) | `319cdb4` + `b279cdf` pushed together with the AUDIT-08 report and this remediation; CI runs on the push | `gh run list` shows the run covering HEAD; re-check after push |
| H2 (unrendered `{lib_sum}` placeholders in TEST-MATRIX) | Rendered with independently derived values: lib **958** + doc-tests **5** + named integration **233** = **1 196** (was 956/5/233 = 1 194 before the two new coverage tests); `check_gate_totals.py` now also fails on `{placeholder}` fragments in any non-audit document | checker exits 0 with canonical 1 196; a placeholder probe fails it (Lane F style) |
| M1 (encoding check missed 6 extension-less text files) | `check_encoding.py` now derives coverage from `git ls-files --eol` (skips only git-marked binaries) instead of a suffix list — 275 text files checked, was 249 | Probe re-run: `0xFF 0xFE` appended to NOTICE now **fails** the check; both fixtures still excluded |
| M2 (`$MC_FIXTURE_DIR` branch untested) | `candidate_dirs` takes the override as a parameter (no process-global state in tests); new test `the_operator_override_is_searched_first` pins the precedence | `cargo test -p mc-registry --lib` = 15 passed (was 14) |
| M3 (`sync_directory` unfalsifiable) | Coverage-impossible reason recorded on the function per DoD item 4 (a directory fsync is only observable across a power loss; Windows cannot fsync a directory handle) — verified by the audit's own break: 74/74 green with it disabled | documented in `save.rs`; the audit's break stands as the evidence |
| M4 (unit scored 9.2 UNSAFE) | `deploy/mc-server.service` hardened: `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `NoNewPrivileges`, kernel/control-group/clock/hostname protections, `RestrictAddressFamilies`, `RestrictNamespaces`, `RestrictRealtime`, `LockPersonality`, `MemoryDenyWriteExecute`, `SystemCallArchitectures` — with `ReadWritePaths` unchanged for the world and backups | Redeployed to the Pi; service starts and answers the status smoke under the hardened unit; `systemd-analyze security` re-scored (recorded below) |
| L1 (speed constant unpinned) | New test `the_speed_constant_is_the_documented_player_derivation` pins the constant and the derived table (attribute × constant); the constant's comment marks a change as a deliberate recalibration | `cargo test -p mc-entity --lib` = 128 passed (was 127) |
| L2 (hopper `push_target` ignores slot limits) | Precondition documented on the function: room assumes hopper slots (64-stack); limited slots would trip the invariant error in `transfer` | doc in `hopper.rs`; the audit's room over-report probe remains the behavioural evidence |
| L3 (comma-grouped totals mangled) | `check_gate_totals.py` accepts `1,194`-style spellings and strips the comma before comparing | probe re-run: comma spelling now resolves to the true value |
| L4 (retired-report reference count drift: 10 vs 13 vs 12) | Measured **12 occurrences in 11 files**; all of them re-pointed at `CHANGELOG.md` or the git-history tag form — the count is now **0** and the GOVERNANCE-REPORT pending item is closed | `grep -rn "PHASE-0[0-9]-REPORT\|AUDIT-0[0-9]-FINDINGS" crates apps --include="*.rs"` = 0 |
| L5 (Pi deployment stale, no commit identity) | Redeployed from a HEAD archive: source tree refreshed, release binary rebuilt **on the device**, hardened unit installed, and `/srv/mc-server/BUILD-INFO` records the built commit and date | `BUILD-INFO` on the device; device tree diff vs HEAD archive = clean |
| L4-adjacent (config doc corruption) | `config.rs` `DataPackConfig` doc comments had backslashes where backticks belonged — repaired | `check_encoding.py` cannot catch this class; found by reading, fixed by rewrite |
| I1/E4 (deployed server loads no data packs) | Closed: the extracted vanilla `data/minecraft` (25 MB) transferred to `/srv/mc-server/vanilla-data/minecraft` and `[datapacks] vanilla_data` set in the deployed config; the journal now records the pack census on startup | `journalctl -u mc-server` after restart |
| I1/E5 (`/var/backups/mc-server/` empty) | Closed: an offline whole-copy archive of the deployment world was written to `/var/backups/mc-server/` while the service was stopped. The `backup.rs` library CLI is still missing (KD-37) — this backup used file-level tooling, and the row stays open for the CLI | tar listing on the device |
| Governance pending 1 (10→12 code comments citing retired reports) | Closed with L4: all re-pointed at `CHANGELOG.md` or the tag form | grep = 0 |
| Governance pending 4 (no tagged binary release) | Closed: tag `v0.1.0-rc.1` + GitHub release with the x86_64 and aarch64 `mc-server` binaries and SHA-256 sums; README/RELEASE-CANDIDATE/CHANGELOG updated to release language | `gh release view v0.1.0-rc.1` |

Remaining open after this remediation: **KD-38 (real-client acceptance)** and
the parity-matrix roadmap gaps (lighting, entity persistence/sync, redstone
wiring, container windows, …) — these are development work, not remediation.

## Final verification (post-remediation tree)

- `cargo test --workspace --no-fail-fast`: **1 196 passed / 0 failed / 21
  ignored, 74 suites**, exit 0 (crate-aware accounting: 0 of 74 unattributed)
- fmt, clippy (`-D warnings`), aarch64 cross-check, cargo deny: all exit 0
- Four docs-audit scripts: all exit 0 (275 text files, 0 broken links, 0
  mismatched totals, 0 line-ending violations)
- Pi: hardened unit active, status smoke `protocol=775`, `ExecMainStatus=0`,
  `systemd-analyze security` re-scored, data packs loaded, backup written,
  `BUILD-INFO` records the built commit
