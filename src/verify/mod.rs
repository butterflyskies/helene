mod hmac_verifier;
mod types;

pub use hmac_verifier::HmacVerifier;
pub use types::{ChannelId, Message, MessageId, SignedMessage};

use thiserror::Error;

#[non_exhaustive]
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
