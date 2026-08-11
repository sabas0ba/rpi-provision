//! Small network value types with validation.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use crate::error::{Error, Result};

/// An IPv4 address with a prefix length, e.g. `192.168.1.50/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Cidr {
    pub address: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Cidr {
    pub fn parse(text: &str) -> Result<Self> {
        let (addr, prefix) = text.split_once('/').ok_or_else(|| {
            Error::new(format!(
                "`{text}` is missing a prefix length (expected e.g. 192.168.1.50/24)"
            ))
        })?;
        let address = Ipv4Addr::from_str(addr.trim())
            .map_err(|_| Error::new(format!("`{addr}` is not a valid IPv4 address")))?;
        let prefix: u8 = prefix
            .trim()
            .parse()
            .map_err(|_| Error::new(format!("`{prefix}` is not a valid prefix length")))?;
        if prefix > 32 {
            return Err(Error::new(format!("prefix length {prefix} exceeds 32")));
        }
        Ok(Self { address, prefix })
    }

    /// Network address implied by this CIDR.
    pub fn network(&self) -> Ipv4Addr {
        let mask = self.netmask_bits();
        Ipv4Addr::from(u32::from(self.address) & mask)
    }

    fn netmask_bits(&self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        }
    }

    /// True when `other` lies inside this CIDR's network.
    pub fn contains(&self, other: Ipv4Addr) -> bool {
        let mask = self.netmask_bits();
        (u32::from(other) & mask) == (u32::from(self.address) & mask)
    }
}

impl fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

/// A 48-bit MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub fn parse(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split([':', '-']).collect();
        if parts.len() != 6 {
            return Err(Error::new(format!("`{text}` is not a MAC address (expected six octets)")));
        }
        let mut bytes = [0u8; 6];
        for (slot, part) in bytes.iter_mut().zip(parts) {
            if part.len() != 2 {
                return Err(Error::new(format!(
                    "`{text}` has an octet that is not two hex digits"
                )));
            }
            *slot = u8::from_str_radix(part, 16)
                .map_err(|_| Error::new(format!("`{text}` contains a non-hexadecimal octet")))?;
        }
        Ok(Self(bytes))
    }

    /// Derive a stable, locally administered unicast MAC from arbitrary bytes.
    ///
    /// Bit 0 of the first octet (multicast) is cleared and bit 1
    /// (locally administered) is set, per IEEE 802.
    pub fn derive(seed: &[u8]) -> Self {
        let digest = crate::sha256::sha256(seed);
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&digest[..6]);
        bytes[0] = (bytes[0] & 0xfe) | 0x02;
        Self(bytes)
    }

    pub fn is_locally_administered(&self) -> bool {
        self.0[0] & 0x02 != 0
    }

    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        write!(f, "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", b[0], b[1], b[2], b[3], b[4], b[5])
    }
}

/// Derive a stable RFC 9562 version 8 UUID from arbitrary bytes.
///
/// NetworkManager only requires connection UUIDs to be unique and well
/// formed; deriving them from the specification keeps generated profiles
/// byte-for-byte reproducible.
pub fn derive_uuid(seed: &[u8]) -> String {
    let digest = crate::sha256::sha256(seed);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8 (custom)
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 9562 variant
    let hex = crate::sha256::to_hex(&bytes);
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Parse an IPv4 address with a spec-friendly error message.
pub fn parse_ipv4(text: &str) -> Result<Ipv4Addr> {
    Ipv4Addr::from_str(text.trim())
        .map_err(|_| Error::new(format!("`{text}` is not a valid IPv4 address")))
}

/// Validate a Linux network interface name.
pub fn validate_interface(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 15 {
        return Err(Error::new(format!(
            "`{name}` is not a valid interface name (1-15 characters)"
        )));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
        return Err(Error::new(format!(
            "`{name}` contains characters not allowed in an interface name"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cidr() {
        let cidr = Ipv4Cidr::parse("192.168.1.50/24").unwrap();
        assert_eq!(cidr.address, Ipv4Addr::new(192, 168, 1, 50));
        assert_eq!(cidr.prefix, 24);
        assert_eq!(cidr.network(), Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(cidr.to_string(), "192.168.1.50/24");
    }

    #[test]
    fn cidr_membership() {
        let cidr = Ipv4Cidr::parse("10.55.0.1/24").unwrap();
        assert!(cidr.contains(Ipv4Addr::new(10, 55, 0, 2)));
        assert!(!cidr.contains(Ipv4Addr::new(10, 55, 1, 2)));
    }

    #[test]
    fn rejects_bad_cidr() {
        assert!(Ipv4Cidr::parse("192.168.1.50").is_err());
        assert!(Ipv4Cidr::parse("192.168.1.500/24").is_err());
        assert!(Ipv4Cidr::parse("192.168.1.50/33").is_err());
    }

    #[test]
    fn parses_mac() {
        let mac = MacAddr::parse("02:1a:2b:3c:4d:5e").unwrap();
        assert_eq!(mac.to_string(), "02:1A:2B:3C:4D:5E");
        assert!(MacAddr::parse("02:1a:2b:3c:4d").is_err());
        assert!(MacAddr::parse("zz:1a:2b:3c:4d:5e").is_err());
    }

    #[test]
    fn derived_mac_is_locally_administered_unicast() {
        for seed in ["a", "b", "usb0", "gadget-host"] {
            let mac = MacAddr::derive(seed.as_bytes());
            assert!(mac.is_locally_administered(), "{mac}");
            assert!(!mac.is_multicast(), "{mac}");
        }
        assert_ne!(MacAddr::derive(b"a"), MacAddr::derive(b"b"));
        assert_eq!(MacAddr::derive(b"a"), MacAddr::derive(b"a"));
    }

    #[test]
    fn derived_uuid_is_well_formed() {
        let uuid = derive_uuid(b"eth0-static");
        assert_eq!(uuid.len(), 36);
        let fields: Vec<&str> = uuid.split('-').collect();
        assert_eq!(fields.iter().map(|f| f.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert_eq!(fields[2].as_bytes()[0], b'8', "version nibble");
        assert!(matches!(fields[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'), "variant nibble");
        assert_eq!(uuid, derive_uuid(b"eth0-static"));
    }

    #[test]
    fn validates_interface_names() {
        assert!(validate_interface("eth0").is_ok());
        assert!(validate_interface("").is_err());
        assert!(validate_interface("this-name-is-far-too-long").is_err());
        assert!(validate_interface("eth 0").is_err());
    }
}
