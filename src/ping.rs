use crate::stats::Stats;
use futures::stream::iter;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::net::IpAddr;
use std::net::ToSocketAddrs;
use std::time::Duration;

const MAX_CONCURRENT_PINGS: usize = 8;
const MAX_TIMEOUT_RETRIES: usize = 3;
const PING_COUNT: usize = 6;

const BAR_FORMAT: &str = "{spinner} {msg} [{pos:>4}/{len:<4}] [{bar:25.cyan/grey}] {eta}";
// Binary counter - https://raw.githubusercontent.com/sindresorhus/cli-spinners/master/spinners.json
const BAR_TICK_CHARS: &str = "⠁⠂⠃⠄⠅⠆⠇⡀⡁⡂⡃⡄⡅⡆⡇⠈⠉⠊⠋⠌⠍⠎⠏⡈⡉⡊⡋⡌⡍⡎⡏⠐⠑⠒⠓⠔⠕⠖⠗⡐⡑⡒⡓⡔⡕⡖⡗⠘⠙⠚⠛⠜\
⠝⠞⠟⡘⡙⡚⡛⡜⡝⡞⡟⠠⠡⠢⠣⠤⠥⠦⠧⡠⡡⡢⡣⡤⡥⡦⡧⠨⠩⠪⠫⠬⠭⠮⠯⡨⡩⡪⡫⡬⡭⡮⡯⠰⠱⠲⠳⠴⠵⠶⠷⡰⡱⡲⡳⡴⡵⡶⡷⠸⠹⠺⠻⠼⠽⠾⠿⡸⡹⡺⡻⡼⡽⡾⡿⢀\
⢁⢂⢃⢄⢅⢆⢇⣀⣁⣂⣃⣄⣅⣆⣇⢈⢉⢊⢋⢌⢍⢎⢏⣈⣉⣊⣋⣌⣍⣎⣏⢐⢑⢒⢓⢔⢕⢖⢗⣐⣑⣒⣓⣔⣕⣖⣗⢘⢙⢚⢛⢜⢝⢞⢟⣘⣙⣚⣛⣜⣝⣞⣟⢠⢡⢢⢣⢤⢥⢦⢧⣠⣡⣢⣣⣤\
⣥⣦⣧⢨⢩⢪⢫⢬⢭⢮⢯⣨⣩⣪⣫⣬⣭⣮⣯⢰⢱⢲⢳⢴⢵⢶⢷⣰⣱⣲⣳⣴⣵⣶⣷⢸⢹⢺⢻⢼⢽⢾⢿⣸⣹⣺⣻⣼⣽⣾⣿";

/// Resolve a hostname to IPs
fn resolve_hostname(hostname: &str) -> impl ExactSizeIterator<Item = IpAddr> {
    hostname
        .to_socket_addrs()
        .expect("couldn't resolve hostname")
        .map(|s| s.ip())
}

/// Ping a single address and map a timeout to `None`
async fn ping_map_timeout(ip: IpAddr) -> Option<Duration> {
    match surge_ping::ping(ip).await {
        Err(err) => match err {
            surge_ping::SurgeError::Timeout { seq: _ } => None,
            _ => panic!("error pinging {:?}: {}", ip, err),
        },
        Ok((_, duration)) => Some(duration),
    }
}

/// Create a progress bar with custom settings
fn styled_progress_bar(len: usize, msg: &'static str) -> ProgressBar {
    ProgressBar::new(len as u64).with_message(msg).with_style(
        ProgressStyle::default_bar()
            .tick_chars(BAR_TICK_CHARS)
            .template(BAR_FORMAT)
            .progress_chars("=> "),
    )
}

/// Ping `ip` `npings`-times while allowing `ntimeouts` timeouts
async fn ping_n_with_timeout(
    ip: IpAddr,
    npings: usize,
    mut ntimeouts: usize,
) -> Option<Vec<Duration>> {
    let mut r = Vec::with_capacity(npings);
    while r.len() < npings {
        match ping_map_timeout(ip.clone()).await {
            Some(d) => r.push(d),
            None => ntimeouts -= 1,
        }
        if ntimeouts == 0 {
            return None;
        }
    }
    return Some(r);
}

/// Pings `ip` and calculates some stats if successfull, else propagates `None`.
async fn ping(ip: IpAddr, max_timeout_retires: usize) -> Option<Stats> {
    let ds = ping_n_with_timeout(ip, PING_COUNT, max_timeout_retires).await;
    return ds.as_ref().map(Stats::from_durations);
}

/// State-machine
pub enum JobRunner {
    /// Holds a list of hostnames that will be resolved into IPs
    Waiting(Vec<String>),

    /// Holds the same list as above but also a vector of tuples, where the first tuple-element
    /// is an index into the vector of hostnames and the second tuple-element is one of the
    /// many IP-addresses that the hostname maps to.
    Resolved(Vec<String>, Vec<(usize, IpAddr)>),

    /// Holds a vector of tuples where the first tuple-element is the hostname and the second
    /// tuple-element is the minimum statistics for the ping to the IP-address out of all
    /// IP-addresses that the hostname maps to.
    Pinged(Vec<(String, Option<Stats>)>),
}

impl JobRunner {
    pub fn new(hostnames: Vec<String>) -> JobRunner {
        JobRunner::Waiting(hostnames)
    }

    /// Resolve all hostnames. The first tuple-element in the return-value
    /// is an index into the hostnames-slice.
    ///
    /// The result is shuffled for more consistent results when pinging concurrently.
    fn resolve_hostnames(hostnames: &[String]) -> Vec<(usize, IpAddr)> {
        let mut vec = Vec::new();

        let bar = styled_progress_bar(hostnames.len(), "Resolving hostnames");

        for (index, hostname) in hostnames.iter().enumerate() {
            let ip_iter = resolve_hostname(hostname);
            vec.reserve(ip_iter.len());
            vec.extend(ip_iter.map(|ip| (index, ip)));
            bar.inc(1);
        }
        bar.finish_and_clear();

        // shuffle, so requests to servers that are close together
        // aren't clustered - for more consistent results
        vec.shuffle(&mut thread_rng());

        return vec;
    }

    async fn ping_all(
        hostnames: Vec<String>,
        ips: Vec<(usize, IpAddr)>,
        max_concurrent_pings: Option<usize>,
        max_timeout_retries: Option<usize>,
    ) -> Vec<(String, Option<Stats>)> {
        async fn wrap_ping(
            tpl: (usize, IpAddr),
            max_timeout_retries: usize,
        ) -> (usize, Option<Stats>) {
            (tpl.0, ping(tpl.1, max_timeout_retries).await)
        }

        let max_concurrent_pings = max_concurrent_pings.unwrap_or(MAX_CONCURRENT_PINGS);
        let max_timeout_retries = max_timeout_retries.unwrap_or(MAX_TIMEOUT_RETRIES);

        let bar = styled_progress_bar(ips.len(), "Pinging IPs");
        let mut ping_results = Vec::with_capacity(ips.len());

        let futures = ips.into_iter().map(|ip| wrap_ping(ip, max_timeout_retries));
        let mut buffered = iter(futures).buffer_unordered(max_concurrent_pings);

        while let Some(tuple) = buffered.next().await {
            ping_results.push(tuple);
            bar.inc(1);
        }
        bar.finish_and_clear();

        ping_results.sort_unstable_by(|(li, ls), (ri, rs)| {
            if li != ri {
                li.cmp(ri)
            } else {
                Stats::cmp(ls, rs)
            }
        });

        let max_index = ping_results.last().unwrap().0;
        let mut results = Vec::with_capacity(max_index);

        // Collect the statistics with the lowest avg-latency for each hostname
        let mut hostnames_iter = hostnames.into_iter();
        let mut ping_iter = ping_results.into_iter();
        while let Some((_, stats)) = ping_iter.find(|(index, _)| index == &results.len()) {
            results.push((hostnames_iter.next().unwrap(), stats));
        }
        assert_eq!(hostnames_iter.next(), None);

        return results;
    }

    pub fn waiting_to_resolved(self) -> Self {
        match self {
            JobRunner::Waiting(hostnames) => {
                let ips = Self::resolve_hostnames(&hostnames);
                return JobRunner::Resolved(hostnames, ips);
            }
            _ => unreachable!("uh oh in resolve hostnames"),
        }
    }

    pub async fn resolved_to_pinged(
        self,
        max_concurrent_pings: Option<usize>,
        max_timeout_retries: Option<usize>,
    ) -> Self {
        match self {
            JobRunner::Resolved(hostnames, ips) => JobRunner::Pinged(
                Self::ping_all(hostnames, ips, max_concurrent_pings, max_timeout_retries).await,
            ),
            _ => unreachable!("uh oh in ping all ips"),
        }
    }

    pub fn finalize(self) -> Vec<(String, Option<Stats>)> {
        match self {
            JobRunner::Pinged(mut stats) => {
                stats.sort_unstable_by(|(_, lhs), (_, rhs)| Stats::cmp(&lhs, &rhs));
                return stats;
            }
            _ => unreachable!("uh oh in get results"),
        }
    }
}
