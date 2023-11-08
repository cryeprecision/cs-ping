use std::net::{IpAddr, ToSocketAddrs};

pub fn resolve_hostname(hostname: &str) -> Option<Vec<IpAddr>> {
    Some(
        hostname
            .to_socket_addrs()
            .ok()?
            .map(|sock| sock.ip())
            .collect(),
    )
}

#[cfg(test)]
mod test {
    use crate::host_lookup::resolve_hostname;

    #[test]
    fn resolves() {
        const DOMAINS: &str = include_str!("./../domains.txt");

        let queries = DOMAINS
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| format!("{}:443", l.trim()))
            .collect::<Vec<_>>();

        for query in queries {
            let start = std::time::Instant::now();
            let result = resolve_hostname(&query);
            let elapsed_ms = start.elapsed().as_millis();
            println!("[{}] ({}ms) -> {:?} ", query, elapsed_ms, result);
        }
    }
}
