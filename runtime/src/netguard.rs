//! A tiny local proxy that every app window's network traffic goes through (Linux and Windows,
//! where the webview takes a proxy URL). It tunnels HTTPS only to the hosts in the app's
//! `allowedHosts` and refuses everything else, so resource hints, DNS prefetch and anything else
//! the content-security-policy does not cover cannot reach the network. A `web` app has no
//! allowed hosts, so nothing gets through.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};

/// Addresses an app may never reach through the proxy, even with allowedHosts "*": this
/// machine, the local network and link-local ranges.
fn public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => !(v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified()
            || v.is_broadcast() || v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64),
        IpAddr::V6(v) => {
            let s = v.segments();
            !(v.is_loopback() || v.is_unspecified() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
                && v.to_ipv4_mapped().is_none_or(|m| public(&IpAddr::V4(m)))
        }
    }
}

/// Only processes of the same user may use the proxy (Linux).
#[cfg(target_os = "linux")]
fn same_user(s: &TcpStream) -> bool {
    use std::os::fd::AsRawFd;
    // The peer of a loopback TCP socket has no SO_PEERCRED; compare the owner of the socket in
    // /proc/net/tcp instead, matched by the client's port.
    let Ok(peer) = s.peer_addr() else { return false };
    let me = unsafe { libc::getuid() };
    let _ = s.as_raw_fd();
    let port = format!("{:04X}", peer.port());
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(t) = std::fs::read_to_string(f) {
            for l in t.lines().skip(1) {
                let cols: Vec<&str> = l.split_whitespace().collect();
                if cols.len() > 7 && cols[1].ends_with(&format!(":{port}")) {
                    return cols[7].parse::<u32>().ok() == Some(me);
                }
            }
        }
    }
    false
}
#[cfg(not(target_os = "linux"))]
fn same_user(_s: &TcpStream) -> bool {
    true
}

fn host_allowed(host: &str, port: u16, allowed: &[String]) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    allowed.iter().any(|a| {
        if a == "*" {
            return true;
        }
        let rest = a.strip_prefix("https://").or_else(|| a.strip_prefix("http://")).unwrap_or(a);
        let (h, p) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok()),
            None => (rest, None),
        };
        h.eq_ignore_ascii_case(&host) && p.map_or(port == 443, |p| p == port)
    })
}

fn handle(mut client: TcpStream, allowed: Vec<String>) {
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(20)));
    if !same_user(&client) {
        return;
    }
    // At most 8 KB of request line and headers.
    let mut reader = BufReader::new(match client.try_clone() {
        Ok(c) => c.take(8192),
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    // Drain the request headers.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => continue,
            Err(_) => return,
        }
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method.eq_ignore_ascii_case("CONNECT") {
        if let Some((host, port)) = target.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                let addr: Option<SocketAddr> = if host_allowed(host, port, &allowed) {
                    (bare, port).to_socket_addrs().ok().and_then(|mut a| a.find(|a| public(&a.ip())))
                } else {
                    None
                };
                if let Some(addr) = addr {
                    if let Ok(upstream) = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(15)) {
                        let _ = client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                        let _ = client.set_read_timeout(None);
                        let (mut c1, mut u1) = (client.try_clone().unwrap(), upstream.try_clone().unwrap());
                        let (mut c2, mut u2) = (client, upstream);
                        let t = std::thread::spawn(move || {
                            let _ = std::io::copy(&mut c1, &mut u1);
                            let _ = u1.shutdown(Shutdown::Write);
                        });
                        let _ = std::io::copy(&mut u2, &mut c2);
                        let _ = c2.shutdown(Shutdown::Write);
                        let _ = t.join();
                        return;
                    }
                }
            }
        }
    }
    let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}

/// Start the proxy for one app window; returns its URL.
pub fn start(allowed: Vec<String>) -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            use std::sync::atomic::Ordering;
            if live.load(Ordering::SeqCst) >= 64 {
                continue; // dropped: too many open tunnels
            }
            let allowed = allowed.clone();
            let live = live.clone();
            live.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                handle(stream, allowed);
                live.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    Ok(format!("http://{addr}"))
}

#[cfg(test)]
mod tests {
    use super::{host_allowed, public};

    #[test]
    fn the_proxy_answers_its_own_user_and_refuses_unlisted_hosts() {
        use std::io::{Read, Write};
        let url = super::start(vec!["api.example.com".into()]).unwrap();
        let addr = url.trim_start_matches("http://");
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.write_all(b"CONNECT evil.example:443 HTTP/1.1\r\nHost: evil.example:443\r\n\r\n").unwrap();
        let mut buf = String::new();
        c.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 403"), "{buf:?}");
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.write_all(b"CONNECT 127.0.0.1:22 HTTP/1.1\r\n\r\n").unwrap();
        let mut buf = String::new();
        c.read_to_string(&mut buf).unwrap();
        assert!(buf.starts_with("HTTP/1.1 403"), "{buf:?}");
    }

    #[test]
    fn local_addresses_are_never_reached() {
        for a in ["127.0.0.1", "10.1.2.3", "192.168.0.1", "172.16.5.5", "169.254.1.1", "::1", "fe80::1", "fc00::1", "100.64.0.1", "::ffff:127.0.0.1"] {
            assert!(!public(&a.parse().unwrap()), "{a}");
        }
        assert!(public(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn only_listed_hosts_get_through() {
        let a = vec!["api.example.com".to_string(), "https://other.example:8443".to_string()];
        assert!(host_allowed("api.example.com", 443, &a));
        assert!(host_allowed("API.example.com", 443, &a));
        assert!(!host_allowed("api.example.com", 80, &a));
        assert!(host_allowed("other.example", 8443, &a));
        assert!(!host_allowed("evil.example", 443, &a));
        assert!(!host_allowed("api.example.com.evil.example", 443, &a));
        assert!(!host_allowed("x", 443, &[]));
    }
}
