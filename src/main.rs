use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures::StreamExt;
use indicatif::ProgressBar;
use rand::seq::SliceRandom;
use rand::Rng;
use surge_ping::{PingIdentifier, PingSequence};
use tokio::io::AsyncWriteExt;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::{Name, TokioAsyncResolver};

use crate::stats::Stats;

mod stats;
mod util;
mod wireguard;

const DEFAULT_SCRIPT_URL: &str = "https://cryptostorm.is/wg_confgen.txt";
const DEFAULT_CONCURRENT_DNS: usize = 1;
const DEFAULT_CONCURRENT_PINGS: usize = 5;
const DEFAULT_PING_COUNT: usize = 5;
const DEFAULT_PING_RETRIES: usize = 3;
const DEFAULT_CONFIG_FOLDER: &str = "./configs/";
const CRYPTOSTORM_SUFFIX: &str = ".cstorm.is";

#[derive(Debug, Clone)]
struct Host {
    hostname: Arc<str>,
    public_key: Arc<str>,
    ips_v4: Arc<[Ipv4Addr]>,
    duration: Duration,
    stats: Option<Stats>,
}

/// - Download the script file
/// - Extract hostnames from the script file
/// - Resolve hostnames into IPv4 addresses
async fn get_hostnames() -> anyhow::Result<Vec<Host>> {
    let re = lazy_regex::regex!(r#"(?m)^"(\w+):(.*?)"$"#);
    let url = util::env_var("SCRIPT_URL", Some(DEFAULT_SCRIPT_URL)).unwrap();

    // download the script file
    let resp = reqwest::get(url).await.context("download wg_confgen.txt")?;
    let text = resp.text().await.context("download wg_confgen.txt")?;

    // extract hostnames and corresponding public keys from the txt file
    let hosts: Vec<(Arc<str>, Arc<str>)> = re
        .captures_iter(&text)
        .map(|capture| {
            let location = capture.get(1).unwrap().as_str();
            let public_key = capture.get(2).unwrap().as_str();
            (
                format!("{}{}", location, CRYPTOSTORM_SUFFIX).into(),
                public_key.to_string().into(),
            )
        })
        .collect();

    let resolver =
        TokioAsyncResolver::tokio(ResolverConfig::cloudflare_tls(), ResolverOpts::default());
    let bar = util::styled_progress_bar(hosts.len() as u64, "Resolve Hostnames");

    // resolve hostnames into ip addresses
    let concurrent_dns =
        util::env_var_parse("CONCURRENT_DNS", Some(DEFAULT_CONCURRENT_DNS)).unwrap();
    let hosts = futures::stream::iter(hosts)
        .map(|(hostname, public_key)| {
            let resolver = &resolver;
            let bar = &bar;
            async move {
                let name = Name::from_utf8(hostname.as_ref()).context("convert host to a name")?;

                let start = Instant::now();
                let resolved_v4 = resolver
                    .ipv4_lookup(name.clone())
                    .await
                    .with_context(|| format!("resolve ip of {}", hostname));
                let elapsed = start.elapsed();

                bar.inc(1);
                Ok::<_, anyhow::Error>(Host {
                    hostname,
                    public_key,
                    // if there was an error, return an empty vec
                    ips_v4: resolved_v4.map_or_else(
                        |_| Vec::new().into(),
                        |ip| ip.into_iter().map(Ipv4Addr::from).collect(),
                    ),
                    duration: elapsed,
                    stats: None,
                })
            }
        })
        .buffer_unordered(concurrent_dns)
        .collect::<Vec<_>>()
        .await;

    bar.finish_and_clear();
    hosts.into_iter().collect()
}

#[derive(Clone)]
struct Job {
    host: Host,
    ip: Ipv4Addr,
    state: JobState,
}

#[derive(Clone)]
enum JobState {
    /// Hostnames were resolved to a list of IP addresses
    Resolved { client: surge_ping::Client },
    /// IP adresses were pinged and the durations collected
    Pinged { results: Arc<[Duration]> },
    /// Something went wrong
    Err { errors: Vec<Arc<anyhow::Error>> },
}

impl Job {
    fn new(host: Host, ip: Ipv4Addr, client: surge_ping::Client) -> Job {
        Job {
            host,
            ip,
            state: JobState::Resolved { client },
        }
    }

    pub fn finish(self) -> Result<Arc<[Duration]>, Vec<Arc<anyhow::Error>>> {
        match self.state {
            JobState::Pinged { results } => Ok(results),
            JobState::Err { errors } => Err(errors),
            _ => unreachable!("called finish on unfinished job"),
        }
    }

    async fn try_resolved_to_pinged(&mut self, bar: &ProgressBar) {
        let JobState::Resolved { client } = &self.state else {
            self.to_error(anyhow::anyhow!("illegal state transition"));
            return;
        };

        let mut pinger = client
            .pinger(self.ip.into(), PingIdentifier(rand::thread_rng().gen()))
            .await;

        let payload = [0u8; 32];
        let mut results = Vec::new();

        let ping_count = util::env_var_parse("PING_COUNT", Some(DEFAULT_PING_COUNT)).unwrap();
        let mut ping_retries =
            util::env_var_parse("PING_RETRIES", Some(DEFAULT_PING_RETRIES)).unwrap();

        'outer: for seq in 0..ping_count {
            // try to ping the ip with some retries
            let error: anyhow::Error = loop {
                match pinger.ping(PingSequence(seq as u16), &payload).await {
                    Ok((_, duration)) => {
                        results.push(duration);
                        bar.inc(1);

                        if results.len() == ping_count {
                            break 'outer;
                        }
                        continue 'outer;
                    }
                    Err(_) if ping_retries > 0 => ping_retries -= 1,
                    Err(err) => break err.into(),
                };
            };

            // error case - out of retries
            self.to_error(error.context(format!(
                "out of retries to ping {} of {} (seq: {})",
                self.ip, self.host.hostname, seq
            )));
            // accomodate for pings that wont be sent so the bar doesn't ent up partially full
            bar.inc((ping_count - seq) as u64);

            return;
        }

        self.state = JobState::Pinged {
            results: results.into(),
        };
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_error(&mut self, error: anyhow::Error) {
        if let JobState::Err { errors } = &mut self.state {
            errors.push(Arc::new(error));
        } else {
            self.state = JobState::Err {
                errors: vec![Arc::new(error)],
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let _ = env_logger::builder()
        .format_timestamp_millis()
        .filter_level(log::LevelFilter::Info)
        .init();

    // try to load a dotenv file and ignore the case where none is found
    let _ = dotenv::dotenv();

    let client =
        surge_ping::Client::new(&surge_ping::Config::default()).context("create ping client")?;
    let mut hosts = get_hostnames().await.context("get ping targets")?;

    let stats = Stats::from_durations(hosts.iter().map(|host| host.duration));
    log::info!(
        "DNS Performance: {}",
        stats.map_or(String::new(), |stats| stats.format_millis().to_string())
    );

    let mut jobs = hosts
        .clone()
        .into_iter()
        .flat_map(|host| {
            host.ips_v4
                .iter()
                .map(|&ip| Job::new(host.clone(), ip, client.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    // randomize order of jobs for a delay between requests to ips of the same hostname
    jobs.shuffle(&mut rand::thread_rng());

    // ping each ip a few times and collect the rtt
    let bar = util::styled_progress_bar((jobs.len() * DEFAULT_PING_COUNT) as u64, "Ping IPs");
    let concurrent_pings =
        util::env_var_parse("CONCURRENT_PINGS", Some(DEFAULT_CONCURRENT_PINGS)).unwrap();
    let _ = futures::stream::iter(&mut jobs)
        .map(|job| job.try_resolved_to_pinged(&bar))
        .buffer_unordered(concurrent_pings)
        .collect::<Vec<()>>()
        .await;
    bar.finish_and_clear();

    // collect the result of each ip into the corresponding hostname
    let mut durations = HashMap::<Arc<str>, Vec<Duration>>::new();
    jobs.into_iter().for_each(|job| {
        // if there was an error, print it
        if let JobState::Err { errors } = &job.state {
            for error in errors.iter() {
                log::warn!("{}", error);
            }
            return;
        }

        let entry = durations.entry(job.host.hostname.clone()).or_default();
        if let Ok(durations) = job.finish() {
            entry.extend_from_slice(durations.as_ref());
        }
    });

    // calculate stats for each hostname
    let stats = durations
        .into_iter()
        .map(|(host, durations)| (host, Stats::from_durations(durations.into_iter())))
        .collect::<HashMap<_, _>>();

    // add the accumulated stats to the corresponding host
    hosts.iter_mut().for_each(|host| {
        match stats.get(&host.hostname) {
            None => (),
            Some(stats) => host.stats = stats.clone(),
        };
    });

    // sort ascending by average latency putting unreachable servers at the top
    hosts.sort_unstable_by(|lhs, rhs| match (lhs.stats.as_ref(), rhs.stats.as_ref()) {
        (Some(lhs), Some(rhs)) => lhs.average.total_cmp(&rhs.average),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });

    // print some infos for each host
    for host in &hosts {
        match &host.stats {
            Some(stats) => log::info!(
                "{:>25} ({:>2} IPs): {}",
                host.hostname,
                host.ips_v4.len(),
                stats.format_millis()
            ),
            None => log::info!("{:>25} ({:>2} IPs):", host.hostname, host.ips_v4.len()),
        }
    }

    let archive_folder =
        util::env_var_parse("CONFIG_FOLDER", Some(PathBuf::from(DEFAULT_CONFIG_FOLDER))).unwrap();

    // create the directory where the configs will be saved if it doesn't exist yet
    if !tokio::fs::metadata(&archive_folder)
        .await
        .map_or(false, |metadata| metadata.is_dir())
    {
        tokio::fs::create_dir(&archive_folder)
            .await
            .with_context(|| {
                format!(
                    "create config folder at {}",
                    archive_folder.to_str().unwrap_or_default()
                )
            })?;
    }

    let mut archive_path = archive_folder.clone();
    archive_path.push(format!(
        "cs-wg-{}.zip",
        chrono::Local::now().format("%Y-%m-%d-%H-%M-%S")
    ));

    let mut archive_file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&archive_path)
        .await
        .with_context(|| {
            format!(
                "open archive file for writing at {}",
                archive_path.to_str().unwrap_or_default()
            )
        })?;

    // assemble the zip archive in memory
    let mut archive_buffer = Vec::<u8>::with_capacity(4096);
    let mut archive_writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive_buffer));
    let archive_options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // save the configs
    for host in &hosts {
        let filename = format!(
            "cs-{:04}ms-{:02}ips-{}.conf",
            host.stats
                .as_ref()
                .map(|stats| (stats.average * 1e3).round() as u64)
                .unwrap_or(9999),
            host.ips_v4.len(),
            host.hostname.strip_suffix(CRYPTOSTORM_SUFFIX).unwrap()
        );

        let config = wireguard::Config::from_env(&host.hostname, &host.public_key)
            .context("build wireguard config")?
            .to_string();

        archive_writer
            .start_file(&filename, archive_options)
            .context("start writing file to archive in memory")?;

        archive_writer
            .write_all(config.as_bytes())
            .with_context(|| {
                format!(
                    "write contents of config for {} to archive buffer",
                    host.hostname
                )
            })?;
    }

    archive_writer.finish().unwrap();
    drop(archive_writer);

    // write the zip archive to disk
    archive_file
        .write_all(&archive_buffer)
        .await
        .context("write archive to disk")?;

    Ok((/* 👍 */))
}
