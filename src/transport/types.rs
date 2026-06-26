use std::fmt;

/// Identifies an established transport connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub String);

/// Identifies a tenant within the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(pub String);

/// Transport-level message envelope.
///
/// Wraps an opaque payload with a sequence number for ordering
/// verification and gap detection by higher layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub tenant_id: TenantId,
    pub seq: u64,
    pub payload: Vec<u8>,
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
