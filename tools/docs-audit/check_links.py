"""Documentation link audit: the objective half of a docs review.

"Consolidate the docs" is a judgement call, but broken references are not. For
every tracked markdown file this checks, mechanically:

  1. markdown links `[text](path)` whose target does not exist;
  2. backticked paths that look like repo files (`docs/...`, `crates/...`,
     `apps/...`, `deploy/...`) and do not exist;
  3. references to files that are not tracked by git (for example a prompt
     pack that is deliberately git-ignored) — a document that tells a reader
     to consult an unshipped file is telling them to consult a file they do
     not have.

Exits 1 if any finding exists. Run from anywhere inside the repository.

Usage: python tools/docs-audit/check_links.py
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

tracked = subprocess.run(
    ['git', 'ls-files'], cwd=ROOT, capture_output=True, text=True, encoding='utf-8'
).stdout.splitlines()
tracked_set = {f.replace('\\', '/') for f in tracked}

docs = [f for f in tracked_set if f.startswith('docs/') and f.endswith('.md')]
print(f'checking {len(docs)} markdown docs\n')

LINK = re.compile(r'\[[^\]]*\]\(([^)#\s]+?)(?:#[^)]*)?\)')
BACKTICK_PATH = re.compile(r'`((?:docs|crates|apps|deploy|tools|target|OpenSourceMinecraftServer)/[A-Za-z0-9_./-]+)`')
UNTRACKED_NAMES = (
    'AGENTS.md', 'TASK-INDEX.md', 'MASTER-PROMPT.md', 'EXECUTION-LOOP.md',
    'EXIT-GATES.md', 'DEFINITION-OF-DONE.md', 'mc-rust-agent-prompts',
)

broken_links: list[tuple[str, str]] = []
broken_paths: list[tuple[str, str]] = []
untracked_refs: list[tuple[str, str]] = []

for rel in docs:
    text = (ROOT / rel).read_text(encoding='utf-8', errors='strict')
    for match in LINK.finditer(text):
        target = match.group(1)
        if target.startswith(('http://', 'https://', 'mailto:')):
            continue
        resolved = (Path(rel).parent / target).as_posix()
        parts: list[str] = []
        for segment in resolved.split('/'):
            if segment in ('', '.'):
                continue
            if segment == '..':
                if parts:
                    parts.pop()
            else:
                parts.append(segment)
        normalised = '/'.join(parts)
        if normalised not in tracked_set and not (ROOT / normalised).exists():
            broken_links.append((rel, target))
    for match in BACKTICK_PATH.finditer(text):
        target = match.group(1)
        # Paths under a declared external-clone root are citations of another
        # repository, not of this one.
        if target.startswith('OpenSourceMinecraftServer/'):
            continue
        if target not in tracked_set and not (ROOT / target).exists():
            broken_paths.append((rel, target))
    for name in UNTRACKED_NAMES:
        if name in text:
            untracked_refs.append((rel, name))

def report(title: str, items: list[tuple[str, str]]) -> None:
    print(f'{title}: {len(items)}')
    seen: set[tuple[str, str]] = set()
    for rel, target in items:
        if (rel, target) in seen:
            continue
        seen.add((rel, target))
        print(f'    {rel} -> {target}')
    print()

report('BROKEN markdown links', broken_links)
report('BROKEN backticked paths', broken_paths)
report('references to untracked files', untracked_refs)
sys.exit(1 if (broken_links or broken_paths or untracked_refs) else 0)
