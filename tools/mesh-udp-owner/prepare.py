#!/usr/bin/env python3
"""Exact-source runner-only UDP transmit ownership experiment. No production patch."""
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]); source=root/'easytier/src/tunnel/udp.rs'
s=source.read_text(); original=s
def replace(old,new,count=1):
 global s
 assert s.count(old)==count,(old[:100],s.count(old),count)
 s=s.replace(old,new)
replace('''            let close_event_sender = close_event_sender;
            let err =''','''            let close_event_sender = close_event_sender;
            if issue4_inline_tx::enabled() {
                // Retain original task/ring as dormant controls. Only packet handoff is removed.
                let _owned_unused_inputs = (ring_recv, close_event_sender);
                futures::future::pending::<()>().await;
                return;
            }
            let err =''')
replace('''            Box::new(RingSink::new(ring_for_send_udp)),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(self.local_url.clone().into()),''','''            issue4_inline_tx::make(ring_for_send_udp, socket.clone(), remote_addr,
                conn_id, conn_stealth.clone(), self.close_event_sender.clone()),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(self.local_url.clone().into()),''')
replace('''            ring_sender,
            ring_recv,
            close_event_sender,
        );''','''            ring_sender,
            ring_recv,
            close_event_sender.clone(),
        );''')
replace('''            Box::new(RingSink::new(ring_for_send_udp)),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(''','''            issue4_inline_tx::make(ring_for_send_udp, socket.clone(), dst_addr,
                conn_id, conn_stealth.clone(), close_event_sender),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(''')
s+='\n'+Path(__file__).with_name('inline_tx.rs.in').read_text()
source.write_text(s)
print('TXOWNER_SOURCE '+json.dumps({'base':'c6772dbfef2395ff96b39bd4801945d92212dffb',
 'original_sha256':hashlib.sha256(original.encode()).hexdigest(),
 'candidate_sha256':hashlib.sha256(s.encode()).hexdigest(),
 'scope':'outbound ring/task boundary only; diagnostic, not multi-peer safe; unchanged wire and crypto'}))
