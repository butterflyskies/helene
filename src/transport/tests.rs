use proptest::prelude::*;

use super::mock::MockTransport;
use super::*;

fn arb_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=1024)
}

fn arb_tenant_id() -> impl Strategy<Value = TenantId> {
    "[a-z0-9]{1,16}".prop_map(TenantId::from)
}

fn arb_envelope() -> impl Strategy<Value = Envelope> {
    (arb_tenant_id(), any::<u64>(), arb_payload()).prop_map(|(tenant_id, seq, payload)| Envelope {
        tenant_id,
        seq,
        payload,
    })
}

fn tid() -> TenantId {
    TenantId::from("test")
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
        assert_eq!(id, ConnectionId::from("mock-a"));

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
                    tenant_id: TenantId::from("test"),
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

// ── drain-after-disconnect test ──────────────────────────────────

#[test]
fn drain_after_disconnect() {
    let rt = rt();
    rt.block_on(async {
        let (mut a, mut b) = MockTransport::pair(tid());
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // Buffer several messages before disconnecting the sender
        let envelopes: Vec<Envelope> = (0..5)
            .map(|i| Envelope {
                tenant_id: tid(),
                seq: i,
                payload: vec![i as u8],
            })
            .collect();

        for env in &envelopes {
            a.send(env).await.unwrap();
        }

        // Sender disconnects — drops tx, but messages are already buffered
        a.disconnect().await.unwrap();

        // Receiver should still be able to drain all buffered messages
        for expected in &envelopes {
            let received = b.recv().await.unwrap();
            assert_eq!(&received, expected);
        }

        // After draining, the next recv should see the closed channel
        assert_eq!(b.recv().await, Err(TransportError::ConnectionClosed));
    });
}

// ── backpressure test ────────────────────────────────────────────

#[test]
fn backpressure_when_buffer_full() {
    let rt = rt();
    rt.block_on(async {
        // Use a tiny buffer to make backpressure observable
        let (mut a, mut b) = MockTransport::pair_with_buffer(tid(), 2);
        a.connect().await.unwrap();
        b.connect().await.unwrap();

        // Fill the buffer exactly
        for i in 0..2u64 {
            let env = Envelope {
                tenant_id: tid(),
                seq: i,
                payload: vec![i as u8],
            };
            a.send(&env).await.unwrap();
        }

        // The third send should block because the buffer is full.
        // Use try_send semantics via a timeout to verify backpressure
        // without actually deadlocking the test.
        let sender = std::sync::Arc::new(a);
        let sender_clone = sender.clone();
        let send_handle = tokio::spawn(async move {
            let env = Envelope {
                tenant_id: TenantId::from("test"),
                seq: 2,
                payload: vec![2],
            };
            sender_clone.send(&env).await
        });

        // Give the send a moment to (not) complete — it should be blocked
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !send_handle.is_finished(),
            "send should block when buffer is full"
        );

        // Drain one message to free a slot
        let received = b.recv().await.unwrap();
        assert_eq!(received.seq, 0);

        // Now the blocked send should complete
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), send_handle)
            .await
            .expect("send should unblock after drain")
            .expect("task should not panic");
        assert!(
            result.is_ok(),
            "send should succeed after buffer space freed"
        );

        // Drain remaining messages
        assert_eq!(b.recv().await.unwrap().seq, 1);
        assert_eq!(b.recv().await.unwrap().seq, 2);
    });
}

// ── mock transport accessors ─────────────────────────────────────

#[test]
fn mock_transport_tenant_id() {
    let tenant = TenantId::from("acme");
    let (a, b) = MockTransport::pair(tenant);
    assert_eq!(a.tenant_id().as_str(), "acme");
    assert_eq!(b.tenant_id().as_str(), "acme");
}

// ── newtype accessor tests ───────────────────────────────────────

#[test]
fn connection_id_accessors() {
    let id = ConnectionId::from("conn-42");
    assert_eq!(id.as_str(), "conn-42");
    assert_eq!(id.to_string(), "conn-42");

    let id_from_string = ConnectionId::from(String::from("conn-99"));
    assert_eq!(id_from_string.as_str(), "conn-99");
}

#[test]
fn tenant_id_accessors() {
    let id = TenantId::from("tenant-abc");
    assert_eq!(id.as_str(), "tenant-abc");
    assert_eq!(id.to_string(), "tenant-abc");

    let id_from_string = TenantId::from(String::from("tenant-xyz"));
    assert_eq!(id_from_string.as_str(), "tenant-xyz");
}

#[test]
fn envelope_tenant_id_accessor() {
    let env = Envelope {
        tenant_id: TenantId::from("t1"),
        seq: 42,
        payload: vec![1, 2, 3],
    };
    assert_eq!(env.tenant_id().as_str(), "t1");
}
