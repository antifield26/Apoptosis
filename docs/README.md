# Documentation map

This directory is layered so a reader can tell **what is true now** from
**what was true at some point in the past**:

| Layer | Contents |
|---|---|
| **Current state** | [architecture/system-overview.md](architecture/system-overview.md) · [vanilla-parity/PARITY-MATRIX.md](vanilla-parity/PARITY-MATRIX.md) — the single vanilla-divergence authority · [testing/TEST-MATRIX.md](testing/TEST-MATRIX.md) — current coverage + deduplicated defect history · [release/RELEASE-CANDIDATE.md](release/RELEASE-CANDIDATE.md) · [performance/BENCHMARK-BASELINE.md](performance/BENCHMARK-BASELINE.md) (incl. the Pi 5 acceptance record) · [operations/RUNBOOK.md](operations/RUNBOOK.md) · [CONVENTIONS.md](CONVENTIONS.md) — the engineering contract |
| **Reference baselines** | [protocol/](protocol/26.1.2-wire-notes.md) (wire facts extracted from the 26.1.2 jar) · [research/](research/protocol-baseline.md) (measured baselines, reference-clone inventory, provenance log) · [legal/third-party.md](legal/third-party.md) (dependency + reference licences) |
| **Decision records** | [adr/](adr/ADR-0001-system-architecture.md) — ADR-0001…0006, each accepted with a date; decisions are annotated in place, never silently rewritten |
| **Governance audits** | [audits/](audits/AUDIT-08-FINDINGS.md) — the adversarial audits run **after** governance, kept in the working tree because they are the evidence for the current state. Five files: [AUDIT-07-FINDINGS.md](audits/AUDIT-07-FINDINGS.md) (first full cross-audit; 9 findings plus 2 more found while fixing them) · [AUDIT-07-LANE-E.md](audits/AUDIT-07-LANE-E.md) (the Raspberry Pi deployment audit; 5 findings, E1–E5) · [AUDIT-07-REMEDIATION.md](audits/AUDIT-07-REMEDIATION.md) (dispositions for all 16) · [AUDIT-08-FINDINGS.md](audits/AUDIT-08-FINDINGS.md) (second full cross-audit; 12 findings, H1–H2 / M1–M4 / L1–L5 / I1) · [AUDIT-08-REMEDIATION.md](audits/AUDIT-08-REMEDIATION.md) (dispositions for all 12). **All 16 and all 12 are fixed**, except AUDIT-08’s I1 (informational) and L5/E5, which were closed on the device rather than in the tree |
| **History** | [../CHANGELOG.md](../CHANGELOG.md) — the distilled phase-by-phase history. The per-phase reports and the **five pre-governance audit-finding files** (`AUDIT-01`–`AUDIT-06`, including the `AUDIT-04` number that never existed as a file) were retired to **git history** (owner decision, 2026-09-12): read any of them with `git show phase-09-final:docs/phases/PHASE-05-REPORT.md`, or check out the tag. The complete pre-governance tree is the tag `phase-09-final`. The audits run *after* governance are **not** retired — they are in [audits/](audits/AUDIT-08-FINDINGS.md) |

Rules the layers follow: current-state documents describe the present and link
to evidence; reference baselines record measured facts and do not track
implementation status; ADRs are immutable once accepted (a superseding ADR
points back); historical material lives in git history, not in the working
tree.

## Retired paths (2026-09-12) — old → new

The per-phase reports and audits of the retired phases directory (15 files, including the
ten phase reports and the five audit-finding files) were
distilled into [../CHANGELOG.md](../CHANGELOG.md), the defect history in
[testing/TEST-MATRIX.md](testing/TEST-MATRIX.md) and
[vanilla-parity/PARITY-MATRIX.md](vanilla-parity/PARITY-MATRIX.md), and then
deleted. Every path below still resolves **in git**:

| Retired path | Where its content lives now |
|---|---|
| The ten phase reports (`PHASE-00-REPORT.md` … `PHASE-09-REPORT.md`) | [../CHANGELOG.md](../CHANGELOG.md) per-phase sections; full text: `git show phase-09-final:docs/phases/PHASE-05-REPORT.md` |
| The five audit findings (`AUDIT-01-FINDINGS.md` … `AUDIT-06-FINDINGS.md`) | the defect history in [testing/TEST-MATRIX.md](testing/TEST-MATRIX.md); full text at the tag |
| `AUDIT-04-FINDINGS.md` | **never existed as a file** — the fourth audit pass was folded into `AUDIT-05-FINDINGS.md` (its header says so); if you are looking for a missing number, this is why |
| The retired divergence catalog (`KNOWN-DIVERGENCES.md`) | merged into [vanilla-parity/PARITY-MATRIX.md](vanilla-parity/PARITY-MATRIX.md) (the `KD-nn` tags live in its rows) |

The governance decisions and the full old→new accounting:
[GOVERNANCE-REPORT.md](GOVERNANCE-REPORT.md).
