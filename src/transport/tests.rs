use proptest::prelude::*;

use super::mock::MockTransport;
use super::*;

fn arb_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=1024)
}

fn arb_envelope() -> impl Strategy<Value = Envelope> {
    (any::<u64>(), arb_payload()).prop_map(|(seq, payload)| Envelope { seq, payload })
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
            let (mut a, mut b) = MockTransport::pair();
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            a.send(&envelope).await.unwrap();
            let received = b.recv().await.unwrap();
            prop_assert_eq!(received, envelope);
            Ok(())
        })?;
    }

    #[test]
    fn message_ordering(envelopes in prop::collection::vec(arb_envelope(), 1..=32)) {
        let rt = rt();
        rt.block_on(async {
            let (mut a, mut b) = MockTransport::pair();
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
            let (mut a, mut b) = MockTransport::pair();
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
            let (mut a, mut b) = MockTransport::pair();
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            let envelope = Envelope { seq: 0, payload };
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
            let (mut a, mut b) = MockTransport::pair();
            a.connect().await.unwrap();
            b.connect().await.unwrap();

            let envelope = Envelope { seq, payload: vec![] };
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
        let (a, _b) = MockTransport::pair();
        let envelope = Envelope {
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
        let (_a, b) = MockTransport::pair();
        assert_eq!(b.recv().await, Err(TransportError::NotConnected));
    });
}

#[test]
fn double_connect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair();
        a.connect().await.unwrap();
        assert_eq!(a.connect().await, Err(TransportError::AlreadyConnected));
    });
}

#[test]
fn disconnect_without_connect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair();
        assert_eq!(a.disconnect().await, Err(TransportError::NotConnected));
    });
}

#[test]
fn send_after_disconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair();
        a.connect().await.unwrap();
        a.disconnect().await.unwrap();

        let envelope = Envelope {
            seq: 0,
            payload: vec![],
        };
        assert_eq!(a.send(&envelope).await, Err(TransportError::NotConnected));
    });
}

#[test]
fn connect_disconnect_reconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair();

        let id1 = a.connect().await.unwrap();
        assert!(a.is_connected());

        a.disconnect().await.unwrap();
        assert!(!a.is_connected());

        let id2 = a.connect().await.unwrap();
        assert!(a.is_connected());
        assert_eq!(id1, id2);

        // can still send/recv after reconnect
        b.connect().await.unwrap();
        let envelope = Envelope {
            seq: 42,
            payload: vec![0xFF],
        };
        a.send(&envelope).await.unwrap();
        let received = b.recv().await.unwrap();
        assert_eq!(received, envelope);
    });
}

#[test]
fn connection_closed_on_sender_drop() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair();
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // drop sender side — receiver should get ConnectionClosed
        drop(a);
        assert_eq!(b.recv().await, Err(TransportError::ConnectionClosed));
    });
}

#[test]
fn is_connected_reflects_state() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, _b) = MockTransport::pair();

        assert!(!a.is_connected());
        a.connect().await.unwrap();
        assert!(a.is_connected());
        a.disconnect().await.unwrap();
        assert!(!a.is_connected());
    });
}
