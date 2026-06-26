use proptest::prelude::*;

use super::*;

fn arb_message() -> impl Strategy<Value = Message> {
    (
        "[0-9]{17,20}",
        "[0-9]{17,20}",
        any::<u64>(),
        "[a-zA-Z0-9_]{1,32}",
        ".*",
    )
        .prop_map(
            |(channel_id, message_id, timestamp, author, content)| Message {
                channel_id: ChannelId(channel_id),
                message_id: MessageId(message_id),
                timestamp,
                author,
                content,
            },
        )
}

fn arb_key() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 16..=64)
}

proptest! {
    #[test]
    fn sign_verify_roundtrip(msg in arb_message(), key in arb_key()) {
        let verifier = HmacVerifier::new(key).unwrap();
        let signed = verifier.sign(&msg);
        let recovered = verifier.verify(&signed).unwrap();
        prop_assert_eq!(recovered, msg);
    }

    #[test]
    fn tamper_channel_id(msg in arb_message(), key in arb_key(), garbage in "[0-9]{17,20}") {
        let verifier = HmacVerifier::new(key).unwrap();
        let mut signed = verifier.sign(&msg);
        signed.message.channel_id = ChannelId(garbage);
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_message_id(msg in arb_message(), key in arb_key(), garbage in "[0-9]{17,20}") {
        let verifier = HmacVerifier::new(key).unwrap();
        let mut signed = verifier.sign(&msg);
        signed.message.message_id = MessageId(garbage);
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_content(msg in arb_message(), key in arb_key(), garbage in ".*") {
        let verifier = HmacVerifier::new(key).unwrap();
        let mut signed = verifier.sign(&msg);
        signed.message.content = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_author(msg in arb_message(), key in arb_key(), garbage in "[a-zA-Z0-9_]{1,32}") {
        let verifier = HmacVerifier::new(key).unwrap();
        let mut signed = verifier.sign(&msg);
        signed.message.author = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_timestamp(msg in arb_message(), key in arb_key(), garbage in any::<u64>()) {
        let verifier = HmacVerifier::new(key).unwrap();
        let mut signed = verifier.sign(&msg);
        signed.message.timestamp = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn wrong_key(msg in arb_message(), key_a in arb_key(), key_b in arb_key()) {
        prop_assume!(key_a != key_b);
        let signer = HmacVerifier::new(key_a).unwrap();
        let checker = HmacVerifier::new(key_b).unwrap();
        let signed = signer.sign(&msg);
        prop_assert_eq!(checker.verify(&signed), Err(VerifyError::InvalidSignature));
    }

    #[test]
    fn deterministic(msg in arb_message(), key in arb_key()) {
        let verifier = HmacVerifier::new(key).unwrap();
        let a = verifier.sign(&msg);
        let b = verifier.sign(&msg);
        prop_assert_eq!(a.signature, b.signature);
    }
}

#[test]
fn empty_signature_is_missing() {
    let verifier = HmacVerifier::new(b"key".to_vec()).unwrap();
    let msg = Message {
        channel_id: ChannelId("123".into()),
        message_id: MessageId("456".into()),
        timestamp: 0,
        author: "test".into(),
        content: "hello".into(),
    };
    let signed = SignedMessage {
        message: msg,
        signature: vec![],
    };
    assert_eq!(verifier.verify(&signed), Err(VerifyError::MissingSignature));
}

#[test]
fn null_byte_collision_prevented() {
    let msg_a = Message {
        channel_id: ChannelId("a\0b".into()),
        message_id: MessageId("c".into()),
        timestamp: 0,
        author: "test".into(),
        content: "hello".into(),
    };
    let msg_b = Message {
        channel_id: ChannelId("a".into()),
        message_id: MessageId("b\0c".into()),
        timestamp: 0,
        author: "test".into(),
        content: "hello".into(),
    };
    assert_ne!(
        msg_a.canonical_bytes(),
        msg_b.canonical_bytes(),
        "length-prefixed serialization must prevent null byte boundary confusion"
    );
}

#[test]
fn empty_key_rejected() {
    let result = HmacVerifier::new(Vec::<u8>::new());
    assert_eq!(result.unwrap_err(), VerifyError::EmptyKey);
}

#[test]
fn truncated_signature_rejected() {
    let verifier = HmacVerifier::new(b"secret".to_vec()).unwrap();
    let msg = Message {
        channel_id: ChannelId("123".into()),
        message_id: MessageId("456".into()),
        timestamp: 42,
        author: "test".into(),
        content: "hello".into(),
    };
    let mut signed = verifier.sign(&msg);
    signed.signature.truncate(16);
    assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
}
