#!/usr/bin/env python3
"""Source-independent, test-process-only causal intervention for iperf completion."""
import argparse
import os
from pathlib import Path
import signal
from diagnose import Lab

class ControlLab(Lab):
    shim_mode = None
    phase = 'setup'
    def record(self, kind, value):
        super().record(kind, {'phase':self.phase,'shim_mode':self.shim_mode,**value})
    def spawn(self, args, name, env=None):
        # Only the isolated test server receives this library, never Core/client.
        if self.shim_mode is not None and name.endswith('-server') and any('iperf3' in str(x) for x in args):
            env = {**os.environ, **(env or {}), 'LD_PRELOAD':str(Path('ci-evidence/select-diagnostic.so').resolve()),
                   'ETCI_SELECT_INTERVENTION':str(self.shim_mode)}
        return super().spawn(args,name,env)
    def series(self):
        for index,mode in enumerate([None,0,1,1,0,None]):
            self.shim_mode=mode
            self.phase=f'control-{self.mode}-{index}'
            self.iperf(rate=100,reverse=False,duration=6,repeat=index)
        self.shim_mode=None
        self.phase=f'control-{self.mode}-strace'
        self.iperf(rate=100,trace=True,duration=6)
    def run(self):
        self.setup()
        self.series()
        self.start_mesh('udp')
        self.series()
        self.halfclose(0);self.halfclose(1)
        self.record('suite_finished',{'note':'diagnostic intervention is not a proposed production workaround',
            'test_failures':sum(x.get('ok') is False for x in self.results)})

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--bundle',required=True);p.add_argument('--output',required=True);p.add_argument('--iperf39',required=True)
    a=p.parse_args();lab=ControlLab(a.bundle,a.output,a.iperf39)
    def stop(sig, frame): raise RuntimeError(f'interrupted by {sig}')
    signal.signal(signal.SIGINT,stop);signal.signal(signal.SIGTERM,stop)
    try:lab.run()
    except Exception as exc:
        lab.record('harness_failure',{'error':repr(exc)});raise
    finally:lab.cleanup()
