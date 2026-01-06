//! Security tests for vault-pvm networking
//!
//! Allowlist model: only global unicast addresses are permitted

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Clone)]
struct NetworkPolicy {
    allowed_ports: Vec<u16>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            allowed_ports: vec![80, 443, 8080, 8443],
        }
    }
}

impl NetworkPolicy {
    fn is_addr_allowed(&self, addr: &SocketAddr) -> Result<(), &'static str> {
        if !self.allowed_ports.is_empty() && !self.allowed_ports.contains(&addr.port()) {
            return Err("port not in allowlist");
        }

        match addr.ip() {
            IpAddr::V4(v4) => self.check_ipv4(v4),
            IpAddr::V6(v6) => self.check_ipv6(v6),
        }
    }

    fn check_ipv4(&self, ip: Ipv4Addr) -> Result<(), &'static str> {
        let o = ip.octets();

        if o[0] == 0
            || o[0] == 10
            || o[0] == 127
            || (o[0] == 100 && o[1] >= 64 && o[1] <= 127)
            || (o[0] == 169 && o[1] == 254)
            || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)
            || (o[0] == 192 && o[1] == 0 && o[2] == 2)
            || (o[0] == 192 && o[1] == 88 && o[2] == 99)
            || (o[0] == 192 && o[1] == 168)
            || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
            || (o[0] == 198 && o[1] == 51 && o[2] == 100)
            || (o[0] == 203 && o[1] == 0 && o[2] == 113)
            || o[0] >= 224
        {
            return Err("non-global ipv4 blocked");
        }

        Ok(())
    }

    fn check_ipv6(&self, ip: Ipv6Addr) -> Result<(), &'static str> {
        let segments = ip.segments();

        // 6to4 - extract embedded IPv4
        if segments[0] == 0x2002 {
            let embedded = Ipv4Addr::new(
                (segments[1] >> 8) as u8,
                (segments[1] & 0xff) as u8,
                (segments[2] >> 8) as u8,
                (segments[2] & 0xff) as u8,
            );
            return self.check_ipv4(embedded);
        }

        // teredo - extract embedded IPv4 (XOR'd)
        if segments[0] == 0x2001 && segments[1] == 0x0000 {
            let obfuscated = ((segments[6] as u32) << 16) | (segments[7] as u32);
            let embedded = Ipv4Addr::from(obfuscated ^ 0xffffffff);
            return self.check_ipv4(embedded);
        }

        // only allow global unicast (2000::/3)
        if (segments[0] & 0xe000) != 0x2000 {
            return Err("non-global ipv6 blocked");
        }

        Ok(())
    }
}

// === BLOCKED: non-global IPv4 ===

#[test]
fn test_blocks_loopback() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"127.0.0.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"[::1]:80".parse().unwrap()).is_err());
}

#[test]
fn test_blocks_private_networks() {
    let policy = NetworkPolicy::default();
    // 10.0.0.0/8
    assert!(policy.is_addr_allowed(&"10.0.0.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"10.255.255.255:80".parse().unwrap()).is_err());
    // 172.16.0.0/12
    assert!(policy.is_addr_allowed(&"172.16.0.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"172.31.255.255:80".parse().unwrap()).is_err());
    // 192.168.0.0/16
    assert!(policy.is_addr_allowed(&"192.168.0.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"192.168.255.255:80".parse().unwrap()).is_err());
}

#[test]
fn test_blocks_link_local() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"169.254.1.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"169.254.169.254:80".parse().unwrap()).is_err()); // metadata
}

#[test]
fn test_blocks_cgnat() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"100.64.0.1:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"100.100.100.200:80".parse().unwrap()).is_err()); // alibaba metadata
    assert!(policy.is_addr_allowed(&"100.127.255.255:80".parse().unwrap()).is_err());
}

#[test]
fn test_blocks_reserved_ranges() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"0.0.0.1:80".parse().unwrap()).is_err());      // current network
    assert!(policy.is_addr_allowed(&"192.0.0.1:80".parse().unwrap()).is_err());    // IETF protocol
    assert!(policy.is_addr_allowed(&"192.0.2.1:80".parse().unwrap()).is_err());    // TEST-NET-1
    assert!(policy.is_addr_allowed(&"192.88.99.1:80".parse().unwrap()).is_err());  // 6to4 anycast
    assert!(policy.is_addr_allowed(&"198.18.0.1:80".parse().unwrap()).is_err());   // benchmarking
    assert!(policy.is_addr_allowed(&"198.51.100.1:80".parse().unwrap()).is_err()); // TEST-NET-2
    assert!(policy.is_addr_allowed(&"203.0.113.1:80".parse().unwrap()).is_err());  // TEST-NET-3
    assert!(policy.is_addr_allowed(&"224.0.0.1:80".parse().unwrap()).is_err());    // multicast
    assert!(policy.is_addr_allowed(&"255.255.255.255:80".parse().unwrap()).is_err()); // broadcast
}

// === BLOCKED: non-global IPv6 ===

#[test]
fn test_blocks_ipv6_non_global() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"[::1]:80".parse().unwrap()).is_err());        // loopback
    assert!(policy.is_addr_allowed(&"[fe80::1]:80".parse().unwrap()).is_err());    // link-local
    assert!(policy.is_addr_allowed(&"[fc00::1]:80".parse().unwrap()).is_err());    // unique local
    assert!(policy.is_addr_allowed(&"[fd00::1]:80".parse().unwrap()).is_err());    // unique local
    assert!(policy.is_addr_allowed(&"[fec0::1]:80".parse().unwrap()).is_err());    // site-local
    assert!(policy.is_addr_allowed(&"[ff02::1]:80".parse().unwrap()).is_err());    // multicast
}

#[test]
fn test_blocks_ipv4_mapped() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"[::ffff:127.0.0.1]:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"[::ffff:10.0.0.1]:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"[::ffff:192.168.1.1]:80".parse().unwrap()).is_err());
}

#[test]
fn test_blocks_ipv4_compatible() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"[::10.0.0.1]:80".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"[::127.0.0.1]:80".parse().unwrap()).is_err());
}

// === BLOCKED: tunnel bypasses ===

#[test]
fn test_blocks_6to4_private() {
    let policy = NetworkPolicy::default();
    // 2002:0a00:0001::1 embeds 10.0.0.1
    assert!(policy.is_addr_allowed(&"[2002:0a00:0001::1]:80".parse().unwrap()).is_err());
    // 2002:c0a8:0101::1 embeds 192.168.1.1
    assert!(policy.is_addr_allowed(&"[2002:c0a8:0101::1]:80".parse().unwrap()).is_err());
    // 2002:7f00:0001::1 embeds 127.0.0.1
    assert!(policy.is_addr_allowed(&"[2002:7f00:0001::1]:80".parse().unwrap()).is_err());
    // 2002:a9fe:a9fe::1 embeds 169.254.169.254 (metadata)
    assert!(policy.is_addr_allowed(&"[2002:a9fe:a9fe::1]:80".parse().unwrap()).is_err());
}

#[test]
fn test_blocks_teredo_private() {
    let policy = NetworkPolicy::default();
    // 10.0.0.1 XOR 0xffffffff = f5ff:fffe
    assert!(policy.is_addr_allowed(&"[2001:0000:4136:e378:8000:63bf:f5ff:fffe]:80".parse().unwrap()).is_err());
    // 127.0.0.1 XOR 0xffffffff = 80ff:fffe
    assert!(policy.is_addr_allowed(&"[2001:0000:4136:e378:8000:63bf:80ff:fffe]:80".parse().unwrap()).is_err());
    // 169.254.169.254 XOR 0xffffffff = 5601:5601
    assert!(policy.is_addr_allowed(&"[2001:0000:4136:e378:8000:63bf:5601:5601]:80".parse().unwrap()).is_err());
}

// === ALLOWED: global unicast ===

#[test]
fn test_allows_public_ipv4() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"8.8.8.8:443".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"1.1.1.1:443".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"93.184.216.34:443".parse().unwrap()).is_ok());
}

#[test]
fn test_allows_public_ipv6() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"[2607:f8b0:4004:800::200e]:443".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"[2606:4700:4700::1111]:443".parse().unwrap()).is_ok());
}

#[test]
fn test_allows_6to4_public() {
    let policy = NetworkPolicy::default();
    // 2002:0808:0808::1 embeds 8.8.8.8
    assert!(policy.is_addr_allowed(&"[2002:0808:0808::1]:80".parse().unwrap()).is_ok());
}

// === PORT ALLOWLIST ===

#[test]
fn test_port_allowlist() {
    let policy = NetworkPolicy::default();
    // allowed ports
    assert!(policy.is_addr_allowed(&"8.8.8.8:80".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"8.8.8.8:443".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"8.8.8.8:8080".parse().unwrap()).is_ok());
    assert!(policy.is_addr_allowed(&"8.8.8.8:8443".parse().unwrap()).is_ok());
    // blocked ports
    assert!(policy.is_addr_allowed(&"8.8.8.8:22".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"8.8.8.8:3306".parse().unwrap()).is_err());
    assert!(policy.is_addr_allowed(&"8.8.8.8:6379".parse().unwrap()).is_err());
}

// === BOUNDARY TESTS ===

#[test]
fn test_boundary_172() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"172.15.255.255:80".parse().unwrap()).is_ok());  // before
    assert!(policy.is_addr_allowed(&"172.16.0.0:80".parse().unwrap()).is_err());     // start
    assert!(policy.is_addr_allowed(&"172.31.255.255:80".parse().unwrap()).is_err()); // end
    assert!(policy.is_addr_allowed(&"172.32.0.0:80".parse().unwrap()).is_ok());      // after
}

#[test]
fn test_boundary_cgnat() {
    let policy = NetworkPolicy::default();
    assert!(policy.is_addr_allowed(&"100.63.255.255:80".parse().unwrap()).is_ok());  // before
    assert!(policy.is_addr_allowed(&"100.64.0.0:80".parse().unwrap()).is_err());     // start
    assert!(policy.is_addr_allowed(&"100.127.255.255:80".parse().unwrap()).is_err()); // end
    assert!(policy.is_addr_allowed(&"100.128.0.0:80".parse().unwrap()).is_ok());     // after
}
