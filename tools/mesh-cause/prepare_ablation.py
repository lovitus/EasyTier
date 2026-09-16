"""Generate diagnostic-only interventions. Never a production repair."""
import pathlib, subprocess, sys
root=pathlib.Path(sys.argv[1])
if subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip()!='c6772dbfef2395ff96b39bd4801945d92212dffb':
    raise RuntimeError('wrong source')
p=root/'easytier/src/gateway/socks5.rs';s=p.read_text()
old='if entry_count == 0 && !socks5_enabled && self.entries.is_empty() {'
assert s.count(old)==1
p.write_text(s.replace(old,'if entry_count == 0 && !socks5_enabled && (crate::__issue4_ablate(1) || self.entries.is_empty()) {',1))
p=root/'easytier/src/peers/traffic_metrics.rs';s=p.read_text()
for direction in ['tx','rx']:
    old=f'    pub(crate) async fn record_{direction}(&self, peer_id: PeerId, packet_type: u8, bytes: u64) {{'
    assert s.count(old)==1
    s=s.replace(old,old+'\n        // DIAGNOSTIC ONLY: deliberately omits logical DATA telemetry, not security.\n        if matches!(traffic_kind(packet_type), TrafficKind::Data) && crate::__issue4_ablate(2) { return; }',1)
p.write_text(s)
p=root/'easytier/src/lib.rs';s=p.read_text()
assert '__issue4_ablate' not in s
s+='''
// EPHEMERAL CAUSAL EXPERIMENT. Never release or deploy this diagnostic API.
pub(crate) fn __issue4_ablate(bit: u8) -> bool {
    static MASK: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    static SCAN: std::sync::Once = std::sync::Once::new();
    static METRICS: std::sync::Once = std::sync::Once::new();
    let mask = *MASK.get_or_init(|| {
        assert_eq!(std::env::var("ET_ISSUE4_GUARD").as_deref(), Ok("ISOLATED_CI_ONLY"));
        let mask: u8 = std::env::var("ET_ISSUE4_MASK").expect("mask").parse().expect("integer mask");
        assert!(mask < 4);
        eprintln!("ISSUE4_ABLATION initialized mask={mask}; NOT A PRODUCTION BINARY");
        mask
    });
    if mask & bit == 0 { return false; }
    let first = if bit == 1 { &SCAN } else { &METRICS };
    first.call_once(|| eprintln!("ISSUE4_ABLATION executed bit={bit}; NOT A PRODUCTION FIX"));
    true
}
'''
p.write_text(s)
