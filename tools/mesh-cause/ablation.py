"""Ablate independently in a static no-SOCKS, no-policy namespace fixture."""
import argparse, hashlib, json, os, pathlib, re, signal, statistics, sys, time, types
p=argparse.ArgumentParser()
p.add_argument('--stock',required=True);p.add_argument('--probe',required=True)
p.add_argument('--iperf',required=True);p.add_argument('--output',required=True)
a=p.parse_args(); stock=pathlib.Path(a.stock).resolve();probe=pathlib.Path(a.probe).resolve()
sha={k:hashlib.sha256((v/'easytier-core').read_bytes()).hexdigest() for k,v in [('stock',stock),('probe',probe)]}
source=pathlib.Path('tools/mesh-ci/diagnose.py').read_text()
old='95c5724367ce4ab5a33688f871cc1531c50d84e78389e0c77d4d17ea99b5dc71'
assert source.count(old)==1
source=source.replace(old,sha['stock']).replace('391c191c3d8b477c3b7e8ef19e87ae3cba5c9504','c6772dbfef2395ff96b39bd4801945d92212dffb')
m=types.ModuleType('issue4_fixture');m.__file__=str(pathlib.Path('tools/mesh-ci/diagnose.py').resolve())
exec(compile(source,m.__file__,'exec'),m.__dict__)
class Trial(m.Lab):
    arm='stock';mask=0;round_id=-1;phase='environment'
    def record(self,kind,value):
        super().record(kind,{'arm':self.arm,'mask':self.mask,'round':self.round_id,'phase':self.phase,'diagnostic_only':self.arm!='stock','binary_sha256':sha['stock' if self.arm=='stock' else 'probe'],**value})
    def spawn(self,args,name,env=None):
        if name.startswith('core-'):
            name=f'core-r{self.round_id}-{self.arm}-{name}'
            env={**(env or os.environ),'ET_ISSUE4_MASK':str(self.mask),'ET_ISSUE4_GUARD':'ISOLATED_CI_ONLY'}
        return super().spawn(args,name,env)
    def snapshot(self,label,extra=()):
        return super().snapshot(f'r{self.round_id}-{self.arm}-{label}',extra)
    def stop(self,p):
        if p.poll() is not None:return
        escalated=False;start=time.monotonic()
        try:
            os.killpg(p.pid,signal.SIGTERM);p.wait(timeout=3)
        except ProcessLookupError:pass
        except m.sp.TimeoutExpired:
            escalated=True
            try:os.killpg(p.pid,signal.SIGKILL)
            except ProcessLookupError:pass
            p.wait(timeout=3)
        if any('easytier-core' in str(x) for x in p.args):
            self.record('core_stop',{'pid':p.pid,'kill_escalation':escalated,'wait_s':time.monotonic()-start,'exit':p.returncode})
    def idle(self,label):
        before=self.core_resources();start=time.monotonic();time.sleep(2);after=self.core_resources();dt=time.monotonic()-start
        cpu=sum(b['user_s']+b['system_s']-x['user_s']-x['system_s'] for x,b in zip(before,after))
        self.record('idle',{'before':before,'after':after,'elapsed_s':dt,'combined_core_equivalents':cpu/dt})
    def run(self):
        self.setup();self.iperfs['verified']=str(pathlib.Path(a.iperf).resolve())
        self.phase='native-tool-control'
        for reverse in [False,True]:self.iperf(version='verified',rate=100,reverse=reverse,duration=3)
        orders=[['stock','normal','scan','metrics','both'],['metrics','both','normal','scan'],['both','scan','metrics','normal','stock']]
        index=0
        for block,order in enumerate(orders):
            for arm in order:
                self.arm=arm;self.mask={'stock':0,'normal':0,'scan':1,'metrics':2,'both':3}[arm];self.round_id=index;index+=1
                d=stock if arm=='stock' else probe;self.core=d/'easytier-core';self.cli=d/'easytier-cli'
                for carrier in (['udp','tcp'] if block%2==0 else ['tcp','udp']):
                    self.phase='startup';self.start_mesh(carrier)
                    self.phase='integrity-eof';self.halfclose(0);self.halfclose(1)
                    self.phase='warmup';self.iperf(version='verified',rate=100,reverse=True,duration=1)
                    self.phase='fixed-100'
                    for reverse in [False,True]:self.iperf(version='verified',rate=100,reverse=reverse,duration=3)
                    self.phase='saturation'
                    name=f'ping-{index}-{carrier}'
                    ping=self.spawn(self.ns(0,['ping','-n','-i','0.05','-c','80','-w','6','10.89.0.2']),name)
                    self.iperf(version='verified',rate=0,reverse=True,duration=4)
                    rc=self.wait(ping,3);text=(self.out/(name+'.out')).read_text()
                    match=re.search(r'(\d+) packets transmitted, (\d+) received',text)
                    ok=rc==0 and match is not None and match.groups()==('80','80')
                    self.record('ping_progress',{'ok':ok,'exit':rc,'summary':text.splitlines()[-3:]});self.stop(ping)
                    if arm!='stock':
                        logs='\n'.join(x.read_text(errors='replace') for x in self.out.glob(f'core-r{self.round_id}-{self.arm}-core-{carrier}-*.err'))
                        assert f'initialized mask={self.mask}' in logs,'diagnostic was not reached'
                        for bit in [1,2]:
                            assert (f'executed bit={bit}' in logs)==bool(self.mask&bit),'unexpected intervention activation'
                        self.record('activation',{'ok':True,'mask':self.mask,'carrier':carrier})
        self.record('suite_finished',{'ok':not any(x.get('ok') is False for x in self.results)})
lab=Trial(str(stock),a.output,a.iperf)
def interrupted(sig,frame):raise RuntimeError(f'signal {sig}')
signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
try:lab.run()
except Exception as exc:lab.record('harness_failure',{'error':repr(exc)});raise
finally:lab.cleanup()
rows=lab.results;groups=[]
for mode in ['udp','tcp']:
 for phase in ['fixed-100','saturation']:
  for direction in ['0->1','1->0']:
   arms={}
   for arm in ['stock','normal','scan','metrics','both']:
    rs=[r for r in rows if r['kind']=='iperf' and r.get('ok') and r['mode']==mode and r['phase']==phase and r['payload_direction']==direction and r['arm']==arm]
    if rs:arms[arm]={'n':len(rs),**{k:[min(r[k] for r in rs),statistics.median(r[k] for r in rs),max(r[k] for r in rs)] for k in ['goodput_mbps','cpu_s_per_GiB']}}
   if arms:
    result={'carrier':mode,'phase':phase,'direction':direction,'arms':arms};groups.append(result);print('ISSUE4_CAUSAL_SUMMARY '+json.dumps(result),flush=True)
(pathlib.Path(a.output)/'causal-summary.json').write_text(json.dumps(groups,indent=2))
clean=next((r for r in reversed(rows) if r['kind']=='cleanup'),{})
valid=any(r['kind']=='suite_finished' for r in rows) and clean.get('root_routes_unchanged') and all(x['delete_exit']==0 and not x['residual_pids'] for x in clean.get('namespaces',[]))
sys.exit(0 if valid and not any(r.get('ok') is False or r['kind']=='harness_failure' for r in rows) else 1)
