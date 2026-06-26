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
        .prop_map(|(channel_id, message_id, timestamp, author, content)| Message {
            channel_id: ChannelId(channel_id),
            message_id: MessageId(message_id),
            timestamp,
            author,
            content,
        })
}

fn arb_key() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 16..=64)
}

proptest! {
    #[test]
    fn sign_verify_roundtrip(msg in arb_message(), key in arb_key()) {
        let verifier = HmacVerifier::new(key);
        let signed = verifier.sign(&msg);
        let recovered = verifier.verify(&signed).unwrap();
        prop_assert_eq!(recovered, msg);
    }

    #[test]
    fn tamper_channel_id(msg in arb_message(), key in arb_key(), garbage in "[0-9]{17,20}") {
        let verifier = HmacVerifier::new(key);
        let mut signed = verifier.sign(&msg);
        signed.message.channel_id = ChannelId(garbage);
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_message_id(msg in arb_message(), key in arb_key(), garbage in "[0-9]{17,20}") {
        let verifier = HmacVerifier::new(key);
        let mut signed = verifier.sign(&msg);
        signed.message.message_id = MessageId(garbage);
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_content(msg in arb_message(), key in arb_key(), garbage in ".*") {
        let verifier = HmacVerifier::new(key);
        let mut signed = verifier.sign(&msg);
        signed.message.content = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_author(msg in arb_message(), key in arb_key(), garbage in "[a-zA-Z0-9_]{1,32}") {
        let verifier = HmacVerifier::new(key);
        let mut signed = verifier.sign(&msg);
        signed.message.author = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn tamper_timestamp(msg in arb_message(), key in arb_key(), garbage in any::<u64>()) {
        let verifier = HmacVerifier::new(key);
        let mut signed = verifier.sign(&msg);
        signed.message.timestamp = garbage;
        if signed.message != msg {
            prop_assert_eq!(verifier.verify(&signed), Err(VerifyError::InvalidSignature));
        }
    }

    #[test]
    fn wrong_key(msg in arb_message(), key_a in arb_key(), key_b in arb_key()) {
        prop_assume!(key_a != key_b);
        let signer = HmacVerifier::new(key_a);
        let checker = HmacVerifier::new(key_b);
        let signed = signer.sign(&msg);
        prop_assert_eq!(checker.verify(&signed), Err(VerifyError::InvalidSignature));
    }

    #[test]
    fn deterministic(msg in arb_message(), key in arb_key()) {
        let verifier = HmacVerifier::new(key);
        let a = verifier.sign(&msg);
        let b = verifier.sign(&msg);
        prop_assert_eq!(a.signature, b.signature);
    }
}

#[test]
fn empty_signature_is_missing() {
    let verifier = HmacVerifier::new(b"key".to_vec());
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
