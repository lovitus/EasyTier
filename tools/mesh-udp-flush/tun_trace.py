"""Bounded syscall observation for isolated namespaces, never production tracing."""
import json
import os
from pathlib import Path
import shutil


class TunWriteTrace:
    def __init__(self, out, label, cores):
        self.out = out
        self.label = label
        self.path = Path('/sys/kernel/tracing/instances') / f'et-tun-{os.getpid()}'
        self.cores = cores

    def identities(self):
        result = []
        for core in self.cores:
            root = Path(f'/proc/{core.pid}')
            tids = sorted(int(p.name) for p in (root / 'task').iterdir())
            fds = []
            for fd in (root / 'fd').iterdir():
                try:
                    if os.readlink(fd) == '/dev/net/tun':
                        fds.append(int(fd.name))
                except FileNotFoundError:
                    pass
            if not fds:
                raise RuntimeError('missing TUN descriptor')
            result.append({'pid': core.pid, 'tids': tids, 'tun_fds': sorted(fds)})
        return result

    def start(self):
        before = self.identities()
        (self.out / (self.label + '-tun-identities-before.json')).write_text(json.dumps(before))
        self.path.mkdir()
        (self.path / 'tracing_on').write_text('0')
        (self.path / 'buffer_size_kb').write_text('16384')
        tids = sorted({tid for row in before for tid in row['tids']})
        condition = ' || '.join(f'common_pid == {tid}' for tid in tids)
        for phase in ['enter', 'exit']:
            event = self.path / 'events/syscalls' / f'sys_{phase}_write'
            (event / 'filter').write_text(condition)
            shutil.copyfile(event / 'format', self.out / (self.label + f'-tun-{phase}-format.txt'))
            (event / 'enable').write_text('1')
        (self.path / 'tracing_on').write_text('1')
        self.before = before

    def finish(self):
        (self.path / 'tracing_on').write_text('0')
        shutil.copyfile(self.path / 'trace', self.out / (self.label + '-tun-writes.txt'))
        stats = {p.parent.name: p.read_text() for p in (self.path / 'per_cpu').glob('cpu*/stats')}
        (self.out / (self.label + '-tun-trace-stats.json')).write_text(json.dumps(stats))
        after = self.identities()
        (self.out / (self.label + '-tun-identities-after.json')).write_text(json.dumps(after))
        if after != self.before:
            raise RuntimeError('TUN fd/thread identities changed; trace cannot prove complete coverage')
        if not stats:
            raise RuntimeError('missing trace loss counters')
        for content in stats.values():
            values = dict(line.split(':', 1) for line in content.splitlines() if ':' in line)
            for key in ['overrun', 'commit overrun', 'dropped events']:
                if int(values[key]) != 0:
                    raise RuntimeError('trace lost events')

    def close(self):
        if self.path.exists():
            (self.path / 'tracing_on').write_text('0')
            for phase in ['enter', 'exit']:
                event = self.path / 'events/syscalls' / f'sys_{phase}_write/enable'
                if event.exists():
                    event.write_text('0')
            self.path.rmdir()
