#!/usr/bin/env python3
"""Run the existing lab against immutable packaged Core binaries, without overlays."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile


PREFIX = 'easytier-no-leaf-comparator-linux-x86_64-musl'
CLI_ARTIFACT = 10895437417
CLI_ARCHIVE_SHA = 'e321fcb6e1fe1b1937f98fdc170f0865510effd23ab1fa004312520916647a06'
CLI_SHA = '4367176ed12dcee87f177f0faeb6c03a9e60f6a96fa47edf60da2756fbb6341e'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def api(path):
    return json.loads(subprocess.check_output(
        ['gh', 'api', 'repos/lovitus/EasyTier/' + path], timeout=120))


def download(artifact_id, destination, expected_sha=None):
    assert isinstance(artifact_id, int) and artifact_id > 0
    metadata = api(f'actions/artifacts/{artifact_id}')
    assert not metadata['expired'], 'artifact expired; do not silently rebuild'
    expected = metadata['digest'].removeprefix('sha256:')
    assert re.fullmatch('[0-9a-f]{64}', expected)
    if expected_sha:
        assert expected == expected_sha, 'artifact provenance changed'
    with destination.open('wb') as stream:
        subprocess.run(['gh', 'api', f'repos/lovitus/EasyTier/actions/artifacts/{artifact_id}/zip'],
                       stdout=stream, check=True, timeout=180)
    assert digest(destination) == expected, 'outer artifact digest mismatch'
    return metadata


def prepare(root):
    pair = json.loads(os.environ['ARTIFACT_PAIR'])
    assert set(pair) == {'baseline', 'candidate'}
    identities = {}
    for label, spec in pair.items():
        assert set(spec) == {'id', 'sha'} and re.fullmatch('[0-9a-f]{40}', spec['sha'])
        directory = root / 'binaries' / label
        directory.mkdir(parents=True, exist_ok=False)
        archive = directory / 'artifact.zip'
        metadata = download(spec['id'], archive)
        assert metadata['workflow_run']['head_sha'] == spec['sha']
        with zipfile.ZipFile(archive) as zipped:
            expected_files = {PREFIX + '.tar.gz', 'SHA256SUMS.txt'}
            assert set(zipped.namelist()) == expected_files
            expected, filename = zipped.read('SHA256SUMS.txt').decode().strip().split()
            assert filename == PREFIX + '.tar.gz'
            tar_path = directory / filename
            with zipped.open(filename) as source, tar_path.open('wb') as target:
                shutil.copyfileobj(source, target)
        assert digest(tar_path) == expected, 'inner archive digest mismatch'
        with tarfile.open(tar_path, 'r:gz') as packed:
            def text(name):
                member = packed.getmember(PREFIX + '/' + name)
                assert member.isfile() and member.size <= 65536
                return packed.extractfile(member).read().decode()
            info = text('BUILD_INFO.txt')
            fields = dict(line.split('=', 1) for line in info.splitlines() if '=' in line)
            assert fields['commit'] == spec['sha']
            assert fields['target'] == 'x86_64-unknown-linux-musl'
            assert fields['feature_set'] == 'jemalloc' and fields['leaf_policy_features'] == 'false'
            assert fields['audit_comparator'] == 'true'
            expected, filename = text('SHA256SUMS.txt').strip().split()
            assert filename == 'easytier-core-no-leaf'
            member = packed.getmember(PREFIX + '/' + filename)
            assert member.isfile() and 0 < member.size < 1024**3
            binary = directory / filename
            with packed.extractfile(member) as source, binary.open('wb') as target:
                shutil.copyfileobj(source, target)
        assert digest(binary) == expected, 'Core payload digest mismatch'
        binary.chmod(0o755)
        (root / (label + '-BUILD_INFO.txt')).write_text(info)
        identities[label] = {**spec, 'run_id': metadata['workflow_run']['id'],
                             'binary_sha256': expected, 'artifact_digest': metadata['digest'],
                             'rustc': next(line for line in info.splitlines() if line.startswith('rustc '))}
        for tool, arguments in [('file', []), ('readelf', ['-n'])]:
            result = subprocess.check_output([tool, *arguments, str(binary)], text=True, timeout=20)
            (root / f'{label}-{tool}.txt').write_text(result)
        archive.unlink()
        tar_path.unlink()
    assert identities['baseline']['rustc'] == identities['candidate']['rustc']
    # The UDP-only integration does not change the RPC contract. Reuse the
    # independently checksummed base CLI rather than rebuilding Core for a CLI.
    archive = root / 'cli-artifact.zip'
    download(CLI_ARTIFACT, archive, CLI_ARCHIVE_SHA)
    with zipfile.ZipFile(archive) as zipped:
        matches = [name for name in zipped.namelist()
                   if name == 'stock/easytier-cli' or name.endswith('/stock/easytier-cli')]
        assert len(matches) == 1, 'missing or ambiguous immutable stock CLI'
        for label in pair:
            target = root / 'binaries' / label / 'easytier-cli'
            with zipped.open(matches[0]) as source, target.open('wb') as destination:
                shutil.copyfileobj(source, destination)
            assert digest(target) == CLI_SHA
            target.chmod(0o755)
    archive.unlink()
    (root / 'provenance.json').write_text(json.dumps({
        'harness_sha': os.environ['GITHUB_SHA'], 'cores': identities,
        'cli_artifact': CLI_ARTIFACT, 'cli_sha256': CLI_SHA,
        'scope': 'unmodified packages; namespace lab; no WAN or original-host claim'
    }, indent=2))


def run(root, phase, probe):
    lab = Path(__file__).with_name('lab.py').resolve()
    suite = root / phase
    suite.mkdir(exist_ok=False)
    results = []
    # Keep the lab's stock arm: it does not enable or require diagnostic source
    # overlays. The actual arm/source identity is recorded outside that lab.
    order = ['baseline', 'candidate', 'candidate', 'baseline', 'baseline', 'candidate']
    cases = [(family, stealth, label) for family in (4, 6)
             for stealth in ((False, True) if phase == 'fixed' else (False,)) for label in order]
    if phase == 'trace':
        cases = [(4, True, 'candidate')]
    for index, (family, stealth, label) in enumerate(cases):
        output = suite / f'{index:02d}-{label}-v{family}-stealth{int(stealth)}'
        binaries = root / 'binaries' / label
        command = [sys.executable, str(lab), '--stock', str(binaries), '--candidate', str(binaries),
                   '--core-name', 'easytier-core-no-leaf', '--order', 'stock', '--output', str(output),
                   '--network-counters']
        if family == 6:
            command.append('--inner-ipv6')
        if stealth:
            command.append('--stealth')
        if phase == 'saturation':
            assert probe and probe.is_file()
            command += ['--unpaced-probe', str(probe.resolve())]
        if phase == 'trace':
            command += ['--paced-mbps', '50']
            command = ['strace', '-ff', '-qq', '-s', '1', '-e', 'trace=sendmsg',
                       '-o', str(suite / 'sendmsg'), *command]
        with (suite / f'{index:02d}.log').open('w') as log:
            completed = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=360)
        row = {'label': label, 'inner_ip_family': family, 'underlay_ip_family': 4,
               'stealth': stealth, 'exit_code': completed.returncode, 'output': str(output.relative_to(root))}
        results.append(row)
        (suite / 'runs.json').write_text(json.dumps(results, indent=2))
        assert completed.returncode == 0, f'{phase} {label} failed; original assertions and logs retained'
    if phase == 'trace':
        successes = 0
        for path in suite.glob('sendmsg.*'):
            for line in path.open(errors='replace'):
                if ('sendmsg(' in line and ('UDP_SEGMENT' in line or 'cmsg_type=0x67' in line)
                        and re.search(r'= [1-9][0-9]*\s*$', line)):
                    successes += 1
        (suite / 'gso-activation.json').write_text(json.dumps({
            'successful_gso_submissions': successes, 'performance_evidence': False}))
        assert successes > 0, 'no successful UDP_SEGMENT submission observed'


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=['prepare', 'fixed', 'trace', 'saturation'])
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--probe', type=Path)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    if args.phase == 'prepare':
        prepare(root)
    else:
        run(root, args.phase, args.probe)
