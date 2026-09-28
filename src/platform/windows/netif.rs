//! The addresses this machine has: every unicast address of an adapter
//! that is up, loopback left out (`GetAdaptersAddresses`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};

pub fn addresses() -> Vec<IpAddr> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u8>;
    // The table may grow between the size it says and the call: ask again.
    let mut tries = 0;
    loop {
        buf = vec![0u8; size as usize];
        let r = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                flags,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        // ERROR_BUFFER_OVERFLOW: `size` now says how much.
        if r == 111 && tries < 3 {
            tries += 1;
            continue;
        }
        if r != 0 {
            return Vec::new();
        }
        break;
    }
    let mut out = Vec::new();
    let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    unsafe {
        while !adapter.is_null() {
            let a = &*adapter;
            if a.OperStatus == IfOperStatusUp {
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() {
                        let ip = match (*sa).sa_family {
                            AF_INET => {
                                let v4 = &*(sa as *const SOCKADDR_IN);
                                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr))))
                            }
                            AF_INET6 => {
                                let v6 = &*(sa as *const SOCKADDR_IN6);
                                Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.u.Byte)))
                            }
                            _ => None,
                        };
                        if let Some(ip) = ip.filter(|ip| !ip.is_loopback() && !ip.is_unspecified()) {
                            out.push(ip);
                        }
                    }
                    u = (*u).Next;
                }
            }
            adapter = a.Next;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_machine_has_addresses_and_none_is_loopback() {
        let addrs = super::addresses();
        assert!(addrs.iter().all(|a| !a.is_loopback()), "{addrs:?}");
        // Every machine that runs the tests is on a network of some kind.
        assert!(!addrs.is_empty());
    }
}
