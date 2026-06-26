use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::{Message, MessageVerifier, SignedMessage, VerifyError};

type HmacSha256 = Hmac<Sha256>;

pub struct HmacVerifier {
    key: Vec<u8>,
}

impl HmacVerifier {
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }

    fn compute_mac(&self, msg: &Message) -> Vec<u8> {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(&msg.canonical_bytes());
        mac.finalize().into_bytes().to_vec()
    }
}

impl MessageVerifier for HmacVerifier {
    fn sign(&self, msg: &Message) -> SignedMessage {
        let signature = self.compute_mac(msg);
        SignedMessage {
            message: msg.clone(),
            signature,
        }
    }

    fn verify(&self, msg: &SignedMessage) -> Result<Message, VerifyError> {
        if msg.signature.is_empty() {
            return Err(VerifyError::MissingSignature);
        }

        let expected = self.compute_mac(&msg.message);

        if expected.len() != msg.signature.len() {
            return Err(VerifyError::InvalidSignature);
        }

        if expected.ct_eq(&msg.signature).into() {
            Ok(msg.message.clone())
        } else {
            Err(VerifyError::InvalidSignature)
        }
    }
}
