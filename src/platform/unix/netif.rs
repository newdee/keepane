//! The addresses this machine has: every address of an interface that is
//! up, loopback left out (`getifaddrs`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn addresses() -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return out;
    }
    let mut cur = list;
    unsafe {
        while !cur.is_null() {
            let ifa = &*cur;
            let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0 && ifa.ifa_flags & libc::IFF_LOOPBACK as u32 == 0;
            if up && !ifa.ifa_addr.is_null() {
                let ip = match i32::from((*ifa.ifa_addr).sa_family) {
                    libc::AF_INET => {
                        let v4 = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                        Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))))
                    }
                    libc::AF_INET6 => {
                        let v6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                        Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.s6_addr)))
                    }
                    _ => None,
                };
                if let Some(ip) = ip.filter(|ip| !ip.is_loopback() && !ip.is_unspecified()) {
                    out.push(ip);
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_machine_has_addresses_and_none_is_loopback() {
        let addrs = super::addresses();
        assert!(addrs.iter().all(|a| !a.is_loopback()), "{addrs:?}");
        assert!(!addrs.is_empty());
    }
}
