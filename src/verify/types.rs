use std::fmt;

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelId(pub String);

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub channel_id: ChannelId,
    pub message_id: MessageId,
    pub timestamp: u64,
    pub author: String,
    pub content: String,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SignedMessage {
    pub message: Message,
    pub signature: Vec<u8>,
}

impl Message {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        Self::write_length_prefixed(&mut buf, self.channel_id.0.as_bytes());
        Self::write_length_prefixed(&mut buf, self.message_id.0.as_bytes());
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        Self::write_length_prefixed(&mut buf, self.author.as_bytes());
        Self::write_length_prefixed(&mut buf, self.content.as_bytes());
        buf
    }

    fn write_length_prefixed(buf: &mut Vec<u8>, data: &[u8]) {
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
