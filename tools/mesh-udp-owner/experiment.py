#!/usr/bin/env python3
"""Causal test of one outbound UDP handoff; not a deployable feature."""
import argparse,hashlib,json,os,re,signal,statistics,sys,time,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--stock',required=True);p.add_argument('--probe',required=True)
p.add_argument('--iperf',required=True);p.add_argument('--output',required=True)
a=p.parse_args();paths={'stock':Path(a.stock).resolve(),'probe':Path(a.probe).resolve()}
shas={k:hashlib.sha256((d/'easytier-core').read_bytes()).hexdigest() for k,d in paths.items()}
source=Path('tools/mesh-ci/diagnose.py').read_text()
old='95c5724367ce4ab5a33688f871cc1531c50d84e78389e0c77d4d17ea99b5dc71'
assert source.count(old)==1
source=source.replace(old,shas['stock']).replace('391c191c3d8b477c3b7e8ef19e87ae3cba5c9504','c6772dbfef2395ff96b39bd4801945d92212dffb')
m=types.ModuleType('owner_fixture');m.__file__=str(Path('tools/mesh-ci/diagnose.py').resolve())
exec(compile(source,m.__file__,'exec'),m.__dict__)
class Trial(m.Lab):
    arm='stock';round_id=-1;phase='environment'
    def __init__(self,*args):
        self.process_origin={};super().__init__(*args)
    def record(self,kind,value):
        super().record(kind,{'arm':self.arm,'round':self.round_id,'phase':self.phase,
            'diagnostic_only':self.arm!='stock','binary_sha256':shas['stock' if self.arm=='stock' else 'probe'],**value})
    def spawn(self,args,name,env=None):
        core=name.startswith('core-')
        if core:
            name=f'core-r{self.round_id}-{self.arm}-{name}'
            env={**(env or os.environ),'ET_ISSUE4_TX_MODE':'inline' if self.arm=='inline' else 'queued',
                 'ET_ISSUE4_TX_GUARD':'ISOLATED_TWO_PEER_ONLY'}
        proc=super().spawn(args,name,env)
        if core:self.process_origin[proc.pid]={'launched_arm':self.arm,'launched_round':self.round_id,'log':name+'.err'}
        return proc
    def snapshot(self,label,extra=()):return super().snapshot(f'r{self.round_id}-{self.arm}-{label}',extra)
    def stop(self,proc):
        if proc.poll() is not None:return
        start=time.monotonic();escalated=False
        try:os.killpg(proc.pid,signal.SIGTERM);proc.wait(timeout=3)
        except ProcessLookupError:pass
        except m.sp.TimeoutExpired:
            escalated=True
            try:os.killpg(proc.pid,signal.SIGKILL)
            except ProcessLookupError:pass
            proc.wait(timeout=3)
        if proc.pid in self.process_origin:
            self.record('core_stop',{'pid':proc.pid,**self.process_origin[proc.pid],
                'exit':proc.returncode,'kill_escalation':escalated,'wait_s':time.monotonic()-start,
                'ok':not escalated and proc.returncode==0})
    def idle(self,label):
        before=self.core_resources();start=time.monotonic();time.sleep(2);after=self.core_resources();elapsed=time.monotonic()-start
        cpu=sum(b['user_s']+b['system_s']-x['user_s']-x['system_s'] for x,b in zip(before,after))
        self.record('idle',{'before':before,'after':after,'elapsed_s':elapsed,'combined_core_equivalents':cpu/elapsed})
    def run(self):
        self.setup();self.iperfs['verified-3.21']=str(Path(a.iperf).resolve())
        self.phase='underlay-tool-control'
        for rev in [False,True]:self.iperf(version='verified-3.21',rate=100,reverse=rev,duration=3)
        order=['stock','queued','inline','inline','queued','queued','inline','stock']
        for self.round_id,self.arm in enumerate(order):
            d=paths['stock' if self.arm=='stock' else 'probe'];self.core=d/'easytier-core';self.cli=d/'easytier-cli'
            self.phase='startup';self.start_mesh('udp')
            self.phase='integrity-eof';self.halfclose(0);self.halfclose(1)
            if self.arm!='stock':
                for proc in self.cores:
                    log=(self.out/self.process_origin[proc.pid]['log']).read_text(errors='replace')
                    assert f'ISSUE4_TX_OWNER mode={self.arm};' in log,'probe did not activate'
                self.record('activation',{'ok':True,'mode':self.arm,'processes':len(self.cores)})
            self.phase='warmup';self.iperf(version='verified-3.21',rate=100,reverse=True,duration=2)
            self.phase='fixed-100'
            for rev in [False,True]:self.iperf(version='verified-3.21',rate=100,reverse=rev,duration=4)
            self.phase='saturation'
            for rev in [False,True]:
                label=f'ping-r{self.round_id}-{int(rev)}'
                ping=self.spawn(self.ns(0,['ping','-n','-i','0.1','-c','40','-w','7','10.89.0.2']),label)
                self.iperf(version='verified-3.21',rate=0,reverse=rev,duration=5)
                rc=self.wait(ping,3);text=(self.out/(label+'.out')).read_text()
                match=re.search(r'(\d+) packets transmitted, (\d+) received',text)
                counts=[int(x) for x in match.groups()] if match else None
                self.record('ping_progress',{'ok':rc==0 and counts==[40,40],'exit':rc,
                    'counts':counts,'during_reverse':rev,'file':label+'.out','summary':text.splitlines()[-3:]});self.stop(ping)
            self.phase='sparse-udp-1200'
            for rev in [False,True]:self.iperf(version='verified-3.21',rate=1,reverse=rev,udp=True,duration=3)
        self.record('suite_finished',{'ok':not any(x.get('ok') is False for x in self.results)})
lab=Trial(str(paths['stock']),a.output,a.iperf)
def interrupted(sig,frame):raise RuntimeError(f'signal {sig}')
signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
try:lab.run()
except Exception as exc:lab.record('harness_failure',{'error':repr(exc)});raise
finally:lab.cleanup()
rows=lab.results;groups=[]
for phase in ['fixed-100','saturation','sparse-udp-1200']:
 for direction in ['0->1','1->0']:
  arms={}
  for arm in ['stock','queued','inline']:
   rr=[r for r in rows if r['kind']=='iperf' and r.get('ok') and r['mode']=='udp' and r['phase']==phase and r['payload_direction']==direction and r['arm']==arm]
   if rr:arms[arm]={'n':len(rr),**{k:[min(r[k] for r in rr),statistics.median(r[k] for r in rr),max(r[k] for r in rr)] for k in ['goodput_mbps','cpu_s_per_GiB']}}
  item={'phase':phase,'direction':direction,'arms':arms};groups.append(item);print('TXOWNER_SUMMARY '+json.dumps(item),flush=True)
Path(a.output,'owner-summary.json').write_text(json.dumps(groups,indent=2))
clean=next((r for r in reversed(rows) if r['kind']=='cleanup'),{})
complete=any(r['kind']=='suite_finished' for r in rows)
valid=complete and clean.get('root_routes_unchanged') and all(x['delete_exit']==0 and not x['residual_pids'] for x in clean.get('namespaces',[]))
failed=[r for r in rows if r.get('ok') is False or r['kind']=='harness_failure']
Path(a.output,'owner-outcome.json').write_text(json.dumps({'complete':complete,'cleanup':clean,'failed':failed},indent=2))
sys.exit(0 if valid and not failed else 1)
