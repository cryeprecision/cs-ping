use std::net::IpAddr;
use std::time::Duration;
use surge_ping::SurgeError;

pub struct Pinger {
    addr: IpAddr,
}

impl Pinger {
    pub fn new(addr: IpAddr) -> Pinger {
        Pinger { addr }
    }

    /// Ping a single address and map a timeout to `None`
    pub async fn try_ping(&self) -> Result<Option<Duration>, SurgeError> {
        match surge_ping::ping(self.addr).await {
            Err(err) => match err {
                surge_ping::SurgeError::Timeout { seq: _ } => Ok(None),
                _ => return Err(err),
            },
            Ok((_, duration)) => Ok(Some(duration)),
        }
    }

    /// Ping `addr` `npings`-times while allowing `ntimeouts` timeouts
    pub async fn ping(&self, npings: usize, mut ntimeouts: usize) -> Option<Vec<Duration>> {
        let mut results = Vec::with_capacity(npings);
        for _ in 0..npings {
            println!("Trying to ping");
            loop {
                if let Some(dur) = self.try_ping().await.unwrap() {
                    results.push(dur);
                } else if ntimeouts == 0 {
                    return None;
                } else {
                    ntimeouts -= 1;
                }
            }
        }
        return Some(results);
    }
}
