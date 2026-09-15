#!/usr/bin/env python3
"""Issue #4 diagnostic fixture; never imported by production EasyTier."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess as sp
import sys
import threading
import time

N = 1048579
PATTERN = bytes(range(251))

def payload(n):
    return (PATTERN * ((n + 250) // 251))[:n]

def exact(sock, n):
    parts = bytearray()
    while len(parts) < n:
        b = sock.recv(min(65536, n - len(parts)))
        if not b:
            raise EOFError('premature EOF')
        parts.extend(b)
    return bytes(parts)

def fixture(kind, host, port, sessions, n):
    if kind == 'server':
        with socket.socket() as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.settimeout(12)
            listener.bind((host, port))
            listener.listen(128)
            print('READY', flush=True)
            for _ in range(sessions):
                conn, _ = listener.accept()
                with conn:
                    conn.settimeout(10)
                    size = struct.unpack('!Q', exact(conn, 8))[0]
                    if not 0 < size <= N:
                        raise ValueError('fixture size out of bounds')
                    data = bytearray()
                    while True:
                        b = conn.recv(65536)
                        if not b:
                            break
                        data.extend(b)
                        if len(data) > size:
                            raise ValueError('too many bytes')
                    if bytes(data) != payload(size):
                        raise ValueError('integrity failure')
                    conn.sendall(struct.pack('!Q', size) + hashlib.sha256(data).digest())
                    conn.shutdown(socket.SHUT_WR)
            print(json.dumps({'ok': True, 'sessions': sessions, 'eof': True}), flush=True)
    else:
        for _ in range(sessions):
            with socket.create_connection((host, port), 10) as conn:
                conn.settimeout(10)
                data = payload(n)
                conn.sendall(struct.pack('!Q', n) + data)
                conn.shutdown(socket.SHUT_WR)
                answer = exact(conn, 40)
                if answer != struct.pack('!Q', n) + hashlib.sha256(data).digest():
                    raise ValueError('result mismatch')
                if conn.recv(1) != b'':
                    raise ValueError('missing final EOF')
        print(json.dumps({'ok': True, 'sessions': sessions, 'bytes_each': n, 'eof': True}), flush=True)

class Lab:
    def __init__(self, bundle, output, old_iperf):
        if os.environ.get('GITHUB_ACTIONS') != 'true' or os.geteuid() != 0:
            raise RuntimeError('this fixture is restricted to root on an ephemeral GitHub Actions runner')
        self.out = Path(output).resolve()
        self.out.mkdir(parents=True, exist_ok=False)
        self.bundle = Path(bundle).resolve()
        self.core = self.bundle / 'easytier-core'
        self.cli = self.bundle / 'easytier-cli'
        self.iperfs = {'system': shutil.which('iperf3'), '3.9': str(Path(old_iperf).resolve())}
        self.names = [f'etci-{os.getpid()}-a', f'etci-{os.getpid()}-b']
        self.owned = []
        self.cores = []
        self.created = []
        self.results = []
        self.seq = 0
        self.mode = 'underlay'
        self.clock = os.sysconf('SC_CLK_TCK')
        self.abort = threading.Event()
        self.stop_monitor = threading.Event()
        self.root_routes = self.cmd(['ip', '-j', 'route', 'show', 'table', 'all']).stdout
        self.record('environment', {'uname': self.cmd(['uname', '-a']).stdout,
            'cpu': self.cmd(['lscpu']).stdout, 'iperf_versions': {k: self.cmd([v, '--version'], check=False).stdout for k,v in self.iperfs.items()},
            'binary_sha256': hashlib.sha256(self.core.read_bytes()).hexdigest(),
            'workflow_sha': os.getenv('GITHUB_SHA'), 'runner_os': os.getenv('RUNNER_OS'),
            'runner_image': os.getenv('ImageVersion'), 'core_source_sha': '391c191c3d8b477c3b7e8ef19e87ae3cba5c9504'})
        if hashlib.sha256(self.core.read_bytes()).hexdigest() != '95c5724367ce4ab5a33688f871cc1531c50d84e78389e0c77d4d17ea99b5dc71':
            raise RuntimeError('Core hash mismatch')

    def cmd(self, args, check=True, timeout=15):
        r = sp.run([str(x) for x in args], text=True, stdout=sp.PIPE, stderr=sp.PIPE, timeout=timeout)
        if check and r.returncode:
            raise RuntimeError(json.dumps({'command': args, 'exit': r.returncode, 'stderr': r.stderr}))
        return r

    def ns(self, i, args):
        return ['ip', 'netns', 'exec', self.names[i]] + [str(x) for x in args]

    def record(self, kind, value):
        row = {'kind': kind, 'mode': self.mode, **value}
        with (self.out / 'results.jsonl').open('a') as f:
            f.write(json.dumps(row) + '\n')
        self.results.append(row)
        print('ETCI_RESULT ' + json.dumps(row), flush=True)

    def spawn(self, args, name, env=None):
        out = (self.out / (name + '.out')).open('w')
        err = (self.out / (name + '.err')).open('w')
        p = sp.Popen([str(x) for x in args], stdout=out, stderr=err, stdin=sp.DEVNULL,
                     start_new_session=True, env=env)
        out.close(); err.close()
        self.owned.append(p)
        return p

    def stop(self, p):
        if p.poll() is None:
            try:
                os.killpg(p.pid, signal.SIGTERM)
                p.wait(timeout=3)
            except (ProcessLookupError, sp.TimeoutExpired):
                if p.poll() is None:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait(timeout=3)

    def resources(self, p):
        try:
            base = Path('/proc') / str(p.pid)
            s = (base / 'stat').read_text().rsplit(')', 1)[1].split()
            return {'pid': p.pid, 'user_s': int(s[11])/self.clock, 'system_s': int(s[12])/self.clock,
                'rss_kib': int(s[21])*os.sysconf('SC_PAGE_SIZE')//1024, 'threads': int(s[17]),
                'fds': len(list((base/'fd').iterdir())), 'minor_faults': int(s[7]), 'major_faults': int(s[9]),
                'start_ticks': int(s[19])}
        except (OSError, ValueError, IndexError):
            return {'pid': p.pid, 'unavailable': True}

    def core_resources(self):
        return [self.resources(p) for p in self.cores]

    def monitor(self):
        with (self.out/'resources.jsonl').open('a') as f:
            while not self.stop_monitor.wait(0.5):
                states = self.core_resources()
                f.write(json.dumps({'time_ns': time.monotonic_ns(), 'mode': self.mode, 'cores': states})+'\n')
                f.flush()
                if any(x.get('rss_kib',0)>524288 or x.get('threads',0)>256 or x.get('fds',0)>1024 for x in states):
                    self.abort.set()
                if any(p.stat().st_size > 20*1024*1024 for p in self.out.glob('core-*.*')):
                    self.abort.set()

    def wait(self, p, timeout):
        deadline = time.monotonic() + timeout
        while p.poll() is None:
            if self.abort.is_set():
                raise RuntimeError('resource safety limit exceeded')
            if time.monotonic() >= deadline:
                return 124
            time.sleep(.05)
        return p.returncode

    def setup(self):
        for ns in self.names:
            self.cmd(['ip','netns','add',ns]); self.created.append(ns)
            self.cmd(['ip','-n',ns,'link','set','lo','up'])
        a, b = f'eca{os.getpid()}', f'ecb{os.getpid()}'
        self.cmd(['ip','link','add',a,'type','veth','peer','name',b])
        for i, link in enumerate([a,b]):
            self.cmd(['ip','link','set',link,'netns',self.names[i]])
            self.cmd(['ip','-n',self.names[i],'link','set',link,'name','under0'])
            self.cmd(['ip','-n',self.names[i],'addr','add',f'192.0.2.{i+1}/30','dev','under0'])
            self.cmd(['ip','-n',self.names[i],'link','set','under0','mtu','1500','up'])
        self.thread = threading.Thread(target=self.monitor, daemon=True); self.thread.start()

    def snapshot(self, label, extra=()):
        data = {'cores': self.core_resources(), 'namespaces': []}
        for i in range(2):
            data['namespaces'].append({c: self.cmd(self.ns(i, cmd), check=False).stdout for c,cmd in [
                ('links',['ip','-j','-s','link']),('tcp',['ss','-tinpe']),('udp',['ss','-unap']),
                ('snmp',['cat','/proc/net/snmp']),('softnet',['cat','/proc/net/softnet_stat']),
                ('offload',['ethtool','-k','tun0'])]})
        data['tasks'] = []
        for p in list(self.cores)+list(extra):
            item = {'pid': p.pid}
            for fn in ['syscall','wchan','status','schedstat']:
                try: item[fn] = Path(f'/proc/{p.pid}/{fn}').read_text()
                except OSError as e: item[fn] = str(e)
            try: item['fds'] = {x.name: os.readlink(x) for x in Path(f'/proc/{p.pid}/fd').iterdir()}
            except OSError: pass
            data['tasks'].append(item)
        (self.out/(label+'.snapshot.json')).write_text(json.dumps(data,indent=2))

    def start_mesh(self, mode):
        for p in self.cores: self.stop(p)
        self.cores=[]; self.mode=mode
        for i in range(2):
            self.cmd(['ip','-n',self.names[i],'link','delete','tun0'],check=False)
        common=['--network-name',f'et-ci-{os.getpid()}-{mode}','--network-secret','ci-public-fixture-only',
                '--disable-ipv6','true','--disable-p2p','true','--disable-upnp','true','--accept-dns','false',
                '--dev-name','tun0','--mtu','1380']
        env={**os.environ,'RUST_LOG':'warn'}
        for i in [1,0]:
            args=common+['--ipv4',f'10.89.0.{i+1}','--hostname',f'ci-{i}',
                         '--instance-name',f'ci-{i}','--listeners',f'{mode}://0.0.0.0:{27100+i}']
            if i==0: args+=['--peers',f'{mode}://192.0.2.2:27101']
            self.cores.insert(0,self.spawn(self.ns(i,[self.core]+args),f'core-{mode}-{i}',env))
        deadline=time.monotonic()+35
        while time.monotonic()<deadline:
            if any(p.poll() is not None for p in self.cores): raise RuntimeError('Core exited during startup')
            if self.cmd(self.ns(0,['ping','-c','1','-W','1','10.89.0.2']),check=False).returncode==0: break
        else: raise RuntimeError('mesh readiness timeout')
        self.snapshot(mode+'-ready')
        for i in range(2):
            r=self.cmd(self.ns(i,[self.cli,'-p','127.0.0.1:15888','-o','json','peer']),check=False)
            (self.out/f'{mode}-peer-{i}.json').write_text(r.stdout)
        self.idle(mode)

    def idle(self,label):
        before=self.core_resources(); start=time.monotonic(); time.sleep(8); after=self.core_resources()
        elapsed=time.monotonic()-start
        cpu=sum(b['user_s']+b['system_s']-a['user_s']-a['system_s'] for a,b in zip(before,after))
        self.record('idle',{'label':label,'elapsed_s':elapsed,'before':before,'after':after,'combined_core_equivalents':cpu/elapsed})

    def destination(self,i):
        return f'192.0.2.{i+1}' if self.mode=='underlay' else f'10.89.0.{i+1}'

    def halfclose(self, sender, sessions=1, size=N):
        self.seq+=1; label=f'{self.seq:03}-{self.mode}-halfclose-{sender}-{sessions}'
        receiver=1-sender; target=self.destination(receiver); port=39101
        server=self.spawn(self.ns(receiver,[sys.executable,__file__,'fixture','server',target,str(port),str(sessions),str(size)]),label+'-server')
        deadline=time.monotonic()+3
        while time.monotonic()<deadline:
            if 'READY' in (self.out/(label+'-server.out')).read_text(): break
            if server.poll() is not None: break
            time.sleep(.05)
        client=self.spawn(self.ns(sender,[sys.executable,__file__,'fixture','client',target,str(port),str(sessions),str(size)]),label+'-client')
        cr=self.wait(client,25); sr=self.wait(server,3)
        self.record('halfclose',{'label':label,'payload_direction':f'{sender}->{receiver}','sessions':sessions,'bytes_each':size,
                                 'client_exit':cr,'server_exit':sr,'ok':cr==0 and sr==0})
        self.stop(client); self.stop(server)
        if cr or sr: self.snapshot(label+'-failure')

    def iperf(self, version='system', rate=100, reverse=False, client_host=0, parallel=1, repeat=0, udp=False, trace=False, duration=4):
        self.seq+=1
        label=f'{self.seq:03}-{self.mode}-iperf-{version}-{rate}-{client_host}-{int(reverse)}-p{parallel}-r{repeat}'
        server_host=1-client_host; target=self.destination(server_host); port=39102
        command=self.ns(server_host,[self.iperfs[version],'-s','-1','-B',target,'-p',str(port),'-J'])
        if trace: command=['strace','-ff','-qq','-ttt','-s','0','-e','trace=select,pselect6,poll,ppoll,setsockopt,getsockopt,fcntl,close','-o',str(self.out/(label+'-server.strace'))]+command
        server=self.spawn(command,label+'-server')
        deadline=time.monotonic()+4
        while time.monotonic()<deadline:
            r=self.cmd(self.ns(server_host,['ss','-H','-lnt','sport','=',str(port)]),check=False)
            if r.stdout.strip(): break
            if server.poll() is not None: break
            time.sleep(.05)
        args=[self.iperfs[version],'-c',target,'-p',str(port),'-t',str(duration),'-P',str(parallel),'-J','-b',str(rate)+'M' if rate else '0']
        if reverse: args+=['-R']
        if udp: args+=['-u','-l','1200']
        before=self.core_resources(); start=time.monotonic()
        client=self.spawn(self.ns(client_host,args),label+'-client')
        late=False; cutoff=duration+9
        while client.poll() is None and time.monotonic()-start<cutoff:
            if self.abort.is_set(): raise RuntimeError('resource safety limit exceeded')
            if not late and time.monotonic()-start>duration+3:
                self.snapshot(label+'-before-timeout',[server,client]); late=True
            time.sleep(.05)
        cr=client.returncode if client.poll() is not None else 124
        sr=self.wait(server,2) if cr!=124 else 124
        after=self.core_resources(); elapsed=time.monotonic()-start
        try: result=json.loads((self.out/(label+'-client.out')).read_text())
        except (ValueError,OSError): result={}
        rx=result.get('end',{}).get('sum_received',result.get('end',{}).get('sum',{}))
        cpu=sum(b['user_s']+b['system_s']-a['user_s']-a['system_s'] for a,b in zip(before,after))
        success=cr==0 and sr==0 and not result.get('error') and rx.get('bytes',0)>0
        row={'label':label,'version':version,'rate_mbps':rate,'client_host':client_host,'server_host':server_host,
            'reverse':reverse,'payload_direction':f'{server_host}->{client_host}' if reverse else f'{client_host}->{server_host}',
            'parallel':parallel,'app_udp':udp,'diagnostic_trace':trace,'client_exit':cr,'server_exit':sr,'ok':success,
            'gross_elapsed_s':elapsed,'cpu_s':cpu,'before':before,'after':after,'receiver':rx,'error':result.get('error')}
        if success and not trace:
            row['cpu_s_per_GiB']=cpu/(rx['bytes']/2**30)
            row['goodput_mbps']=rx.get('bits_per_second',0)/1e6
        self.record('iperf',row)
        self.stop(client); self.stop(server)
        return success

    def offload(self,on):
        setting='on' if on else 'off'
        for i in range(2):
            r=self.cmd(self.ns(i,['ethtool','-K','tun0','tso',setting,'gso',setting,'gro',setting]),check=False)
            self.record('kernel_feature_change',{'host':i,'setting':setting,'exit':r.returncode,'stderr':r.stderr})
        self.snapshot(self.mode+'-offload-'+setting)

    def run(self):
        self.setup()
        self.halfclose(0); self.halfclose(1)
        for rep in range(3):
            for reverse in [False,True]: self.iperf(rate=0,reverse=reverse,repeat=rep)
        for carrier in ['udp','tcp']:
            self.start_mesh(carrier)
            self.halfclose(0); self.halfclose(1)
            for rep in range(3):
                rates=[50,100,200,0] if rep%2==0 else [0,200,100,50]
                if carrier=='tcp': rates=[100,0]
                for rate in rates:
                    for reverse in [False,True]: self.iperf(rate=rate,reverse=reverse,repeat=rep)
            for host,rev in [(0,False),(0,True),(1,False),(1,True)]:
                self.iperf(version='3.9',rate=100,reverse=rev,client_host=host,trace=host==0 and not rev)
            self.offload(False)
            self.halfclose(0); self.halfclose(1)
            self.iperf(rate=100); self.iperf(rate=100,reverse=True)
            self.offload(True)
            self.iperf(rate=0,parallel=8)
            self.iperf(rate=50,udp=True); self.iperf(rate=50,reverse=True,udp=True)
            self.snapshot(carrier+'-before-churn')
            self.halfclose(0,sessions=100,size=65537)
            self.halfclose(1,sessions=100,size=65537)
            time.sleep(2); self.snapshot(carrier+'-after-churn')
            for i in range(2): self.cmd(self.ns(i,['tc','qdisc','add','dev','under0','root','netem','delay','5ms']))
            self.iperf(rate=100,reverse=True)
            for i in range(2): self.cmd(self.ns(i,['tc','qdisc','del','dev','under0','root']))
        self.record('suite_finished',{'test_failures':sum(x.get('ok') is False for x in self.results),'note':'Measured failures remain failures; suite completion is not an all-tests-pass claim.'})

    def cleanup(self):
        self.stop_monitor.set()
        if hasattr(self,'thread'): self.thread.join(timeout=2)
        for p in reversed(self.owned): self.stop(p)
        statuses=[]
        for ns in reversed(self.created):
            r=self.cmd(['ip','netns','pids',ns],check=False)
            residual=r.stdout.split()
            for pid in residual:
                try: os.kill(int(pid),signal.SIGKILL)
                except ProcessLookupError: pass
            d=self.cmd(['ip','netns','delete',ns],check=False)
            statuses.append({'namespace':ns,'residual_pids':residual,'delete_exit':d.returncode})
        routes=self.cmd(['ip','-j','route','show','table','all'],check=False).stdout
        self.record('cleanup',{'namespaces':statuses,'root_routes_unchanged':routes==self.root_routes})
        (self.out/'summary.json').write_text(json.dumps(self.results,indent=2))

if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='fixture':
        fixture(sys.argv[2],sys.argv[3],int(sys.argv[4]),int(sys.argv[5]),int(sys.argv[6]))
    else:
        p=argparse.ArgumentParser(); p.add_argument('--bundle',required=True); p.add_argument('--output',required=True); p.add_argument('--iperf39',required=True)
        a=p.parse_args(); lab=Lab(a.bundle,a.output,a.iperf39)
        def interrupted(sig, frame): raise RuntimeError(f'interrupted by {sig}')
        signal.signal(signal.SIGTERM,interrupted); signal.signal(signal.SIGINT,interrupted)
        try: lab.run()
        except Exception as e:
            lab.record('harness_failure',{'error':repr(e)}); raise
        finally: lab.cleanup()
