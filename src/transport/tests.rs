use proptest::prelude::*;

use super::mock::MockTransport;
use super::*;

fn arb_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=1024)
}

fn arb_tenant_id() -> impl Strategy<Value = TenantId> {
    "[a-z0-9]{1,16}".prop_map(TenantId)
}

fn arb_envelope() -> impl Strategy<Value = Envelope> {
    (arb_tenant_id(), any::<u64>(), arb_payload()).prop_map(|(tenant_id, seq, payload)| Envelope {
        tenant_id,
        seq,
        payload,
    })
}

fn tid() -> TenantId {
    TenantId("test".into())
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// ── proptests ────────────────────────────────────────────────────

proptest! {
    #[test]
    fn send_recv_roundtrip(envelope in arb_envelope()) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair(tid());
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            a.send(&envelope).await.unwrap();
            let received = b.recv().await.unwrap();
            prop_assert_eq!(received, envelope);
            Ok(())
        })?;
    }

    #[test]
    fn channel_fifo_order(envelopes in prop::collection::vec(arb_envelope(), 1..=32)) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair(tid());
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            for env in &envelopes {
                a.send(env).await.unwrap();
            }
            for expected in &envelopes {
                let received = b.recv().await.unwrap();
                prop_assert_eq!(&received, expected);
            }
            Ok(())
        })?;
    }

    #[test]
    fn bidirectional(env_a in arb_envelope(), env_b in arb_envelope()) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair(tid());
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            a.send(&env_a).await.unwrap();
            b.send(&env_b).await.unwrap();

            let got_by_b = b.recv().await.unwrap();
            let got_by_a = a.recv().await.unwrap();

            prop_assert_eq!(got_by_b, env_a);
            prop_assert_eq!(got_by_a, env_b);
            Ok(())
        })?;
    }

    #[test]
    fn large_payload(payload in prop::collection::vec(any::<u8>(), 4096..=65536)) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair(tid());
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            let envelope = Envelope { tenant_id: tid(), seq: 0, payload };
            a.send(&envelope).await.unwrap();
            let received = b.recv().await.unwrap();
            prop_assert_eq!(received, envelope);
            Ok(())
        })?;
    }

    #[test]
    fn empty_payload(seq in any::<u64>()) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair(tid());
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            let envelope = Envelope { tenant_id: tid(), seq, payload: vec![] };
            a.send(&envelope).await.unwrap();
            let received = b.recv().await.unwrap();
            prop_assert_eq!(received, envelope);
            Ok(())
        })?;
    }
}

// ── lifecycle tests ──────────────────────────────────────────────

#[test]
fn send_before_connect() {
    let rt = rt();
    rt.block_on(async {
        let (a, _b) = MockTransport::pair(tid());
        let envelope = Envelope {
            tenant_id: tid(),
            seq: 0,
            payload: vec![1, 2, 3],
        };
        assert_eq!(a.send(&envelope).await, Err(TransportError::NotConnected));
    });
}

#[test]
fn recv_before_connect() {
    let rt = rt();
    rt.block_on(async {
        let (_a, b) = MockTransport::pair(tid());
        assert_eq!(b.recv().await, Err(TransportError::NotConnected));
    });
}

#[test]
fn double_connect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        assert_eq!(a.connect().await, Err(TransportError::AlreadyConnected));
    });
}

#[test]
fn disconnect_without_connect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair(tid());
        assert_eq!(a.disconnect().await, Err(TransportError::NotConnected));
    });
}

#[test]
fn send_after_disconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        a.disconnect().await.unwrap();

        let envelope = Envelope {
            tenant_id: tid(),
            seq: 0,
            payload: vec![],
        };
        assert_eq!(a.send(&envelope).await, Err(TransportError::NotConnected));
    });
}

#[test]
fn connect_disconnect_lifecycle() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair(tid());

        let id = a.connect().await.unwrap();
        assert!(a.is_connected());
        assert_eq!(id, ConnectionId("mock-a".into()));

        a.disconnect().await.unwrap();
        assert!(!a.is_connected());

        // cannot reconnect — channel was closed on disconnect
        assert_eq!(a.connect().await, Err(TransportError::ConnectionClosed));
    });
}

#[test]
fn connection_closed_on_sender_drop() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // drop sender side — receiver should get ConnectionClosed
        drop(a);
        assert_eq!(b.recv().await, Err(TransportError::ConnectionClosed));
    });
}

#[test]
fn recv_after_peer_disconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // peer disconnects — drops their tx, closing our rx channel
        a.disconnect().await.unwrap();
        assert_eq!(b.recv().await, Err(TransportError::ConnectionClosed));
    });
}

#[test]
fn recv_after_disconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // same-side disconnect — recv checks connected flag first
        a.disconnect().await.unwrap();
        assert_eq!(a.recv().await, Err(TransportError::NotConnected));
    });
}

#[test]
fn is_connected_reflects_state() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair(tid());

        assert!(!a.is_connected());
        a.connect().await.unwrap();
        assert!(a.is_connected());
        a.disconnect().await.unwrap();
        assert!(!a.is_connected());
    });
}

#[test]
fn concurrent_sends_all_arrive() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        let a = std::sync::Arc::new(a);
        let n = 50;
        let mut handles = Vec::new();
        for i in 0..n {
            let sender = a.clone();
            handles.push(tokio::spawn(async move {
                let env = Envelope {
                    tenant_id: TenantId("test".into()),
                    seq: i,
                    payload: vec![i as u8],
                };
                sender.send(&env).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let mut seqs: Vec<u64> = Vec::new();
        for _ in 0..n {
            seqs.push(b.recv().await.unwrap().seq);
        }
        seqs.sort();
        assert_eq!(seqs, (0..n).collect::<Vec<_>>());
    });
}
