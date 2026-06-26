use std::fmt;

/// Identifies an established transport connection.
///
/// Opaque wrapper around a connection identifier string. Use [`ConnectionId::as_str`]
/// to inspect the value and the [`From`] impls to construct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId(String);

impl ConnectionId {
    /// Returns the connection identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ConnectionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ConnectionId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies a tenant within the system.
///
/// Opaque wrapper around a tenant identifier string. Use [`TenantId::as_str`]
/// to inspect the value and the [`From`] impls to construct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Returns the tenant identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for TenantId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TenantId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Transport-level message envelope.
///
/// Wraps an opaque payload with a sequence number for ordering
/// verification and gap detection by higher layers.
///
/// The `tenant_id` field is payload metadata — it identifies the originating
/// tenant for routing and accounting at higher layers. The transport itself
/// is tenant-agnostic and does not inspect or route based on this field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub tenant_id: TenantId,
    pub seq: u64,
    pub payload: Vec<u8>,
}

impl Envelope {
    /// Returns the tenant identifier for this envelope.
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
}
