use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub channel_id: ChannelId,
    pub message_id: MessageId,
    pub timestamp: u64,
    pub author: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct SignedMessage {
    pub message: Message,
    pub signature: Vec<u8>,
}

impl Message {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(self.channel_id.0.as_bytes());
        buf.push(0x00);
        buf.extend_from_slice(self.message_id.0.as_bytes());
        buf.push(0x00);
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.push(0x00);
        buf.extend_from_slice(self.author.as_bytes());
        buf.push(0x00);
        buf.extend_from_slice(self.content.as_bytes());
        buf
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
