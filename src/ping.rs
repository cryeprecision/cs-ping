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
const MAX_TIMEOUT_RETIRES: usize = 3;
const PING_COUNT: usize = 6;
const SCRIPT_URL: &str = "https://cryptostorm.is/wg_confgen.txt";

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

/// Downloads the script-file and extracts all server locations
async fn get_locations() -> Vec<String> {
    let re = regex::Regex::new("(?m)^\"(\\w+):(:?.*?)\"$").expect("couldn't parse regex");
    let resp = reqwest::get(SCRIPT_URL)
        .await
        .expect("couldn't download script");
    let s = resp.text().await.expect("couldn't get content of script");
    return re.captures_iter(&s).map(|c| c[1].to_string()).collect();
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
async fn ping(ip: IpAddr) -> Option<Stats> {
    let ds = ping_n_with_timeout(ip, PING_COUNT, MAX_TIMEOUT_RETIRES).await;
    return ds.as_ref().map(Stats::from_durations);
}

enum JobRunner {
    /// Holds a list of locations that will be resolved into hostnames
    Waiting(Vec<String>),

    /// Holds the same list as above but also a vector of tuples, where the first tuple-element
    /// is an index into the vector of locations and the second tuple-element is one of the
    /// many IP-addresses that the location maps to.
    Resolved(Vec<String>, Vec<(usize, IpAddr)>),

    /// Holds a vector of tuples where the first tuple-element is the location and the second
    /// tuple-element is the minimum statistics for the ping to the IP-address out of all
    /// IP-addresses that the location maps to.
    Pinged(Vec<(String, Option<Stats>)>),
}

impl JobRunner {
    fn new(locations: Vec<String>) -> JobRunner {
        JobRunner::Waiting(locations)
    }

    /// Resolve a server location to a list of IPs
    fn location_to_ips(location: &str) -> impl ExactSizeIterator<Item = IpAddr> {
        return resolve_hostname(&format!("{}.cstorm.is:443", location));
    }

    /// Resolve all server locations. The first tuple-element in the return-value
    /// is an index into the locations-slice.
    ///
    /// The result is shuffled for more consistent results when pinging concurrently.
    fn resolve_hostnames(locations: &[String]) -> Vec<(usize, IpAddr)> {
        let mut vec = Vec::new();

        let bar = styled_progress_bar(locations.len(), "Resolving hostnames");

        for (index, location) in locations.iter().enumerate() {
            let ip_iter = Self::location_to_ips(location);
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
        locations: Vec<String>,
        ips: Vec<(usize, IpAddr)>,
    ) -> Vec<(String, Option<Stats>)> {
        async fn wrap_ping(tpl: (usize, IpAddr)) -> (usize, Option<Stats>) {
            (tpl.0, ping(tpl.1).await)
        }

        let bar = styled_progress_bar(ips.len(), "Pinging IPs");
        let mut ping_results = Vec::with_capacity(ips.len());

        let futures = ips.into_iter().map(wrap_ping);
        let mut buffered = iter(futures).buffer_unordered(MAX_CONCURRENT_PINGS);

        while let Some(tuple) = buffered.next().await {
            ping_results.push(tuple);
            bar.inc(1);
        }
        bar.finish_and_clear();

        ping_results.sort_unstable_by(|(l_index, l_stats), (r_index, r_stats)| {
            if l_index != r_index {
                l_index.cmp(r_index)
            } else {
                Stats::cmp(l_stats, r_stats)
            }
        });

        let max_index = ping_results.last().unwrap().0;
        let mut results = Vec::with_capacity(max_index);

        // Collect the statistics with the lowest avg-latency for each location
        let mut locations_iter = locations.into_iter();
        let mut ping_iter = ping_results.into_iter();
        while let Some((_, stats)) = ping_iter.find(|(index, _)| index == &results.len()) {
            results.push((locations_iter.next().unwrap(), stats));
        }

        return results;
    }

    fn waiting_to_resolved(self) -> Self {
        match self {
            JobRunner::Waiting(locations) => {
                let hostnames = Self::resolve_hostnames(&locations);
                return JobRunner::Resolved(locations, hostnames);
            }
            _ => unreachable!("uh oh in resolve hostnames"),
        }
    }

    async fn resolved_to_pinged(self) -> Self {
        match self {
            JobRunner::Resolved(locations, ips) => {
                let pings = Self::ping_all(locations, ips).await;
                return JobRunner::Pinged(pings);
            }
            _ => unreachable!("uh oh in ping all ips"),
        }
    }

    fn finalize(self) -> Vec<(String, Option<Stats>)> {
        match self {
            JobRunner::Pinged(mut stats) => {
                stats.sort_unstable_by(|(_, lhs), (_, rhs)| Stats::cmp(&lhs, &rhs));
                return stats;
            }
            _ => unreachable!("uh oh in get results"),
        }
    }
}

pub async fn ping_all_ips() {
    let results = JobRunner::new(get_locations().await)
        .waiting_to_resolved()
        .resolved_to_pinged()
        .await
        .finalize();

    for (location, result) in results.into_iter() {
        match result {
            Some(stats) => println!("{:^16} -> {}", location, stats),
            None => println!("{:^16} -> TIMEOUT", location),
        }
    }
}
