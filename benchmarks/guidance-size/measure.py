#!/usr/bin/env python3
"""Measure generated guidance bytes without launching Packet28 or its hooks."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = 'crates/suite-cli/src/agent_surface.rs'
FORMATS = ['Claude', 'Agents', 'Cursor', 'CursorRule', 'WindsurfRule']


def measure(source, rustc):
    # This pure renderer only needs clap for argument parsing. Remove that derive
    # to compile its exact rendering functions in a dependency-free test driver.
    standalone = source.replace('use clap::ValueEnum;', '').replace(', ValueEnum', '')
    standalone += '\nfn main() {\n'
    for name in FORMATS:
        standalone += f'println!("{{}}", render_prompt_fragment(AgentPromptFormat::{name}, None).len());\n'
    standalone += '}\n'
    with tempfile.TemporaryDirectory(prefix='guidance-size-') as directory:
        directory = Path(directory)
        path = directory / 'measure.rs'
        path.write_text(standalone)
        binary = directory / 'measure'
        subprocess.run([rustc, '--edition=2021', '-Awarnings', str(path), '-o', str(binary)], check=True)
        lengths = subprocess.check_output([str(binary)], text=True).splitlines()
    return dict(zip(FORMATS, map(int, lengths)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True, help='Existing Git revision to compare')
    args = parser.parse_args()
    rustc = subprocess.check_output(['rustup', 'which', 'rustc'], cwd=ROOT, text=True).strip()
    revision = subprocess.check_output(['git', 'rev-parse', args.base], cwd=ROOT, text=True).strip()
    before = subprocess.check_output(['git', 'show', f'{revision}:{SOURCE}'], cwd=ROOT, text=True)
    after = (ROOT / SOURCE).read_text()
    before_bytes, after_bytes = measure(before, rustc), measure(after, rustc)
    print(json.dumps({
        'base_revision': revision,
        'renderer_sha256': hashlib.sha256(after.encode()).hexdigest(),
        'rustc': subprocess.check_output([rustc, '--version'], text=True).strip(),
        'command': f'python3 benchmarks/guidance-size/measure.py --base {revision}',
        'scope': 'UTF-8 bytes, no provider token, latency, cache-hit, or adherence claim',
        'formats': {name: {'before_bytes': before_bytes[name], 'after_bytes': after_bytes[name],
                           'removed_bytes': before_bytes[name] - after_bytes[name]} for name in FORMATS},
    }, indent=2))


if __name__ == '__main__':
    main()
