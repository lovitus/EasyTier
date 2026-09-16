#!/usr/bin/env python3
"""Exact Core experiment. No production hosts, configuration or releases."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import socket
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'core-packet-path-probe'))
from natural_cohort_lab import host_cpu_values


def echo(role):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(3)
        if role == 'server':
            sock.bind(('10.88.0.2',35905))
            for _ in range(30):
                payload, address = sock.recvfrom(2048)
                sock.sendto(payload,address)
        else:
            for sequence in range(30):
                size=[1,64,1200][sequence%3]
                payload=sequence.to_bytes(4,'big')+bytes([sequence])*size
                sock.sendto(payload,('10.88.0.2',35905))
                reply,address=sock.recvfrom(2048)
                assert reply==payload and address==('10.88.0.2',35905)
            print(json.dumps({'ok':True,'datagrams':30,'sizes':[1,64,1200]}))


def run(args):
    out=Path(args.output).resolve();out.mkdir(parents=True,exist_ok=False)
    binaries={'stock':Path(args.stock).resolve(),'candidate':Path(args.candidate).resolve()}
    loadgen=Path(__file__).with_name('paced_probe.py').resolve()
    integrity=Path(__file__).resolve().parents[1]/'core-packet-path-probe/natural_cohort_lab.py'
    names=['etfa'+str(os.getpid()),'etfb'+str(os.getpid())]
    children=[];created=[];rows=[]
    def record(kind,**data):
        row={'kind':kind,**data};rows.append(row)
        with (out/'results.jsonl').open('a') as f:f.write(json.dumps(row)+'\n')
    def command(argv,ns=None,check=True,timeout=30):
        p=subprocess.run((['ip','netns','exec',ns] if ns else [])+list(map(str,argv)),
                         capture_output=True,text=True,timeout=timeout)
        if check and p.returncode:raise RuntimeError(f'{argv}: {p.returncode}: {p.stderr}')
        return p
    def spawn(argv,label,ns,env=None):
        with (out/(label+'.log')).open('w') as log:
            p=subprocess.Popen(['ip','netns','exec',ns]+list(map(str,argv)),stdout=log,
                               stderr=log,start_new_session=True,env=env)
        children.append(p);return p
    def stop(p):
        killed=False
        if p.poll() is None:
            os.killpg(p.pid,signal.SIGTERM)
            try:p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                killed=True;os.killpg(p.pid,signal.SIGKILL);p.wait(timeout=3)
        return {'pid':p.pid,'exit':p.returncode,'killed':killed}
    def routes(label):
        value={family:command(['ip','-j',family,'route','show','table','all']).stdout for family in ['-4','-6']}
        (out/(label+'.json')).write_text(json.dumps(value,indent=2));return value
    def snapshot(processes):
        result=[]
        for p in processes:
            fields=Path(f'/proc/{p.pid}/stat').read_text().rsplit(')',1)[1].split()
            result.append({'pid':p.pid,'start':int(fields[19]),'cpu':(int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK'),
                           'rss':int(fields[21])*os.sysconf('SC_PAGESIZE')})
        return result
    def host_cpu():
        with open('/proc/stat') as f:return host_cpu_values(f.readline(),os.sysconf('SC_CLK_TCK'))
    def types(value):
        if isinstance(value,list):return [v for item in value for v in types(item)]
        if isinstance(value,dict):
            if value.get('tunnel_proto') not in [None,'','-']:return value['tunnel_proto'].lower().split(',')
            return [v for item in value.values() for v in types(item)]
        return []
    def peers(cli,round_id,label):
        for i,ns in enumerate(names):
            result=command([cli,'-p','127.0.0.1:35903','-o','json','peer','list'],ns)
            (out/f'r{round_id}-{label}-peers-{i}.json').write_text(result.stdout)
            actual=types(json.loads(result.stdout));assert actual and set(actual)=={'udp'},actual
            record('peer',round=round_id,endpoint=i,phase=label,actual=actual)
    def parse_stats(file):
        sink=[];writer=[];errors=[]
        for line in file.read_text(errors='replace').splitlines():
            target = sink if line.startswith('ISSUE4_FLUSH_SINK ') else writer if line.startswith('ISSUE4_FLUSH_WRITER ') else None
            if target is None:
                continue
            try:
                target.append(json.loads(line.split(' ',1)[1]))
            except json.JSONDecodeError as error:
                errors.append({'error':str(error),'raw_line':line})
        return sink,writer,errors
    def interrupted(sig,_frame):raise RuntimeError(f'signal {sig}')
    signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
    before_routes=routes('host-routes-before')
    try:
        record('binaries',values={k:{'core':hashlib.sha256((v/args.core_name).read_bytes()).hexdigest(),
                                     'cli':hashlib.sha256((v/'easytier-cli').read_bytes()).hexdigest()} for k,v in binaries.items()})
        for ns in names:
            command(['ip','netns','add',ns]);created.append(ns)
            command(['ip','-n',ns,'link','set','lo','up'])
            command(['sysctl','-qw','net.ipv6.conf.all.disable_ipv6=1','net.ipv6.conf.default.disable_ipv6=1'],ns)
        command(['ip','link','add','under0','netns',names[0],'type','veth','peer','name','under0','netns',names[1]])
        for i,ns in enumerate(names):
            command(['ip','-n',ns,'addr','add',f'192.0.2.{i+1}/30','dev','under0'])
            command(['ip','-n',ns,'link','set','under0','mtu','1500','up'])
            # Match all arms; do not rely on veth carrying an existing GSO skb.
            command(['ethtool','-K','under0','tx-udp-segmentation','off','tso','off','gro','on'],ns)
            features=command(['ethtool','-k','under0'],ns).stdout
            (out/f'underlay-features-{i}.txt').write_text(features)
            expected={'tcp-segmentation-offload':'off','tx-udp-segmentation':'off','generic-receive-offload':'on'}
            actual={k.strip():v.strip().split()[0] for line in features.splitlines() if ':' in line for k,v in [line.split(':',1)] if k.strip() in expected}
            assert actual==expected,actual
        order=[('stock',False),('legacy',False),('stage',False),('gso',False),
               ('gso',False),('stage',False),('legacy',False),('stage',False),
               ('legacy',False),('gso',False),('stock',False),
               ('legacy',True),('stage',True),('gso',True)]
        if args.order:
            requested = args.order.split(',')
            invalid = set(requested) - {'stock', 'legacy', 'stage', 'gso'}
            assert not invalid, f'unknown diagnostic arms: {sorted(invalid)}'
            order = [(arm, False) for arm in requested]
        for round_id,(arm,stealth) in enumerate(order):
            path=binaries['stock' if arm=='stock' else 'candidate'];cli=path/'easytier-cli'
            cores=[]
            for i,ns in enumerate(names):
                cfg=out/f'r{round_id}-cfg-{i}';cfg.mkdir();home=cfg/'home';home.mkdir()
                env={k:v for k,v in os.environ.items() if not k.startswith('ET_')}
                env.update({'HOME':str(home),'XDG_CONFIG_HOME':str(home/'config'),'RUST_LOG':'warn'})
                if arm!='stock':env.update({'ET_ISSUE4_FLUSH_MODE':arm,'ET_ISSUE4_EXPERIMENT':'ISOLATED_LAB_ONLY'})
                argv=[path/args.core_name,'--config-dir',cfg,'--network-name','flush-lab',
                      '--network-secret','isolated-test-only','--ipv4',f'10.88.0.{i+1}',
                      '--listeners',f'udp://192.0.2.{i+1}:35904','--hostname',f'flush-{i}',
                      '--rpc-portal','127.0.0.1:35903','--mtu','1380','--dev-name','tun0',
                      '--disable-ipv6','true','--disable-p2p','true','--disable-upnp','true',
                      '--disable-encryption','false','--encryption-algorithm','aes-gcm',
                      '--compression','none','--accept-dns','false',
                      '--secure-mode',str(stealth).lower(),'--stealth-mode',str(stealth).lower()]
                if i==0:argv+=['--peers','udp://192.0.2.2:35904']
                cores.append(spawn(argv,f'r{round_id}-{arm}-core-{i}',ns,env))
            (out/f'r{round_id}-{arm}-core-pids.json').write_text(
                json.dumps([core.pid for core in cores])
            )
            time.sleep(8)
            assert all(p.poll() is None for p in cores),'startup failure'
            peers(cli,round_id,'before')
            for i,ns in enumerate(names):
                link=json.loads(command(['ip','-j','-d','link','show','tun0'],ns).stdout)
                assert link and link[0]['mtu']==1360 and link[0]['linkinfo']['info_data']['vnet_hdr']
                record('tun',round=round_id,endpoint=i,state=link)
            udp_server=spawn([sys.executable,__file__,'echo','server'],f'r{round_id}-udp-server',names[1])
            time.sleep(.2)
            response=command([sys.executable,__file__,'echo','client'],names[0]);udp_server.wait(timeout=4)
            assert udp_server.returncode==0
            record('udp_echo',round=round_id,arm=arm,stealth=stealth,result=json.loads(response.stdout))
            for direction in (['upload','download'] if round_id%2==0 else ['download','upload']):
                label=f'r{round_id}-{arm}-{direction}'
                server=spawn([sys.executable,integrity,'integrity','server',direction],label+'-integrity',names[1])
                time.sleep(.2)
                result=command([sys.executable,integrity,'integrity','client',direction],names[0]);server.wait(timeout=4)
                assert server.returncode==0
                record('integrity',round=round_id,arm=arm,stealth=stealth,direction=direction,result=json.loads(result.stdout))
                amount=33554432 if stealth else 67108864
                env={**os.environ,'ET_PACED_MBPS':'200'}
                server=spawn([sys.executable,loadgen,'server','--listen','10.88.0.2:35902','--sessions','1','--timeout-seconds','20'],label+'-server',names[1],env)
                time.sleep(.2)
                ping=spawn(['ping','-c','20','-i','0.1','-W','1','10.88.0.2'],label+'-ping',names[0])
                before=snapshot(cores);host_before=host_cpu()
                result=command(['env','ET_PACED_MBPS=200',sys.executable,loadgen,'client','--target','10.88.0.2:35902','--direction',direction,'--bytes',str(amount),'--timeout-seconds','20'],names[0],check=False)
                host_after=host_cpu();after=snapshot(cores)
                (out/(label+'-client.json')).write_text(result.stdout)
                (out/(label+'-client.stderr')).write_text(result.stderr)
                record('raw_transfer',round=round_id,arm=arm,stealth=stealth,direction=direction,before=before,after=after,host_before=host_before,host_after=host_after,exit=result.returncode,stdout=result.stdout)
                server.wait(timeout=4);assert result.returncode==0 and server.returncode==0
                data=json.loads(result.stdout);assert data['ok'] and data['bytes']==amount and data['rate_cap_mbps']==200
                for first,last in zip(before,after):assert(first['pid'],first['start'])==(last['pid'],last['start'])
                ping.wait(timeout=5);text=(out/(label+'-ping.log')).read_text()
                loss=re.search(r'([0-9.]+)% packet loss',text)
                assert ping.returncode==0 and loss and float(loss[1])==0,'ICMP progress failed'
                record('transfer',round=round_id,arm=arm,stealth=stealth,direction=direction,result=data,
                       core_cpu_s_GiB=sum(y['cpu']-x['cpu'] for x,y in zip(before,after))/(amount/1024**3),
                       host_cpu_s_GiB=(host_after['busy_seconds']-host_before['busy_seconds'])/(amount/1024**3))
            peers(cli,round_id,'after')
            exits=[stop(p) for p in cores];record('core_stop',round=round_id,arm=arm,values=exits)
            assert all(x['exit']==0 and not x['killed'] for x in exits)
            for i in range(2):
                log=out/f'r{round_id}-{arm}-core-{i}.log';sink,writer,errors=parse_stats(log)
                record('activation',round=round_id,arm=arm,stealth=stealth,endpoint=i,sink=sink,writer=writer,parse_errors=errors)
                if arm=='stock':
                    assert not sink and not writer and not errors
                    continue
                assert f'ISSUE4_FLUSH_MODE {arm}' in log.read_text(errors='replace')
                assert all(x['mode']==arm for x in sink+writer)
                if writer:
                    assert any(x['packets']>0 for x in writer)
                if stealth and writer:assert any(x['stealth_enabled'] and x['outer_seen'] for x in writer),'Stealth outer phase not observed'
                if arm=='gso' and writer:
                    assert any(x['gso_calls']>0 and sum(x['batch_histogram'][2:])>0 for x in writer),'no actual Core GSO batch'
                else:assert all(x['gso_calls']==0 for x in writer)
                if errors or not sink or not writer:
                    record('observation_failure',round=round_id,arm=arm,endpoint=i,
                           missing_sink=not sink,missing_writer=not writer,parse_errors=errors)
        record('suite_complete',rounds=len(order))
    except Exception as error:
        record('failure',error=repr(error));raise
    finally:
        try:
            cleanup=[stop(p) for p in reversed(children)]
            for ns in reversed(created):
                remaining=command(['ip','netns','pids',ns],check=False).stdout.strip()
                result=command(['ip','netns','delete',ns],check=False)
                cleanup.append({'namespace':ns,'exit':result.returncode,'remaining':remaining})
            (out/'cleanup.json').write_text(json.dumps(cleanup,indent=2))
        finally:
            after_routes=routes('host-routes-after')
            record('root_routes',unchanged=before_routes==after_routes)
    assert before_routes==after_routes
    assert all(x['exit']==0 and not x.get('killed') and not x.get('remaining') for x in cleanup)
    summary=[]
    for arm in ['stock','legacy','stage','gso']:
        for direction in ['upload','download']:
            selected=[r for r in rows if r['kind']=='transfer' and r['arm']==arm and not r['stealth'] and r['direction']==direction]
            if not selected:
                continue
            summary.append({'arm':arm,'direction':direction,'n':len(selected),
                            'Mbps':statistics.median(r['result']['bits_per_second']/1e6 for r in selected),
                            'core_cpu_s_GiB':statistics.median(r['core_cpu_s_GiB'] for r in selected),
                            'host_cpu_s_GiB':statistics.median(r['host_cpu_s_GiB'] for r in selected)})
    observations_ok=not any(r['kind']=='observation_failure' for r in rows)
    (out/'summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps({'summary':summary,'observations_complete':observations_ok,
                      'scope':'diagnostic measurements; not acceptance when observation is incomplete'}))
    if not observations_ok:
        raise RuntimeError('observation incomplete; raw measurements retained, run remains FAIL')


if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='echo':echo(sys.argv[2])
    else:
        parser=argparse.ArgumentParser()
        for name in ['stock','candidate','output']:parser.add_argument('--'+name,required=True)
        parser.add_argument('--core-name',default='easytier-core')
        parser.add_argument('--order',help='comma-separated diagnostic arms; defaults to the full interleaved matrix')
        run(parser.parse_args())
