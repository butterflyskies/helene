mod types;
mod hmac_verifier;

pub use types::{ChannelId, Message, MessageId, SignedMessage};
pub use hmac_verifier::HmacVerifier;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("invalid signature")]
    InvalidSignature,

    #[error("missing signature")]
    MissingSignature,

    #[error("empty key")]
    EmptyKey,
}

pub trait MessageVerifier: Send + Sync {
    fn sign(&self, msg: &Message) -> SignedMessage;
    fn verify(&self, msg: &SignedMessage) -> Result<Message, VerifyError>;
}

#[cfg(test)]
mod tests;
