//! LAN-only destination selection. Literal IP settings never require DNS.
use core::{future::Future, net::Ipv4Addr};

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    Dns,
    NonLocal,
}

pub async fn resolve<F, Fut>(host: &str, dns: F) -> Result<Ipv4Addr, ResolveError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Ipv4Addr, ResolveError>>,
{
    let address = match host.parse::<Ipv4Addr>() {
        Ok(address) => address,
        Err(_) => dns().await?,
    };
    if crate::config::local_ipv4(address) {
        Ok(address)
    } else {
        Err(ResolveError::NonLocal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{cell::Cell, future::ready};

    #[tokio::test]
    async fn literal_destinations_never_call_dns_even_when_it_is_unavailable() {
        let lookups = Cell::new(0);
        for (host, expected) in [
            ("192.168.6.1", Ok(Ipv4Addr::new(192, 168, 6, 1))),
            ("10.1.2.3", Ok(Ipv4Addr::new(10, 1, 2, 3))),
            ("8.8.8.8", Err(ResolveError::NonLocal)),
        ] {
            let result = resolve(host, || {
                lookups.set(lookups.get() + 1);
                ready(Err(ResolveError::Dns))
            })
            .await;
            assert_eq!(result, expected);
        }
        assert_eq!(lookups.get(), 0);
    }

    #[tokio::test]
    async fn manual_hostnames_require_a_successful_local_dns_answer() {
        assert_eq!(
            resolve("gateway.local", || ready(Ok(Ipv4Addr::new(192, 168, 6, 1)))).await,
            Ok(Ipv4Addr::new(192, 168, 6, 1))
        );
        assert_eq!(
            resolve("gateway.local", || ready(Ok(Ipv4Addr::new(8, 8, 8, 8)))).await,
            Err(ResolveError::NonLocal)
        );
        assert_eq!(
            resolve("gateway.local", || ready(Err(ResolveError::Dns))).await,
            Err(ResolveError::Dns)
        );
    }
}
