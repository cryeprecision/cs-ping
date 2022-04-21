use futures::StreamExt;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::time::Duration;
use zzz::{ProgressBar, ProgressBarIterExt, ProgressBarStreamExt};

const MAX_CONCURRENT_JOBS: usize = 8;
const MAX_TIMEOUT_RETIRES: usize = 3;
const PING_AVERAGE_COUNT: usize = 6;

async fn try_ping(addr: IpAddr) -> Option<Duration> {
    match surge_ping::ping(addr).await {
        Err(err) => match err {
            surge_ping::SurgeError::Timeout { seq: _ } => None,
            _ => panic!("{}", err),
        },
        Ok((_, duration)) => Some(duration),
    }
}

async fn ping(addr: IpAddr) -> Option<[Duration; PING_AVERAGE_COUNT]> {
    let mut result = [Duration::default(); PING_AVERAGE_COUNT];
    let mut result_iter = result.iter_mut();
    let mut timeouts_left = MAX_TIMEOUT_RETIRES;

    while let Some(result_slot) = result_iter.next() {
        loop {
            match try_ping(addr).await {
                Some(val) => {
                    *result_slot = val;
                    break;
                }
                None => timeouts_left -= 1,
            }
            if timeouts_left == 0 {
                return None;
            }
        }
    }
    return Some(result);
}

fn get_socket_addresses(location: &str) -> Vec<IpAddr> {
    format!("{}.cstorm.is:443", location)
        .to_socket_addrs()
        .expect("couldn't resolve hostname")
        .map(|s| s.ip())
        .collect()
}

async fn get_script_text() -> String {
    reqwest::get("https://cryptostorm.is/wg_confgen.txt")
        .await
        .expect("couldn't download script")
        .text()
        .await
        .expect("couldn't get content of script")
}

// Shared across multiple jobs
struct Shared {
    locations: Vec<String>,
}

struct Job {
    shared: Rc<Shared>,
    index: usize,
    addr: IpAddr,
}

struct Output {
    durations: Option<[Duration; PING_AVERAGE_COUNT]>,
}

impl Job {
    fn new(shared: Rc<Shared>, index: usize, addr: &IpAddr) -> Self {
        Job {
            shared: shared,
            index: index,
            addr: addr.clone(),
        }
    }
    async fn work(&mut self) -> Output {
        Output {
            durations: ping(self.addr.clone()).await,
        }
    }
    async fn run(mut self) -> (Self, Output) {
        let output = self.work().await;
        (self, output)
    }
}

#[derive(Debug)]
struct PingStats {
    avg: f32,
    std_dev: f32,
    min: f32,
    max: f32,
    all: [f32; PING_AVERAGE_COUNT],
}

impl PartialEq for PingStats {
    fn eq(&self, other: &Self) -> bool {
        self.avg.eq(&other.avg)
    }
}
impl PartialOrd for PingStats {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.avg.partial_cmp(&other.avg)
    }
}

impl PingStats {
    fn avg(ms: &[f32; PING_AVERAGE_COUNT]) -> f32 {
        let folder = |acc, next| acc + next;
        return ms.iter().fold(0_f32, folder) / (PING_AVERAGE_COUNT as f32);
    }
    fn std_dev(ms: &[f32; PING_AVERAGE_COUNT]) -> f32 {
        let folder = |(acc, acc_sq), ms| (acc + ms, acc_sq + (ms * ms));
        let (sum, sum_sq) = ms.iter().fold((0_f32, 0_f32), folder);
        let s1 = (sum * sum) / (PING_AVERAGE_COUNT as f32);
        return ((sum_sq - s1) / ((PING_AVERAGE_COUNT - 1) as f32)).sqrt();
    }
    fn min(ms: &[f32; PING_AVERAGE_COUNT]) -> f32 {
        let reducer = |min, next| if next < min { next } else { min };
        return ms.iter().reduce(reducer).unwrap().to_owned();
    }
    fn max(ms: &[f32; PING_AVERAGE_COUNT]) -> f32 {
        let reducer = |max, next| if next > max { next } else { max };
        return ms.iter().reduce(reducer).unwrap().to_owned();
    }

    fn new(durations: &[Duration; PING_AVERAGE_COUNT]) -> Self {
        let ms = durations.map(|d| d.as_secs_f32() * 1000_f32);
        Self {
            avg: Self::avg(&ms),
            std_dev: Self::std_dev(&ms),
            min: Self::min(&ms),
            max: Self::max(&ms),
            all: ms,
        }
    }
}

// fn post_process_results(results: Vec<>)

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let script_text = get_script_text().await;

    let re = regex::Regex::new("(?m)^\"(\\w+):(:?.*?)\"$").expect("couldn't create regex");

    let locations = re
        .captures_iter(&script_text)
        .map(|cap| cap[1].to_string())
        .collect::<Vec<_>>();

    let shared = Rc::new(Shared { locations });

    let mut pb = ProgressBar::smart();
    pb.set_message(Some("Resolving Hostnames"));

    let mut jobs = Vec::<Job>::new();

    for (i, location) in shared.locations.iter().with_progress(pb).enumerate() {
        let ips = get_socket_addresses(location);
        jobs.reserve(ips.len());
        for ip in ips {
            jobs.push(Job::new(shared.clone(), i, &ip));
        }
    }

    let mut pb = ProgressBar::smart();
    pb.set_message(Some("    Pinging IPs    "));

    let futures = jobs.into_iter().map(Job::run).collect::<Vec<_>>();
    let mut stream = futures::stream::iter(futures)
        .buffer_unordered(MAX_CONCURRENT_JOBS)
        .with_progress(pb);

    let mut result_map = HashMap::<String, Option<PingStats>>::new();

    while let Some((job, output)) = stream.next().await {
        let location = job.shared.locations[job.index].clone();
        let entry = result_map.entry(location).or_default();
        let stats = match output.durations {
            Some(val) => PingStats::new(&val),
            None => continue,
        };
        if entry.is_none() || &stats < entry.as_ref().unwrap() {
            *entry = Some(stats);
        }
    }
    drop(stream);

    let mut results = result_map.into_iter().collect::<Vec<_>>();
    results.sort_unstable_by(|(_, lhs), (_, rhs)| {
        if lhs.is_none() && rhs.is_none() {
            Ordering::Equal
        } else if rhs.is_none() {
            Ordering::Less
        } else if lhs.is_none() {
            Ordering::Greater
        } else {
            lhs.partial_cmp(rhs).unwrap()
        }
    });

    for (name, opt) in results {
        match opt {
            Some(stats) => println!(
                "{: <20} -> {: >4.0}ms ±{: >2.0}ms range: [{: >4.0}, {: <4.0}]",
                name, stats.avg, stats.std_dev, stats.min, stats.max
            ),
            None => println!("{: <20} -> TIMEOUT", name),
        }
    }
}
