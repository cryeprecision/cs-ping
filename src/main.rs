use futures::StreamExt;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::time::Duration;
use zzz::{ProgressBar, ProgressBarIterExt};

const MAX_CONCURRENT_JOBS: usize = 8;
const MAX_TIMEOUT_RETIRES: usize = 5;
const PING_AVERAGE_COUNT: usize = 5;

async fn ping(addr: IpAddr) -> Option<[Duration; PING_AVERAGE_COUNT]> {
    let mut result = [Duration::default(); PING_AVERAGE_COUNT];
    let mut result_iter = result.iter_mut();
    let mut timeouts_left = MAX_TIMEOUT_RETIRES;

    while let Some(result_slot) = result_iter.next() {
        loop {
            match surge_ping::ping(addr).await {
                Ok((_, duration)) => {
                    *result_slot = duration;
                    break;
                }
                Err(err) => match err {
                    surge_ping::SurgeError::Timeout { seq: _ } => {
                        if timeouts_left == 0 {
                            return None;
                        }
                        timeouts_left -= 1;
                    }
                    _ => panic!("{}", err),
                },
            }
        }
    }
    return Some(result);
}

fn get_average_ms(durations: &[Duration; PING_AVERAGE_COUNT]) -> f32 {
    (durations
        .iter()
        .fold(0f32, |acc, next| acc + next.as_secs_f32())
        / (PING_AVERAGE_COUNT as f32))
        * 1000f32
}

fn get_std_dev(durations: &[Duration; PING_AVERAGE_COUNT]) -> f32 {
    let (sum, sum_sq) = durations
        .iter()
        .fold((0f32, 0f32), |(acc, acc_sq), duration| {
            let ms = duration.as_secs_f32() * 1000f32;
            (acc + ms, acc_sq + (ms * ms))
        });
    return (sum_sq - (sum * sum) / (PING_AVERAGE_COUNT as f32))
        / ((PING_AVERAGE_COUNT - 1) as f32);
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

    let mut pb = ProgressBar::with_target(jobs.len());
    pb.set_message(Some("Pinging IPs"));

    let futures = jobs.into_iter().map(Job::run).collect::<Vec<_>>();
    let mut stream = futures::stream::iter(futures).buffer_unordered(MAX_CONCURRENT_JOBS);

    let mut result_map = HashMap::<String, Option<(f32, f32)>>::new();

    while let Some((job, output)) = stream.next().await {
        pb.add(1);
        let entry = result_map
            .entry(job.shared.locations[job.index].clone())
            .or_default();
        let (duration_ms, std_dev) = match output.durations {
            Some(val) => (get_average_ms(&val), get_std_dev(&val)),
            None => continue,
        };
        if entry.is_none() || duration_ms.partial_cmp(&entry.unwrap().0).unwrap() == Ordering::Less
        {
            *entry = Some((duration_ms, std_dev));
        }
    }
    drop(pb);

    let mut results = result_map.into_iter().collect::<Vec<_>>();
    results.sort_unstable_by(|(_, lhs_ms), (_, rhs_ms)| {
        let lhs_ms = lhs_ms.map_or(std::f32::MAX, |(ms, _)| ms);
        let rhs_ms = rhs_ms.map_or(std::f32::MAX, |(ms, _)| ms);
        return lhs_ms.partial_cmp(&rhs_ms).unwrap();
    });

    for (name, opt) in results {
        match opt {
            Some((ms, std_dev)) => {
                println!("{: <20} -> {: >5.0}ms, ±{: <5.2}ms", name, ms, std_dev)
            }
            None => println!("{: <20} -> TIMEOUT", name),
        }
    }
}
