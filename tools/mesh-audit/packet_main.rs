#![allow(dead_code)]
mod original;
mod patched;
use bytes::BytesMut;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use zerocopy::AsBytes;
struct CountAlloc;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) { ALLOCS.fetch_add(1, Ordering::Relaxed); BYTES.fetch_add(l.size() as u64, Ordering::Relaxed); }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) { ALLOCS.fetch_add(1, Ordering::Relaxed); BYTES.fetch_add(l.size() as u64, Ordering::Relaxed); }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p,l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) { ALLOCS.fetch_add(1, Ordering::Relaxed); BYTES.fetch_add(n as u64, Ordering::Relaxed); }
        unsafe { System.realloc(p,l,n) }
    }
}
#[global_allocator]
static GLOBAL: CountAlloc = CountAlloc;
trait Case: Sized {
    fn make(n: usize, kind: usize, shared: bool, foreign: bool) -> (Self, Option<BytesMut>);
    fn extract(self, op: usize) -> BytesMut;
    fn converted_payload(self, kind: usize) -> Vec<u8>;
    fn malformed(n: usize, cap: usize, kind: usize, op: usize) -> BytesMut;
}
macro_rules! implement {
    ($m:ident) => {
        impl Case for $m::ZCPacket {
            fn make(n:usize, kind:usize, shared:bool, foreign:bool)->(Self,Option<BytesMut>) {
                let types=[$m::ZCPacketType::NIC,$m::ZCPacketType::TCP,$m::ZCPacketType::UDP,$m::ZCPacketType::WG,$m::ZCPacketType::DummyTunnel];
                let payload = if foreign {
                    let inner=Self::make(n,4,false,false).0.inner();
                    let hdr=$m::ForeignNetworkPacketHeader::new(19,"mesh-test");
                    let mut p=hdr.as_bytes().to_vec();p.extend_from_slice(b"mesh-test");p.extend_from_slice(&inner);p
                } else { (0..n).map(|i|(i%251) as u8).collect::<Vec<u8>>() };
                let off=types[kind].get_packet_offsets().payload_offset;
                let mut buf=BytesMut::with_capacity(off+payload.len()+32);
                buf.resize(off+payload.len(),0);buf[off..].copy_from_slice(&payload);
                let (buf,owner)=if shared {
                    let mut all=BytesMut::with_capacity(8+buf.len()+32);
                    all.extend_from_slice(&[0x5a;8]);all.extend_from_slice(&buf);
                    let suffix=all.split_off(8);(suffix,Some(all))
                } else {(buf,None)};
                let mut p=Self::new_from_buf(buf,types[kind]);
                p.fill_peer_manager_hdr(11,19,if foreign {$m::PacketType::ForeignNetworkPacket as u8} else {$m::PacketType::Data as u8});
                (p,owner)
            }
            fn extract(self, op:usize)->BytesMut {
                match op {0=>self.payload_bytes(),1=>self.tunnel_payload_bytes(),2=>self.convert_type($m::ZCPacketType::UDP).inner(),3=>self.foreign_network_packet().inner(),_=>unreachable!()}
            }
            fn converted_payload(self,kind:usize)->Vec<u8> {
                let targets=[$m::ZCPacketType::TCP,$m::ZCPacketType::UDP,$m::ZCPacketType::WG,$m::ZCPacketType::DummyTunnel];
                self.convert_type(targets[kind]).tunnel_payload().to_vec()
            }
            fn malformed(n:usize,cap:usize,kind:usize,op:usize)->BytesMut {
                let types=[$m::ZCPacketType::NIC,$m::ZCPacketType::TCP,$m::ZCPacketType::UDP,$m::ZCPacketType::WG,$m::ZCPacketType::DummyTunnel];
                let mut buf=BytesMut::with_capacity(cap);buf.resize(n,0);
                let p=Self::new_from_buf(buf,types[kind]);
                if op==0 {p.payload_bytes()}else{p.tunnel_payload_bytes()}
            }
        }
    }
}
implement!(original);
implement!(patched);
fn check_equivalence() {
    let mut cases=0;
    for n in [0,1,64,1280,1360,1500,4096,65535] { for kind in 0..5 { for shared in [false,true] {
        for op in [0,1,3] {
            let (a,owner_a)=original::ZCPacket::make(n,kind,shared,op==3);
            let (b,owner_b)=patched::ZCPacket::make(n,kind,shared,op==3);
            assert_eq!(a.extract(op),b.extract(op));
            if shared {assert_eq!(&owner_a.unwrap()[..],&[0x5a;8]);assert_eq!(&owner_b.unwrap()[..],&[0x5a;8]);}
            cases+=1;
        }
        for target in 0..4 {
            let (a,_oa)=original::ZCPacket::make(n,kind,shared,false);
            let (b,_ob)=patched::ZCPacket::make(n,kind,shared,false);
            assert_eq!(a.converted_payload(target),b.converted_payload(target));cases+=1;
        }
    }}}
    let hook=std::panic::take_hook();std::panic::set_hook(Box::new(|_|{}));
    let mut malformed_cases=0;
    for n in [0,1,3,4,8,20,40] {for cap in [0,8,32,128] {for kind in 0..5 {for op in 0..2 {
        let a=std::panic::catch_unwind(||original::ZCPacket::malformed(n,cap,kind,op));
        let b=std::panic::catch_unwind(||patched::ZCPacket::malformed(n,cap,kind,op));
        assert_eq!(a.is_ok(),b.is_ok(),"malformed outcome len={n} cap={cap} kind={kind} op={op}");
        if let (Ok(a),Ok(b))=(a,b) {assert_eq!(a,b);assert_eq!(a.capacity(),b.capacity());}
        malformed_cases+=1;
    }}}}
    std::panic::set_hook(hook);
    println!("ETVERIFY_EQUIVALENCE {{\"valid_cases\":{cases},\"malformed_cases\":{malformed_cases},\"ok\":true}}");
}
fn bench<C: Case>(label:&str,n:usize,shared:bool,op:usize,round:usize) {
    const BATCH: usize=2048; const REPEATS: usize=8;
    let mut elapsed=0u128;let mut allocs=0u64;let mut allocated=0u64;
    for _ in 0..REPEATS {
        let input=(0..BATCH).map(|_|C::make(n,0,shared,op==3)).collect::<Vec<_>>();
        let mut output=Vec::with_capacity(BATCH);
        ALLOCS.store(0,Ordering::Relaxed);BYTES.store(0,Ordering::Relaxed);
        ACTIVE.store(true,Ordering::Relaxed);let start=Instant::now();
        for (p,owner) in input {output.push((black_box(p).extract(op),owner));}
        elapsed+=start.elapsed().as_nanos();ACTIVE.store(false,Ordering::Relaxed);
        allocs+=ALLOCS.load(Ordering::Relaxed);allocated+=BYTES.load(Ordering::Relaxed);
        black_box(&output);drop(output);
    }
    let count=BATCH*REPEATS;
    println!("ETVERIFY_BENCH {{\"variant\":\"{label}\",\"payload\":{n},\"shared\":{shared},\"op\":{op},\"round\":{round},\"samples\":{count},\"ns_each\":{},\"allocations_each\":{},\"allocated_bytes_each\":{}}}",elapsed as f64/count as f64,allocs as f64/count as f64,allocated as f64/count as f64);
}
fn main() {
    check_equivalence();
    for round in 0..7 {for n in [64,1280,4096] {for shared in [false,true] {for op in 0..4 {
        if round%2==0 {bench::<original::ZCPacket>("baseline",n,shared,op,round);bench::<patched::ZCPacket>("candidate",n,shared,op,round);}
        else {bench::<patched::ZCPacket>("candidate",n,shared,op,round);bench::<original::ZCPacket>("baseline",n,shared,op,round);}
    }}}}
}
#[cfg(test)]
mod audit_tests { #[test] fn equivalence(){super::check_equivalence();} }
