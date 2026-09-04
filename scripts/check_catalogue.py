#!/usr/bin/env python3
"""meclaw -- the public template catalogue must name the shipped versions.

`templates/README.md` is the catalogue a reader trusts: it lists every public
template with the version it is at. Each `templates/<name>/template.json`
carries the version that actually ships. When a template is bumped and the
catalogue is not, the catalogue promises a version nobody can install.

This gate compares the two. A template that is NOT listed in the catalogue is
private and therefore not a finding -- only a listed template at a stale
version is. Exit 0 = the catalogue is in step.

Until 2026-09-04 this lived as an inline heredoc in `scripts/strand-gate.sh`;
it is a file now so `scripts/gate.sh` can plan it as a station like any other.
"""

import json, re, sys, pathlib
root = pathlib.Path('.')
bad = []
table = (root/'templates/README.md').read_text()
for tj in root.glob('templates/*/template.json'):
    name = tj.parent.name
    if name.startswith('_'): continue
    ver = json.loads(tj.read_text()).get('version')
    # private templates are not in the public catalogue at all -- only a
    # listed template with a stale version is a finding
    listed = re.search(rf'(`{re.escape(name)}`|{re.escape(name)}@)', table)
    if ver and listed and not re.search(rf'{re.escape(name)}(`[^\n]*|@){re.escape(ver)}', table):
        bad.append(f'{name}@{ver} not at that version in templates/README.md')
print('\n'.join(bad) or 'catalogue ok'); sys.exit(1 if bad else 0)
