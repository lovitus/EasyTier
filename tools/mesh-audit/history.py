#!/usr/bin/env python3
"""Read-only, pinned history and source-footprint audit. No working tree edits."""
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

FORK = 'c6772dbfef2395ff96b39bd4801945d92212dffb'
UPSTREAM = '286e0f4a0d801637178d1e58def19e508c9109c2'
OUT = Path(sys.argv[1]); OUT.mkdir(parents=True, exist_ok=True)

def git(*args, check=True):
    result = subprocess.run(['git', *args], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=90)
    if check and result.returncode:
        raise RuntimeError(f'git {args}: {result.returncode}: {result.stderr}')
    return result.stdout

def emit(kind, value):
    print('ETVERIFY_' + kind + ' ' + json.dumps(value, separators=(',', ':')), flush=True)

base = git('merge-base', FORK, UPSTREAM).strip()
counts = git('rev-list', '--left-right', '--count', f'{FORK}...{UPSTREAM}').strip()
identity = {'fork':FORK, 'upstream':UPSTREAM, 'merge_base':base, 'left_right_commits':counts,
            'workflow_sha':git('rev-parse','HEAD').strip()}
(OUT/'identity.json').write_text(json.dumps(identity,indent=2)); emit('IDENTITY', identity)
records=[]
for line in git('log','--format=%H%x09%cs%x09%s',f'{base}..{FORK}').splitlines():
    sha, date, title = line.split('\t',2)
    records.append({'sha':sha,'date':date,'title':title})
(OUT/'fork-only-commits.json').write_text(json.dumps(records,indent=2))
(OUT/'upstream-only-log.txt').write_text(git('log','--format=%H %cs %s', f'{FORK}..{UPSTREAM}'))
pattern=re.compile(r'perf|throughput|regress|revert|rollback|roll back|scratch|batch|offload|alloc|zero.copy|hot.path|spin|CPU|wake|buffer|queue|window|profil',re.I)
selected=[]
for r in records:
    if not pattern.search(r['title']): continue
    files=git('diff-tree','--no-commit-id','--name-status','-r',r['sha']).splitlines()
    body=git('show','-s','--format=%B',r['sha'])
    item={**r,'files':files,'referenced_shas':re.findall(r'\b[0-9a-f]{40}\b',body)}
    selected.append(item)
    emit('COMMIT', {**r,'files':files[:12], 'file_count':len(files), 'referenced_shas':item['referenced_shas']})
(OUT/'performance-commit-ledger.json').write_text(json.dumps(selected,indent=2))
for label, regex in [('batching','sendmmsg|recvmmsg'),('gro','gro_scratch|GRO_SCRATCH'),
                     ('metrics','counters\\(\\)\\.clone|record_with_resolver'),('buffers','split_off|advance\\(')]:
    history=git('log', '--format=%H %cs %s', '-G',regex,FORK,'--','easytier/src','third_party/netstack-smoltcp/src')
    (OUT/f'{label}-code-history.txt').write_text(history)
    emit('CODE_HISTORY', {'topic':label,'lines':history.splitlines()[:45], 'truncated_in_log':len(history.splitlines())>45})
patchdir=OUT/'historical-patches'; patchdir.mkdir(exist_ok=True)
patches=[]
for r in selected:
    if re.search(r'revert|rollback|roll back|scratch|batch|offload|metric|profil|performance',r['title'],re.I):
        patch=git('show','--format=medium','--no-ext-diff',r['sha'],'--','easytier/src','third_party/netstack-smoltcp/src','easytier/docs/todo','docs/todo')
        content=patch.encode(); truncated=len(content)>2_000_000
        (patchdir/(r['sha']+'.patch')).write_bytes(content[:2_000_000])
        patches.append({'sha':r['sha'],'bytes':len(content),'truncated':truncated})
(OUT/'patch-index.json').write_text(json.dumps(patches,indent=2))
numstats=[]
for line in git('diff','--numstat','--no-renames',base,FORK).splitlines():
    add,delete,path=line.split('\t',2)
    numstats.append({'path':path,'add':int(add) if add.isdigit() else None,'delete':int(delete) if delete.isdigit() else None})
(OUT/'fork-numstat.json').write_text(json.dumps(numstats,indent=2))
source=[r for r in numstats if r['path'].endswith(('.rs','.go','.c','.h')) and r['add'] is not None]
emit('SOURCE_GROWTH', {'source_files_changed':len(source),'add':sum(r['add'] for r in source),
    'delete':sum(r['delete'] for r in source),'top':sorted(source,key=lambda r:r['add']+r['delete'],reverse=True)[:35],
    'qualification':'Text churn includes tests/generated/vendored code; it is not CPU attribution or proof of uselessness.'})
snapdir=OUT/'sources';snapdir.mkdir(exist_ok=True)
focus=['virtual_nic.rs','linux_tun_offload.rs','peer_manager.rs','traffic_metrics.rs','udp.rs','ring.rs','stats_manager.rs','packet_def.rs','tcp_proxy.rs','wrapped_tcp_proxy.rs','mihomo.rs','policy_proxy.rs']
for label,sha in [('base',base),('fork',FORK),('upstream',UPSTREAM)]:
    paths=git('ls-tree','-r','--name-only',sha).splitlines()
    rows=[]
    for path in paths:
        if Path(path).name not in focus or not '/src/' in path or path.startswith('third_party/'):continue
        text=git('show',f'{sha}:{path}')
        dest=snapdir/label/path;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_text(text)
        rows.append({'path':path,'lines':len(text.splitlines()),'bytes':len(text.encode()),'blob':git('rev-parse',f'{sha}:{path}').strip()})
    emit('FOCUS_FILES',{'snapshot':label,'sha':sha,'files':rows})
    (OUT/(label+'-focus.json')).write_text(json.dumps(rows,indent=2))
    for needle in ['sendmmsg','recvmmsg','gro_scratch','try_recv','payload_bytes','leaf-policy-proxy','hotpath-cpu']:
        found=git('grep','-n',needle,sha,'--','easytier/src','easytier-core/src',check=False)
        (OUT/(label+'-'+needle+'.txt')).write_text(found)
        if needle in ('sendmmsg','recvmmsg','gro_scratch'):
            emit('TOKEN_PRESENCE',{'snapshot':label,'token':needle,'matches':found.splitlines()[:20]})
for path in git('ls-tree','-r','--name-only',FORK).splitlines():
    if not path.endswith('.md') or '/todo/' not in path:continue
    text=git('show',f'{FORK}:{path}')
    if not re.search(r'REJECTED|FAILED_NO_REAL|rollback|rolled back|regression|回退|回滚|拒绝',text,re.I):continue
    if not re.search(r'perf|throughput|GRO|batch|性能|吞吐|CPU',text,re.I):continue
    dest=OUT/'decisions'/path;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_text(text)
    lines=text.splitlines()
    matches=[{'line':i+1,'text':s} for i,s in enumerate(lines) if re.search(r'REJECTED|FAILED_NO_REAL|rollback|rolled back|回退|回滚|拒绝|Status:',s,re.I)]
    emit('DECISION',{'path':path,'matches':matches[:12]})
for needle in ['record_with_resolver','nic_packet_filters','BoxNicPacketFilter','build_policy_runtime_with_fallback','build_policy_runtime','leaf-policy-proxy']:
    text=git('grep','-n',needle,FORK,'--','easytier/src','easytier/Cargo.toml',check=False)
    (OUT/('calls-'+needle+'.txt')).write_text(text)
    emit('CALLS',{'needle':needle,'matches':text.splitlines()[:24]})
manifest=[{'path':str(p.relative_to(OUT)),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(OUT.rglob('*')) if p.is_file()]
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
emit('DONE', {'reachable_fork_commits':len(records),'performance_subjects':len(selected),'files':len(manifest)})
