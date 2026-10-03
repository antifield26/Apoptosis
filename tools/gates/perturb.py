"""Perturbation proofs: neutralise a mechanism, watch its named pin go red, restore byte-exact.

## Why this is a file

TEST-TIME-PLAN §2 makes a perturbation proof part of the definition of done for every migrated pin
("zero pins lost: every neutralisation still turns its named test red"), and the P20-02 slices have now
run that proof by hand three times. A proof that lives in a shell history is the shape `run.py`'s own
docstring warns about -- a check rewritten from memory every time, with a different hole each round.

## The trap this file closes (measured, 2026-10-03)

Restoring the perturbed file with a **copy that preserves the original's mtime** leaves cargo's
mtime-based freshness believing the *perturbed* test binary is current. The next full `cargo test`
then reuses it, and the run reports harness artifacts as test failures: with the farmland dry arm
neutralised, `dry_farmland_dries_then_dirts_on_direct_calls` failed with "dirting applies" (the
perturbed handler turned the soil straight to dirt), and with `RANDOM_TICK_SPEED` set to 2 the phase
wiring test failed its `sections × 3` assertion. Both looked like defects in the new tests; neither
was. Every restore here **rewrites the bytes** (a fresh write, hence a fresh mtime), and the tool
refuses to report a case whose restore does not hash back to the baseline.

## Usage

```text
python tools/gates/perturb.py --list          # the cases, by group
python tools/gates/perturb.py growth          # prove one group
python tools/gates/perturb.py all             # prove every group (slow: one rebuild per case)
```

Exit code 0 means every case went red as named and every file was restored byte-exact. A case that
stays green, fails to compile, or restores to different bytes is a failure of the proof, not a
warning -- the pin it names is not carrying the mechanism it claims to carry.
"""

import argparse
import hashlib
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

#: One entry per mechanism. `old` must occur exactly once in `file`; the case is refused otherwise
#: (a literal that matches twice would perturb a mechanism the pin does not name). `target` is the
#: nextest invocation that runs the pin(s) which must go red.
CASES: dict[str, list[dict[str, str]]] = {
    'growth': [
        {
            'name': 'crop light gate',
            'file': 'crates/server/src/game/growth.rs',
            'old': 'if self.raw_brightness(x, y, z) < GROWTH_LIGHT {',
            'new': 'if false {',
            'target': '-p mc-server --lib -E test(dark_wheat_never_grows_on_direct_calls)',
        },
        {
            'name': 'crop max age',
            'file': 'crates/server/src/game/growth.rs',
            'old': 'if age >= max_age_for(name) {',
            'new': 'if false {',
            'target': '-p mc-server --lib -E test(wheat_respects_max_age_on_direct_call)',
        },
        {
            'name': 'beetroot pre-gate',
            'file': 'crates/server/src/game/growth.rs',
            'old': 'if name == CROP_BEETROOTS && self.random.next_i32_bounded(3) != 0 {',
            'new': 'if false {',
            'target': '-p mc-server --lib -E test(beetroot_grows_slower_on_direct_calls)',
        },
        {
            'name': 'growth-speed moisture term',
            'file': 'crates/server/src/game/growth.rs',
            # The indentation is part of the literal: the same assignment shape is not repeated
            # elsewhere, and a two-line anchor keeps the match unique without a line number.
            'old': '                    if int_property(&props, "moisture").is_some_and(|m| m > 0) {\n'
                   '                        point = 3.0;',
            'new': '                    if int_property(&props, "moisture").is_some_and(|m| m > 0) {\n'
                   '                        point = 1.0;',
            'target': '-p mc-server --lib -E test(growth_speed_matches_the_jar_arithmetic)',
        },
        {
            'name': 'farmland wet arm',
            'file': 'crates/server/src/game/growth.rs',
            'old': 'let wet = self.farmland_is_near_water(x, y, z) || self.is_raining_at(x, y + 1, z);',
            'new': 'let wet = false;',
            'target': '-p mc-server --lib -E "test(wet_farmland_wets_on_direct_call) | '
                      'test(dark_farmland_still_wets_on_direct_call)"',
        },
        {
            'name': 'farmland dry arm',
            'file': 'crates/server/src/game/growth.rs',
            'old': '        if moisture > 0 {\n'
                   '            return self.write_property(x, y, z, "minecraft:farmland", "moisture", moisture - 1);\n'
                   '        }',
            'new': '        if false {\n'
                   '            return self.write_property(x, y, z, "minecraft:farmland", "moisture", moisture - 1);\n'
                   '        }',
            'target': '-p mc-server --lib -E test(dry_farmland_dries_then_dirts_on_direct_calls)',
        },
        {
            'name': 'sweep random_tick_speed',
            'file': 'crates/server/src/game/growth.rs',
            'old': 'pub(crate) const RANDOM_TICK_SPEED: usize = 3;',
            'new': 'pub(crate) const RANDOM_TICK_SPEED: usize = 2;',
            'target': '-p mc-server --test growth -E test(wheat_grows_through_the_phase)',
        },
    ],
}

SUMMARY = re.compile(r'Summary \[[^\]]*\] (\d+) tests? run: (\d+) passed(?:, (\d+) failed)?')


def prove(group: str) -> list[dict[str, str]]:
    """Run every case in `group`; return one row per case."""
    rows: list[dict[str, str]] = []
    for case in CASES[group]:
        path = ROOT / case['file']
        baseline = path.read_bytes()
        baseline_hash = hashlib.sha256(baseline).hexdigest()
        text = baseline.decode('utf-8')

        count = text.count(case['old'])
        if count != 1:
            raise SystemExit(f"{case['name']}: literal occurs {count} times in {case['file']}, expected 1")

        verdict = 'BUILD-FAIL'
        try:
            # Bytes, not a copy: a fresh write is a fresh mtime, so cargo cannot mistake the
            # perturbed binary for current (the trap in the module docstring).
            path.write_bytes(text.replace(case['old'], case['new']).encode('utf-8'))
            run = subprocess.run(
                f'cargo nextest run {case["target"]}',
                cwd=ROOT,
                shell=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding='utf-8',
                errors='replace',
            )
            match = SUMMARY.search(run.stdout)
            if match:
                total = int(match.group(1))
                failed = int(match.group(3) or 0)
                verdict = 'NO-TESTS' if total == 0 else ('RED' if failed else 'GREEN!')
                summary = match.group(0)
            else:
                summary = next(
                    (line for line in run.stdout.splitlines() if line.startswith('error')),
                    'no summary line and no error line',
                )
        finally:
            path.write_bytes(baseline)
            restored = hashlib.sha256(path.read_bytes()).hexdigest() == baseline_hash

        rows.append(
            {
                'mechanism': case['name'],
                'verdict': verdict,
                'restore': 'exact' if restored else 'MISMATCH',
                'detail': summary,
            }
        )
        print(f"  {case['name']:<26} {verdict:<9} restore={rows[-1]['restore']}  {summary}")
        if rows[-1]['restore'] != 'exact':
            raise SystemExit(f"{case['name']}: restore did not hash back to the baseline -- stopping")
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('group', nargs='?', default='all', help='case group to prove, or "all"')
    parser.add_argument('--list', action='store_true', help='list the cases and exit')
    args = parser.parse_args()

    if args.list:
        for group, cases in CASES.items():
            print(f'{group}:')
            for case in cases:
                print(f"  {case['name']:<26} {case['file']}  [{case['target']}]")
        return 0

    groups = list(CASES) if args.group == 'all' else [args.group]
    if any(group not in CASES for group in groups):
        print(f'unknown group: {args.group} (have {", ".join(CASES)})')
        return 2

    failed: list[str] = []
    for group in groups:
        print(f'--- {group} ({len(CASES[group])} mechanisms)')
        for row in prove(group):
            if row['verdict'] != 'RED' or row['restore'] != 'exact':
                failed.append(f"{group}/{row['mechanism']}: {row['verdict']}, restore={row['restore']}")

    print()
    if failed:
        print('REFUSING to pass -- a neutralised mechanism left its pin green, or a file did not restore:')
        for entry in failed:
            print(f'  - {entry}')
        return 1
    print('every mechanism was proven red by neutralising it, and every file restored byte-exact')
    return 0


if __name__ == '__main__':
    sys.exit(main())
