//! The validated tenant identifier.
//!
//! [`TenantId`] is the key every per-tenant resource is routed by. It is
//! **parsed, never deserialized**: a tenant id arrives from the network (a
//! header, a path segment, a JWT claim) and is immediately used to pick a
//! database, a schema, a bucket prefix, or a cache namespace. Anything that can
//! reach a file path or a SQL identifier has to be validated once, at the edge,
//! by a single function — so there is deliberately **no `Deserialize` impl**:
//! a `TenantId` cannot appear inside a request body and skip validation.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Maximum length of a tenant id, in bytes.
///
/// 63 is the shortest of the practical downstream limits (DNS label, Postgres
/// identifier truncation at 63 bytes, S3 bucket-name segment).
pub const MAX_TENANT_ID_LEN: usize = 63;

/// A validated tenant identifier.
///
/// Shape: `[a-z0-9][a-z0-9._-]{0,62}` — lowercase ASCII alphanumerics, dots,
/// dashes and underscores, first character alphanumeric, at most
/// [`MAX_TENANT_ID_LEN`] bytes.
///
/// Cheap to clone (`Arc<str>`), usable as a map key (`Eq + Hash + Ord`).
///
/// # Examples
///
/// ```
/// use r2e_tenant::TenantId;
///
/// let id = TenantId::parse("acme-eu").unwrap();
/// assert_eq!(id.as_str(), "acme-eu");
///
/// // rejected: uppercase, traversal, empty, too long
/// assert!(TenantId::parse("Acme").is_err());
/// assert!(TenantId::parse("../etc/passwd").is_err());
/// assert!(TenantId::parse("").is_err());
/// assert!(TenantId::parse(&"a".repeat(64)).is_err());
/// ```
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TenantId(Arc<str>);

impl TenantId {
    /// Validate `raw` and build a tenant id.
    pub fn parse(raw: &str) -> Result<Self, InvalidTenantId> {
        Self::validate(raw)?;
        Ok(Self(Arc::from(raw)))
    }

    /// Validate an already-owned `String` without re-allocating the `str` data.
    pub fn parse_owned(raw: String) -> Result<Self, InvalidTenantId> {
        Self::validate(&raw)?;
        Ok(Self(Arc::from(raw)))
    }

    /// Build a tenant id from a literal, **panicking** on an invalid one.
    ///
    /// The ergonomic form for fixtures, `eager([..])` lists and other trusted
    /// literals, where threading a `Result` through is noise. It still validates:
    /// there is deliberately no unchecked constructor, because a safe public
    /// bypass would let a custom resolver hand `../shared` to a source that puts
    /// `tenant.as_str()` in a file path — the exact boundary this type exists to
    /// hold. A `'static` string is written by the programmer, so a bad one is a
    /// bug to fail on, not an error to handle.
    ///
    /// # Panics
    ///
    /// Panics when `raw` is not a valid tenant id. Use [`parse`](Self::parse)
    /// for anything that can come from a request.
    ///
    /// ```
    /// use r2e_tenant::TenantId;
    ///
    /// assert_eq!(TenantId::from_static("acme").as_str(), "acme");
    /// ```
    #[must_use]
    pub fn from_static(raw: &'static str) -> Self {
        match Self::validate(raw) {
            Ok(()) => Self(Arc::from(raw)),
            Err(err) => panic!("invalid tenant id literal {raw:?}: {err}"),
        }
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(raw: &str) -> Result<(), InvalidTenantId> {
        if raw.is_empty() {
            return Err(InvalidTenantId::Empty);
        }
        if raw.len() > MAX_TENANT_ID_LEN {
            return Err(InvalidTenantId::TooLong(raw.len()));
        }
        let mut chars = raw.chars();
        let first = chars.next().expect("non-empty");
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(InvalidTenantId::InvalidStart(first));
        }
        for ch in chars {
            let ok = ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '-' | '_');
            if !ok {
                return Err(InvalidTenantId::InvalidChar(ch));
            }
        }
        Ok(())
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TenantId({})", self.0)
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for TenantId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for TenantId {
    type Err = InvalidTenantId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for TenantId {
    type Error = InvalidTenantId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for TenantId {
    type Error = InvalidTenantId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_owned(value)
    }
}

// Serialize only: a tenant id is fine to *emit* (logs, responses, metrics) but
// must never be deserialized straight out of a payload — see the module docs.
impl serde::Serialize for TenantId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Why a string is not a valid [`TenantId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidTenantId {
    /// The value was empty.
    Empty,
    /// The value exceeded [`MAX_TENANT_ID_LEN`] bytes.
    TooLong(usize),
    /// The first character was not `[a-z0-9]`.
    InvalidStart(char),
    /// A later character was not `[a-z0-9._-]`.
    InvalidChar(char),
}

impl fmt::Display for InvalidTenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("tenant id is empty"),
            Self::TooLong(len) => write!(
                f,
                "tenant id is {len} bytes, the maximum is {MAX_TENANT_ID_LEN}"
            ),
            Self::InvalidStart(ch) => write!(
                f,
                "tenant id must start with a lowercase letter or digit, found {ch:?}"
            ),
            Self::InvalidChar(ch) => write!(
                f,
                "tenant id may only contain [a-z0-9._-], found {ch:?}"
            ),
        }
    }
}

impl std::error::Error for InvalidTenantId {}
