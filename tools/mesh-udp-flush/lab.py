#!/usr/bin/env python3
"""Exact Core experiment. No production hosts, configuration or releases."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import statistics
import subprocess
import sys
import time
from tun_trace import TunWriteTrace

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'core-packet-path-probe'))
from natural_cohort_lab import host_cpu_values


def echo(role,host='10.88.0.2'):
    with socket.socket(socket.AF_INET6 if ':' in host else socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(3)
        if role == 'server':
            sock.bind((host,35905))
            for _ in range(30):
                payload, address = sock.recvfrom(2048)
                sock.sendto(payload,address)
        else:
            for sequence in range(30):
                size=[1,64,1200][sequence%3]
                payload=sequence.to_bytes(4,'big')+bytes([sequence])*size
                sock.sendto(payload,(host,35905))
                reply,address=sock.recvfrom(2048)
                assert reply==payload and address[:2]==(host,35905)
            print(json.dumps({'ok':True,'datagrams':30,'sizes':[1,64,1200]}))


def run(args):
    assert 0 < args.paced_mbps <= 1000
    assert not (args.unpaced_probe and args.paced_mbps != 200), 'rate control cannot apply to unpaced probe'
    inner_host='fd88::2' if args.inner_ipv6 else '10.88.0.2'
    inner_target=f'[{inner_host}]:35902' if args.inner_ipv6 else f'{inner_host}:35902'
    mixed_target=f'[{inner_host}]:35906' if args.inner_ipv6 else f'{inner_host}:35906'
    out=Path(args.output).resolve();out.mkdir(parents=True,exist_ok=False)
    binaries={'stock':Path(args.stock).resolve(),'candidate':Path(args.candidate).resolve()}
    loadgen=Path(__file__).with_name('paced_probe.py').resolve()
    if args.unpaced_probe:
        loadgen=Path(args.unpaced_probe).resolve()
    load_command=[str(loadgen)] if args.unpaced_probe else [sys.executable,str(loadgen)]
    transfer_timeout=120 if args.unpaced_probe else 20
    integrity=Path(__file__).resolve().parents[1]/'core-packet-path-probe/natural_cohort_lab.py'
    names=['etfa'+str(os.getpid()),'etfb'+str(os.getpid())]
    children=[];created=[];rows=[];control_fds=[];tun_trace=None;capture_stderr={}
    def record(kind,**data):
        row={'kind':kind,**data};rows.append(row)
        with (out/'results.jsonl').open('a') as f:f.write(json.dumps(row)+'\n')
    def command(argv,ns=None,check=True,timeout=30):
        p=subprocess.run((['ip','netns','exec',ns] if ns else [])+list(map(str,argv)),
                         capture_output=True,text=True,timeout=timeout)
        if check and p.returncode:raise RuntimeError(f'{argv}: {p.returncode}: {p.stderr}')
        return p
    def spawn(argv,label,ns,env=None,pass_fds=(),wait_for=None):
        with (out/(label+'.log')).open('w') as log:
            p=subprocess.Popen(['ip','netns','exec',ns]+list(map(str,argv)),stdout=log,
                               stderr=subprocess.PIPE if wait_for else log,start_new_session=True,env=env,pass_fds=pass_fds)
        children.append(p)
        if wait_for:
            prefix=bytearray();capture_stderr[p.pid]=(out/(label+'.stderr'),prefix)
            deadline=time.monotonic()+10
            while wait_for.encode() not in prefix:
                remaining=deadline-time.monotonic()
                if remaining<=0 or not select.select([p.stderr],[],[],remaining)[0]:
                    raise RuntimeError(f'{label}: capture readiness timed out: {prefix.decode(errors="replace")}')
                chunk=os.read(p.stderr.fileno(),4096)
                if not chunk:raise RuntimeError(f'{label}: capture exited before readiness: {prefix.decode(errors="replace")}')
                prefix.extend(chunk)
                if len(prefix)>65536:raise RuntimeError(f'{label}: excessive readiness output')
        return p
    def perf_control(write_fd,read_fd,command_name):
        os.write(write_fd,(command_name+'\n').encode())
        readable,_,_=select.select([read_fd],[],[],30)
        if not readable:raise RuntimeError(f'perf {command_name} acknowledgement timed out')
        reply=os.read(read_fd,64)
        if reply.rstrip(b'\0\n')!=b'ack':raise RuntimeError(f'perf {command_name} unexpected acknowledgement {reply!r}')
    def stop(p):
        killed=False
        if p.poll() is None:
            os.killpg(p.pid,signal.SIGTERM)
            try:p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                killed=True;os.killpg(p.pid,signal.SIGKILL);p.wait(timeout=3)
        if p.pid in capture_stderr:
            path,prefix=capture_stderr.pop(p.pid)
            path.write_bytes(bytes(prefix)+p.stderr.read())
            p.stderr.close()
        return {'pid':p.pid,'exit':p.returncode,'killed':killed}
    def routes(label):
        value={family:command(['ip','-j',family,'route','show','table','all']).stdout for family in ['-4','-6']}
        (out/(label+'.json')).write_text(json.dumps(value,indent=2));return value
    def snapshot(processes):
        result=[]
        for p in processes:
            fields=Path(f'/proc/{p.pid}/stat').read_text().rsplit(')',1)[1].split()
            status=Path(f'/proc/{p.pid}/status').read_text().splitlines()
            high_water=next(int(line.split()[1])*1024 for line in status if line.startswith('VmHWM:'))
            result.append({'pid':p.pid,'start':int(fields[19]),'cpu':(int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK'),
                           'rss':int(fields[21])*os.sysconf('SC_PAGESIZE'),
                           'lifetime_rss_high_water_bytes':high_water})
        return result
    def host_cpu():
        with open('/proc/stat') as f:return host_cpu_values(f.readline(),os.sysconf('SC_CLK_TCK'))
    def network_counters(label):
        for i,ns in enumerate(names):
            counters=command(['sh','-c','for file in /proc/net/snmp /proc/net/snmp6 /proc/net/netstat; do printf "\\n### %s\\n" "$file"; cat "$file"; done'],ns)
            (out/f'{label}-network-{i}.txt').write_text(counters.stdout)
            links=command(['ip','-s','-j','link','show'],ns)
            (out/f'{label}-links-{i}.json').write_text(links.stdout)
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
    def parse_stats(metrics_dir):
        sink=[];writer=[];errors=[]
        files=sorted(metrics_dir.glob('*.json'))
        if not files:
            return sink,writer,[{'error':'no dedicated diagnostic metric files','directory':str(metrics_dir)}]
        for file in files:
            target = sink if file.name.startswith('SINK-') else writer if file.name.startswith('WRITER-') else None
            if target is None:
                errors.append({'error':'unexpected diagnostic metric filename','file':str(file)})
                continue
            try:
                target.append(json.loads(file.read_text()))
            except json.JSONDecodeError as error:
                errors.append({'error':str(error),'file':str(file)})
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
            disabled='0' if args.inner_ipv6 else '1'
            command(['sysctl','-qw',f'net.ipv6.conf.all.disable_ipv6={disabled}',f'net.ipv6.conf.default.disable_ipv6={disabled}'],ns)
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
        record('topology',endpoints=[{'role':role,'namespace':ns,'underlay':f'192.0.2.{i+1}',
                                     'overlay':f'10.88.0.{i+1}'}
                                    for i,(role,ns) in enumerate(zip(['client','server'],names))],
               scope='same GitHub runner, two network namespaces, veth; not physical-host/WAN evidence',
               profiled=args.profile,unpaced=bool(args.unpaced_probe),inner_ipv6=args.inner_ipv6,mixed_flow=args.mixed_flow,
               paced_mbps=None if args.unpaced_probe else args.paced_mbps)
        if args.unpaced_probe and not args.profile and not args.tun_trace and args.tun_head_capacity is None:
            for repeat in range(3):
                for direction in ['upload','download']:
                    label=f'direct-{repeat}-{direction}'
                    server=spawn(load_command+['server','--listen','192.0.2.2:35902','--sessions','1',
                                               '--timeout-seconds',str(transfer_timeout)],label,names[1])
                    time.sleep(.2)
                    host_before=host_cpu()
                    result=command(load_command+['client','--target','192.0.2.2:35902','--direction',direction,
                                                 '--bytes',str(args.transfer_bytes),'--timeout-seconds',
                                                 str(transfer_timeout)],names[0],check=False,timeout=transfer_timeout+10)
                    host_after=host_cpu()
                    record('direct_raw',repeat=repeat,direction=direction,exit=result.returncode,
                           stdout=result.stdout,stderr=result.stderr,host_before=host_before,host_after=host_after)
                    server.wait(timeout=4)
                    assert result.returncode==0 and server.returncode==0
                    data=json.loads(result.stdout)
                    assert data['ok'] and data['bytes']==args.transfer_bytes
                    record('direct_transfer',repeat=repeat,direction=direction,result=data)
        order=[('stock',False),('legacy',False),('stage',False),('gso',False),
               ('gso',False),('stage',False),('legacy',False),('stage',False),
               ('legacy',False),('gso',False),('stock',False),
               ('legacy',True),('stage',True),('gso',True)]
        if args.order:
            requested = args.order.split(',')
            invalid = set(requested) - {'stock', 'legacy', 'stage', 'gso'}
            assert not invalid, f'unknown diagnostic arms: {sorted(invalid)}'
            order = [(arm, args.stealth) for arm in requested]
        for round_id,(arm,stealth) in enumerate(order):
            path=binaries['stock' if arm=='stock' else 'candidate'];cli=path/'easytier-cli'
            cores=[]
            for i,ns in enumerate(names):
                cfg=out/f'r{round_id}-cfg-{i}';cfg.mkdir();home=cfg/'home';home.mkdir()
                metrics_dir=cfg/'issue4-flush-metrics';metrics_dir.mkdir()
                tun_metrics_dir=cfg/'issue4-tun-metrics'
                if args.tun_head_capacity is not None:tun_metrics_dir.mkdir()
                env={k:v for k,v in os.environ.items() if not k.startswith('ET_')}
                env.update({'HOME':str(home),'XDG_CONFIG_HOME':str(home/'config'),'RUST_LOG':'warn'})
                if arm!='stock':env.update({'ET_ISSUE4_FLUSH_MODE':arm,'ET_ISSUE4_EXPERIMENT':'ISOLATED_LAB_ONLY',
                                             'ET_ISSUE4_FLUSH_METRICS_DIR':str(metrics_dir)})
                if args.tun_head_capacity is not None:
                    env.update({'ET_ISSUE4_TUN_HEAD_CAPACITY':str(args.tun_head_capacity),
                                'ET_ISSUE4_TUN_METRICS_DIR':str(tun_metrics_dir)})
                if args.packet_trace:env['ET_ISSUE4_PACKET_TRACE']='1'
                argv=[path/args.core_name,'--config-dir',cfg,'--network-name','flush-lab',
                      '--network-secret','isolated-test-only','--ipv4',f'10.88.0.{i+1}',
                      '--listeners',f'udp://192.0.2.{i+1}:35904','--hostname',f'flush-{i}',
                      '--rpc-portal','127.0.0.1:35903','--mtu','1380','--dev-name','tun0',
                      '--disable-ipv6',str(not args.inner_ipv6).lower(),'--disable-p2p','true','--disable-upnp','true',
                      '--disable-encryption','false','--encryption-algorithm','aes-gcm',
                      '--compression','none','--accept-dns','false',
                      '--secure-mode',str(stealth).lower(),'--stealth-mode',str(stealth).lower()]
                if i==0:argv+=['--peers','udp://192.0.2.2:35904']
                if args.inner_ipv6:argv+=['--ipv6',f'fd88::{i+1}/64']
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
                if args.inner_ipv6:
                    addresses=json.loads(command(['ip','-j','-6','addr','show','dev','tun0'],ns).stdout)
                    assert any(a['local']==f'fd88::{i+1}' for dev in addresses for a in dev['addr_info'])
                    record('tun_ipv6',round=round_id,endpoint=i,state=addresses)
            captures=[]
            if args.icmp_capture:
                assert args.inner_ipv6,'ICMP sequence capture requires inner IPv6'
                for i,ns in enumerate(names):
                    captures.append(spawn(['tcpdump','--immediate-mode','-nn','-tt','-l','-s','160','-c','80',
                                           '-i','tun0','icmp6 and (ip6[40] == 128 or ip6[40] == 129)'],
                                          f'r{round_id}-{arm}-icmp-{i}',ns,wait_for='listening on tun0'))
            udp_server=spawn([sys.executable,__file__,'echo','server',inner_host],f'r{round_id}-udp-server',names[1])
            time.sleep(.2)
            response=command([sys.executable,__file__,'echo','client',inner_host],names[0]);udp_server.wait(timeout=4)
            assert udp_server.returncode==0
            record('udp_echo',round=round_id,arm=arm,stealth=stealth,result=json.loads(response.stdout))
            for capture in captures:
                assert capture.poll() is None,'ICMP capture exited before load'
            for direction in (['upload','download'] if round_id%2==0 else ['download','upload']):
                label=f'r{round_id}-{arm}-{direction}'
                server=spawn([sys.executable,integrity,'integrity','server',direction,inner_host],label+'-integrity',names[1])
                time.sleep(.2)
                result=command([sys.executable,integrity,'integrity','client',direction,inner_host],names[0]);server.wait(timeout=4)
                assert server.returncode==0
                record('integrity',round=round_id,arm=arm,stealth=stealth,direction=direction,result=json.loads(result.stdout))
                amount=args.transfer_bytes if args.unpaced_probe else (33554432 if stealth else 67108864)
                env={**os.environ,'ET_PACED_MBPS':str(args.paced_mbps)}
                profiler=None
                if args.profile:
                    ctl_read,ctl_write=os.pipe();ack_read,ack_write=os.pipe()
                    control_fds.extend([ctl_read,ctl_write,ack_read,ack_write])
                    profiler=spawn(['perf','record','--delay=-1',f'--control=fd:{ctl_read},{ack_write}',
                                    '-e','cpu-clock','-F','99','--call-graph','fp',
                                    '-p',','.join(str(p.pid) for p in cores),'-o',out/(label+'.perf.data')],
                                   label+'-perf',names[0],pass_fds=(ctl_read,ack_write))
                    for fd in [ctl_read,ack_write]:os.close(fd);control_fds.remove(fd)
                    perf_control(ctl_write,ack_read,'enable')
                    record('profile_ready',round=round_id,direction=direction,pids=[p.pid for p in cores])
                server=spawn(load_command+['server','--listen',inner_target,'--sessions','1','--timeout-seconds',str(transfer_timeout)],label+'-server',names[1],env)
                if args.mixed_flow:
                    mixed_server=spawn(load_command+['server','--listen',mixed_target,'--sessions','1','--timeout-seconds',str(transfer_timeout)],label+'-mixed-server',names[1],env)
                time.sleep(.2)
                ping=spawn(['ping']+(['-6'] if args.inner_ipv6 else [])+['-c','20','-i','0.1','-W','1',inner_host],label+'-ping',names[0])
                if args.network_counters:network_counters(label+'-before')
                before=snapshot(cores);host_before=host_cpu()
                if args.mixed_flow:
                    other_direction='download' if direction=='upload' else 'upload'
                    mixed_client=spawn(load_command+['client','--target',mixed_target,'--direction',other_direction,'--bytes',str(amount),'--timeout-seconds',str(transfer_timeout)],label+'-mixed-client',names[0],env)
                if args.tun_trace:
                    tun_trace=TunWriteTrace(out,label,cores)
                    tun_trace.start()
                result=command(['env',f'ET_PACED_MBPS={args.paced_mbps}']+load_command+['client','--target',inner_target,'--direction',direction,'--bytes',str(amount),'--timeout-seconds',str(transfer_timeout)],names[0],check=False,timeout=transfer_timeout+10)
                if args.mixed_flow:
                    mixed_client.wait(timeout=transfer_timeout+10);mixed_server.wait(timeout=4)
                    assert mixed_client.returncode==0 and mixed_server.returncode==0
                    mixed_data=json.loads((out/(label+'-mixed-client.log')).read_text())
                    assert mixed_data['ok'] and mixed_data['bytes']==amount
                    record('mixed_transfer',round=round_id,arm=arm,stealth=stealth,direction=other_direction,result=mixed_data)
                if tun_trace:
                    tun_trace.finish()
                    tun_trace.close()
                    tun_trace=None
                    record('tun_trace_complete',round=round_id,arm=arm,direction=direction,
                           scope='instrumented write counts only; not throughput acceptance')
                host_after=host_cpu();after=snapshot(cores)
                if args.network_counters:network_counters(label+'-after')
                if profiler:
                    perf_control(ctl_write,ack_read,'stop')
                    profiler.wait(timeout=10)
                    assert profiler.returncode==0,'perf recording failed'
                    for fd in [ctl_write,ack_read]:os.close(fd);control_fds.remove(fd)
                    samples=command(['perf','script','-i',out/(label+'.perf.data'),'-F','pid'],timeout=60)
                    (out/(label+'-sample-pids.txt')).write_text(samples.stdout)
                    counts={p.pid:sum(line.strip()==str(p.pid) for line in samples.stdout.splitlines()) for p in cores}
                    record('profile_samples',round=round_id,direction=direction,counts=counts)
                    assert all(counts.values()),'profile missing samples for a Core endpoint'
                    report=command(['perf','report','--stdio','--no-children','--sort','comm,pid,dso,symbol',
                                    '-i',out/(label+'.perf.data')],timeout=60)
                    (out/(label+'-perf-report.txt')).write_text(report.stdout)
                    callgraph=command(['perf','report','--stdio','--children','--sort','pid,symbol',
                                       '-g','graph,0.5,caller','-i',out/(label+'.perf.data')],timeout=60)
                    (out/(label+'-perf-callgraph.txt')).write_text(callgraph.stdout)
                (out/(label+'-client.json')).write_text(result.stdout)
                (out/(label+'-client.stderr')).write_text(result.stderr)
                record('raw_transfer',round=round_id,arm=arm,stealth=stealth,direction=direction,before=before,after=after,host_before=host_before,host_after=host_after,exit=result.returncode,stdout=result.stdout)
                server.wait(timeout=4);assert result.returncode==0 and server.returncode==0
                data=json.loads(result.stdout);assert data['ok'] and data['bytes']==amount
                if not args.unpaced_probe:assert data['rate_cap_mbps']==args.paced_mbps
                for first,last in zip(before,after):assert(first['pid'],first['start'])==(last['pid'],last['start'])
                ping.wait(timeout=5);text=(out/(label+'-ping.log')).read_text()
                loss=re.search(r'([0-9.]+)% packet loss',text)
                assert ping.returncode==0 and loss and float(loss[1])==0,'ICMP progress failed'
                record('transfer',round=round_id,arm=arm,stealth=stealth,direction=direction,result=data,profiled=args.profile,tun_traced=args.tun_trace,
                       mixed_flow=args.mixed_flow,inner_ipv6=args.inner_ipv6,
                       core_cpu_s_GiB=sum(y['cpu']-x['cpu'] for x,y in zip(before,after))/(amount*(2 if args.mixed_flow else 1)/1024**3),
                       host_cpu_s_GiB=(host_after['busy_seconds']-host_before['busy_seconds'])/(amount*(2 if args.mixed_flow else 1)/1024**3))
            for i,capture in enumerate(captures):
                capture.wait(timeout=5)
                stopped=stop(capture)
                capture_log=(out/f'r{round_id}-{arm}-icmp-{i}.stderr').read_text()
                packet_log=(out/f'r{round_id}-{arm}-icmp-{i}.log').read_text()
                observed=re.findall(r'ICMP6, echo (request|reply), id (\d+), seq (\d+)',packet_log)
                identifiers={ident for _,ident,_ in observed}
                assert len(observed)==80 and len(identifiers)==2,'ICMP capture missing records'
                for ident in identifiers:
                    for kind in ('request','reply'):
                        assert sorted(int(seq) for typ,key,seq in observed if typ==kind and key==ident)==list(range(1,21)),'ICMP capture sequence gap'
                record('icmp_capture',round=round_id,endpoint=i,stop=stopped,
                       scope='packet sequence diagnosis only; not throughput acceptance')
                assert stopped['exit']==0 and not stopped['killed']
                assert '0 packets dropped by kernel' in capture_log,'ICMP capture incomplete'
            peers(cli,round_id,'after')
            exits=[stop(p) for p in cores];record('core_stop',round=round_id,arm=arm,values=exits)
            assert all(x['exit']==0 and not x['killed'] for x in exits)
            if args.packet_trace:
                for i in range(2):
                    trace=(out/f'r{round_id}-{arm}-core-{i}.log').read_text()
                    assert 'ISSUE4_PACKET_TRACE_OVERFLOW' not in trace,'packet trace exceeded bound'
                    for stage in ('encrypted_tx','udp_rx','peer_rx','nic_enqueue'):
                        assert f'ISSUE4_PACKET stage={stage} ' in trace,('missing trace stage',stage)
            for i in range(2):
                log=out/f'r{round_id}-{arm}-core-{i}.log';metrics_dir=out/f'r{round_id}-cfg-{i}'/'issue4-flush-metrics'
                if args.tun_head_capacity is not None:
                    files=list((out/f'r{round_id}-cfg-{i}'/'issue4-tun-metrics').glob('*.json'))
                    assert len(files)==1,'missing/ambiguous TUN metrics'
                    stats=json.loads(files[0].read_text())
                    record('tun_capacity',round=round_id,endpoint=i,stats=stats)
                    assert stats['capacity']==args.tun_head_capacity and stats['completed']>0
                    assert stats['scratch_lost']==0
                    if args.tun_head_capacity==0:assert stats['promoted']==0
                    else:assert stats['scratch_capacity']==args.tun_head_capacity
                # Stock never loads the diagnostic overlay or its metrics env.
                # Its expected absence is the negative activation control.
                sink,writer,errors = ([],[],[]) if arm=='stock' else parse_stats(metrics_dir)
                record('activation',round=round_id,arm=arm,stealth=stealth,endpoint=i,sink=sink,writer=writer,
                       metrics_dir=str(metrics_dir),parse_errors=errors)
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
            try:
                if tun_trace:tun_trace.close()
            finally:
                cleanup=[stop(p) for p in reversed(children)]
            for fd in control_fds:os.close(fd)
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
    if len(sys.argv)>1 and sys.argv[1]=='echo':echo(sys.argv[2],sys.argv[3] if len(sys.argv)>3 else '10.88.0.2')
    else:
        parser=argparse.ArgumentParser()
        for name in ['stock','candidate','output']:parser.add_argument('--'+name,required=True)
        parser.add_argument('--core-name',default='easytier-core')
        parser.add_argument('--order',help='comma-separated diagnostic arms; defaults to the full interleaved matrix')
        parser.add_argument('--stealth',action='store_true',help='enable existing secure/Stealth configuration for explicitly selected arms')
        parser.add_argument('--inner-ipv6',action='store_true',help='use Core IPv6 configuration for inner application traffic; underlay remains IPv4')
        parser.add_argument('--mixed-flow',action='store_true',help='concurrent opposite-direction bulk flow with independent port and result check')
        parser.add_argument('--network-counters',action='store_true',help='record per-namespace protocol and link counters around transfers')
        parser.add_argument('--icmp-capture',action='store_true',help='bounded inner-IPv6 TUN sequence capture; not performance evidence')
        parser.add_argument('--packet-trace',action='store_true',help='requires isolated packet-trace overlay; diagnostic rates only')
        parser.add_argument('--unpaced-probe',help='existing compiled easytier-perf-probe; no Core rebuild required')
        parser.add_argument('--paced-mbps',type=int,default=200,help='diagnostic cap per flow; never replaces unpaced acceptance')
        parser.add_argument('--transfer-bytes',type=int,default=1073741824)
        parser.add_argument('--profile',action='store_true',help='separate diagnostic run; rates are not comparison evidence')
        parser.add_argument('--tun-trace',action='store_true',help='bounded write trace; rates are not comparison evidence')
        parser.add_argument('--tun-head-capacity',type=int,choices=[0,4096,8192],help='requires isolated TUN overlay; records per-endpoint metrics')
        run(parser.parse_args())
