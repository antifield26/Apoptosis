"""Documentation encoding audit: every tracked text file must be valid UTF-8.

PowerShell consoles render UTF-8 em-dashes as GBK-looking sequences, which makes
a *reading* problem look like a *file* problem. This decodes every tracked text
file strictly and fails (exit 1) on any file that is not valid UTF-8 or that
contains replacement/mojibake marks. Run from anywhere inside the repository.

Usage: python tools/docs-audit/check_encoding.py
"""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEXT_SUFFIXES = ('.md', '.rs', '.toml', '.tsv', '.yml', '.yaml', '.txt', '.json', '.hex',
                 # research/reproduction sources committed under tools/ (Audit 07, M3):
                 # committing them without extending this list would leave the very files
                 # this check exists for outside its coverage.
                 '.py', '.java', '.service')
# Replacement character plus the characteristic GBK mojibake lead pairs.
MOJIBAKE = ('\ufffd', '\u9225', '\u9418', '\u951b', '\u9429')

files = subprocess.run(
    ['git', 'ls-files'], cwd=ROOT, capture_output=True, text=True, encoding='utf-8'
).stdout.splitlines()

bad_utf8 = []
mojibake = []
non_ascii = 0

for rel in files:
    if not rel.endswith(TEXT_SUFFIXES):
        continue
    raw = (ROOT / rel).read_bytes()
    try:
        text = raw.decode('utf-8')
    except UnicodeDecodeError as error:
        bad_utf8.append((rel, str(error)))
        continue
    if any(mark in text for mark in MOJIBAKE):
        mojibake.append(rel)
    if any(ord(ch) > 127 for ch in text):
        non_ascii += 1

checked = sum(1 for f in files if f.endswith(TEXT_SUFFIXES))
print(f'tracked text files checked      : {checked}')
print(f'files with non-ASCII characters : {non_ascii}')
print(f'files that are NOT valid UTF-8  : {len(bad_utf8)}')
for rel, error in bad_utf8:
    print(f'    {rel}: {error}')
print(f'files containing mojibake marks : {len(mojibake)}')
for rel in mojibake:
    print(f'    {rel}')
sys.exit(1 if (bad_utf8 or mojibake) else 0)
