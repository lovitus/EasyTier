#!/usr/bin/env python3
"""Follow-up observers on the original binary; diagnostic, not a Core patch."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess as sp
import time
from diagnose import Lab

class ObservedLab(Lab):
    phase = 'setup'

    def record(self, kind, value):
        value = {'phase': self.phase, **value}
        if kind == 'iperf':
            value['observer_attached'] = self.phase in ('udp-counts-only','tcp-counts-only','udp-perf-sampled')
        super().record(kind, value)

    def boundaries(self):
        return {
            'monotonic_ns': time.monotonic_ns(),
            'host_stat': Path('/proc/stat').read_text(),
            'cpu_pressure': Path('/proc/pressure/cpu').read_text(),
            'namespace_snmp': [self.cmd(self.ns(i, ['cat','/proc/net/snmp']),check=False).stdout for i in range(2)],
            'links': [json.loads(self.cmd(self.ns(i, ['ip','-j','-s','link'])).stdout) for i in range(2)],
            'cores': self.core_resources(),
        }

    def observed_transfer(self, phase, observer=None, rate=100, duration=6):
        self.phase = phase
        trace = None
        before = self.boundaries()
        if observer:
            trace = self.spawn(observer, phase+'-observer')
            time.sleep(.2)
        try:
            ok = self.iperf(rate=rate,reverse=True,duration=duration)
            after = self.boundaries()
        finally:
            if trace is not None and trace.poll() is None:
                os.killpg(trace.pid, signal.SIGINT)
                if self.wait(trace,4)==124: self.stop(trace)
        (self.out/(phase+'-boundaries.json')).write_text(json.dumps({'before':before,'after':after},indent=2))
        self.record('observer_status', {'label':phase,'observer':observer[0] if observer else None,
            'observer_exit':trace.returncode if trace else None,'transfer_ok':ok,
            'accept_as_uninstrumented_performance': observer is None})
        return ok

    def run(self):
        self.setup()
        self.phase='underlay-tool-pair'
        for rep in range(2):
            for version in (['system','3.9'] if rep == 0 else ['3.9','system']):
                self.iperf(version=version,rate=100,repeat=rep)
        self.start_mesh('udp')
        self.phase='warmup'; self.iperf(rate=100,reverse=True,duration=2)
        self.observed_transfer('udp-uninstrumented-before')
        # Counts-only ptrace run; do not use its CPU or throughput as production performance.
        traceargs=['strace','-f','-c','-qq','-e','trace=sendto,sendmsg,sendmmsg,recvfrom,recvmsg,recvmmsg,read,write,writev,epoll_wait,epoll_pwait,futex',
                   '-o',str(self.out/'udp-syscalls-counts.txt')]
        for core in self.cores: traceargs += ['-p',str(core.pid)]
        self.observed_transfer('udp-counts-only',traceargs,rate=50,duration=3)
        perf = shutil.which('perf')
        if perf:
            self.observed_transfer('udp-perf-sampled',[perf,'record','-e','cpu-clock','-F','199','--call-graph','fp',
                '-o',str(self.out/'udp-perf.data'),'-p',','.join(str(x.pid) for x in self.cores)],rate=200,duration=8)
            r=self.cmd([perf,'report','--stdio','--no-children','--percent-limit','1', '-i',str(self.out/'udp-perf.data')],check=False,timeout=30)
            (self.out/'udp-perf-self.txt').write_text(r.stdout+'\n'+r.stderr)
            self.record('perf_report',{'exit':r.returncode,'available':r.returncode==0})
        else:
            self.record('perf_report',{'available':False,'reason':'perf not installed'})
        self.observed_transfer('udp-uninstrumented-after')
        self.phase='completion-tool-replication'
        for rep in range(2):
            for host,reverse in [(0,False),(1,True),(1,False),(0,True)]:
                self.iperf(version='3.9',rate=100,client_host=host,reverse=reverse,repeat=rep)
        # A-B-A: only kernel features, same adapter and original binary.
        for phase,enabled in [('features-A',True),('features-B',False),('features-A-return',True)]:
            self.phase=phase; self.offload(enabled)
            self.observed_transfer(phase,rate=200)
            self.observed_transfer(phase+'-saturation',rate=0)
        # Separate application-UDP from the TCP workload; not a language benchmark.
        self.phase='udp-packet-workload'; self.iperf(rate=50,reverse=True,udp=True,duration=6)
        self.start_mesh('tcp')
        self.observed_transfer('tcp-uninstrumented-before')
        traceargs=['strace','-f','-c','-qq','-e','trace=sendto,sendmsg,sendmmsg,recvfrom,recvmsg,recvmmsg,read,write,writev,epoll_wait,epoll_pwait,futex',
                   '-o',str(self.out/'tcp-syscalls-counts.txt')]
        for core in self.cores: traceargs += ['-p',str(core.pid)]
        self.observed_transfer('tcp-counts-only',traceargs,rate=50,duration=3)
        self.observed_transfer('tcp-uninstrumented-after')
        self.phase='tcp-completion-replication'
        for rep in range(2):
            for host,reverse in [(0,False),(1,True),(1,False),(0,True)]:
                self.iperf(version='3.9',rate=100,client_host=host,reverse=reverse,repeat=rep)
        self.halfclose(0); self.halfclose(1)
        self.record('suite_finished',{'test_failures':sum(x.get('ok') is False for x in self.results),
            'note':'observer runs are diagnostics; immutable Core unchanged'})

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--bundle',required=True);p.add_argument('--output',required=True);p.add_argument('--iperf39',required=True)
    a=p.parse_args();lab=ObservedLab(a.bundle,a.output,a.iperf39)
    def stop(sig, frame): raise RuntimeError(f'interrupted by {sig}')
    signal.signal(signal.SIGINT,stop);signal.signal(signal.SIGTERM,stop)
    try:lab.run()
    except Exception as exc:
        lab.record('harness_failure',{'error':repr(exc)});raise
    finally:lab.cleanup()
