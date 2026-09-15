#!/usr/bin/env python3
"""Matched rebuilt Core A/B; isolated runner only, no original-artifact mixing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import sys
import time
import types
p=argparse.ArgumentParser()
p.add_argument('--baseline',required=True);p.add_argument('--candidate',required=True)
p.add_argument('--manifest',required=True);p.add_argument('--iperf',required=True);p.add_argument('--output',required=True)
a=p.parse_args()
manifest=json.loads(Path(a.manifest).read_text())
paths={'baseline':Path(a.baseline).resolve(),'candidate':Path(a.candidate).resolve()}
for k,d in paths.items():
    if hashlib.sha256((d/'easytier-core').read_bytes()).hexdigest()!=manifest[k]['sha256']:raise RuntimeError('built binary provenance mismatch: '+k)
source=Path('tools/mesh-ci/diagnose.py').read_text()
old_hash='95c5724367ce4ab5a33688f871cc1531c50d84e78389e0c77d4d17ea99b5dc71'
assert source.count(old_hash)==1, 'unexpected harness hash contract'
source=source.replace(old_hash,manifest['baseline']['sha256'])
source=source.replace('391c191c3d8b477c3b7e8ef19e87ae3cba5c9504',manifest['source'])
module=types.ModuleType('verified_diagnose');module.__file__=str(Path('tools/mesh-ci/diagnose.py').resolve())
exec(compile(source,module.__file__,'exec'),module.__dict__)
class AB(module.Lab):
    variant='baseline';repetition=-1;phase='startup'
    def record(self,kind,value):
        super().record(kind,{'variant':self.variant,'repetition':self.repetition,'phase':self.phase,'core_sha256':manifest[self.variant]['sha256'],'candidate_scope':'packet extraction only',**value})
    def spawn(self,args,name,env=None):
        if name.startswith('core-'):name=f'{self.repetition}-{self.variant}-'+name
        return super().spawn(args,name,env)
    def snapshot(self,label,extra=()):return super().snapshot(f'{self.repetition}-{self.variant}-'+label,extra)
    def idle(self,label):
        before=self.core_resources();start=time.monotonic();time.sleep(3);after=self.core_resources()
        elapsed=time.monotonic()-start
        cpu=sum(b['user_s']+b['system_s']-x['user_s']-x['system_s'] for x,b in zip(before,after))
        self.record('idle',{'label':label,'elapsed_s':elapsed,'before':before,'after':after,'combined_core_equivalents':cpu/elapsed})
    def run(self):
        self.setup();self.iperfs['verified-3.21']=str(Path(a.iperf).resolve());self.phase='measurement-tool-native'
        for host,rev in [(0,False),(0,True),(1,False),(1,True)]:self.iperf(version='verified-3.21',rate=100,reverse=rev,client_host=host,duration=3)
        order=['baseline','candidate','candidate','baseline','baseline','candidate','candidate','baseline','baseline','candidate']
        for self.repetition,self.variant in enumerate(order):
            self.bundle=paths[self.variant];self.core=self.bundle/'easytier-core';self.cli=self.bundle/'easytier-cli'
            for carrier in ['udp','tcp']:
                self.phase='start-'+carrier;self.start_mesh(carrier)
                self.phase='integrity-halfclose';self.halfclose(0);self.halfclose(1)
                self.phase='warmup';self.iperf(version='verified-3.21',rate=100,reverse=True,duration=2)
                self.phase='fixed-100';self.iperf(version='verified-3.21',rate=100,reverse=False,duration=4);self.iperf(version='verified-3.21',rate=100,reverse=True,duration=4)
                self.phase='saturation-control-progress'
                ping=self.spawn(self.ns(0,['ping','-n','-i','0.05','-c','80','-w','7','10.89.0.2']),f'ping-{self.repetition}-{carrier}')
                self.iperf(version='verified-3.21',rate=0,reverse=True,duration=5)
                rc=self.wait(ping,3);self.record('control_ping',{'exit':rc,'ok':rc==0,'file':f'ping-{self.repetition}-{carrier}.out'});self.stop(ping)
                self.phase='sparse-small-udp';self.iperf(version='verified-3.21',rate=1,reverse=True,udp=True,duration=3)
        failed=[r for r in self.results if r.get('ok') is False]
        self.record('suite_finished',{'failed_cases':len(failed),'ok':not failed})
lab=AB(str(paths['baseline']),a.output,a.iperf)
def stop(sig,frame):raise RuntimeError('signal '+str(sig))
signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
try:lab.run()
except Exception as exc:
    lab.record('harness_failure',{'error':repr(exc)});raise
finally:lab.cleanup()
rows=lab.results
keys=sorted({(r['mode'],r['phase']) for r in rows if r['kind']=='iperf' and r.get('ok') and r['mode']!='underlay' and r['phase']!='warmup'})
for mode,phase in keys:
    metrics={}
    for variant in ['baseline','candidate']:
        rr=[r for r in rows if r['kind']=='iperf' and r.get('ok') and r['mode']==mode and r['phase']==phase and r['variant']==variant]
        for direction in sorted({r['payload_direction'] for r in rr}):
            selected=[r for r in rr if r['payload_direction']==direction]
            metrics[variant+' '+direction]={k:[min(r[k] for r in selected),statistics.median(r[k] for r in selected),max(r[k] for r in selected)] for k in ['goodput_mbps','cpu_s_per_GiB']}
            metrics[variant+' '+direction]['n']=len(selected)
    print('ETVERIFY_CORE_AB '+json.dumps({'carrier':mode,'phase':phase,'metrics':metrics}),flush=True)
clean=next((r for r in reversed(rows) if r['kind']=='cleanup'),{})
valid=any(r['kind']=='suite_finished' for r in rows) and clean.get('root_routes_unchanged') and all(x['delete_exit']==0 and not x['residual_pids'] for x in clean.get('namespaces',[]))
if not valid or any(r.get('ok') is False or r['kind']=='harness_failure' for r in rows):sys.exit(1)
