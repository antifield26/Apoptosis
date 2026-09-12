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
# Replacement character plus the characteristic GBK mojibake lead pairs.
MOJIBAKE = ('\ufffd', '\u9225', '\u9418', '\u951b', '\u9429')

# Coverage is derived from git's own text/binary classification
# (`git ls-files --eol`), not a suffix list: Audit 08 (M1) showed a suffix
# filter silently excluding LICENSE, NOTICE, .gitattributes, .gitignore,
# Cargo.lock and .editorconfig — exactly the files a fork could mangle.
# A file is checked unless git marks it binary (-text/-bin).
eol_lines = subprocess.run(
    ['git', 'ls-files', '--eol'], cwd=ROOT, capture_output=True, text=True,
    encoding='utf-8', errors='replace'
).stdout.splitlines()

files = []
for line in eol_lines:
    if '\t' not in line:
        continue
    attrs, rel = line.split('\t', 1)
    if '-text' in attrs or 'binary' in attrs or '-bin' in attrs:
        continue
    files.append(rel)

bad_utf8 = []
mojibake = []
non_ascii = 0

for rel in files:
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

checked = len(files)
print(f'tracked text files checked      : {checked}')
print(f'files with non-ASCII characters : {non_ascii}')
print(f'files that are NOT valid UTF-8  : {len(bad_utf8)}')
for rel, error in bad_utf8:
    print(f'    {rel}: {error}')
print(f'files containing mojibake marks : {len(mojibake)}')
for rel in mojibake:
    print(f'    {rel}')
sys.exit(1 if (bad_utf8 or mojibake) else 0)
