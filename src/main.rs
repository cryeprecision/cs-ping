use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
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

const CRYPTOSTORM_SUFFIX: &str = ".cstorm.is";

const DEFAULT_SCRIPT_URL: &str = "https://cryptostorm.is/wg_confgen.txt";
const DEFAULT_CONCURRENT_DNS: usize = 1;
const DEFAULT_CONCURRENT_PINGS: usize = 5;
const DEFAULT_PINGS_PER_IP: usize = 5;
const DEFAULT_PING_RETRIES: usize = 3;
const DEFAULT_PING_TIMEOUT_MS: u64 = 2000;
const DEFAULT_CONFIG_FOLDER: &str = "./configs/";
const DEFAULT_ASN_DB_PATH: &str = "./asn.mmdb";

const TEMPLATE_CLIENT_PRIVATE_KEY: &str = "{{ PRIVATE_KEY }}";
const TEMPLATE_CLIENT_ADDRESS: &str = "{{ ADDRESS }}";
const TEMPLATE_CLIENT_DNS: &str = "{{ DNS }}";
const TEMPLATE_CLIENT_ALLOWED_IPS: &str = "{{ ALLOWED_IPS }}";
const TEMPLATE_SERVER_PRESHARED_KEY: &str = "{{ PRESHARED_KEY }}";
const TEMPLATE_SERVER_PUBLIC_KEY: &str = "{{ PUBLIC_KEY }}";
const TEMPLATE_SERVER_ENDPOINT: &str = "{{ ENDPOINT }}";

const TEMPLATE: &str = "\
[Interface]
PrivateKey = {{ PRIVATE_KEY }}
Address = {{ ADDRESS }}
DNS = {{ DNS }}

[Peer]
Presharedkey = {{ PRESHARED_KEY }}
PublicKey = {{ PUBLIC_KEY }}
Endpoint = {{ ENDPOINT }}
AllowedIPs = {{ ALLOWED_IPS }}
PersistentKeepalive = 25
";

#[derive(serde::Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
struct Asn {
    asn: String,
    domain: String,
    name: String,
}

fn format_asn_set(asn_set: &HashSet<Asn>) -> String {
    fn domain_name(input: &str) -> &str {
        if let Some((_, domain_name)) = lazy_regex::regex_captures!(r#"([\w_\-]+)\.\w+$"#, input) {
            domain_name
        } else {
            input
        }
    }

    let mut asn_list = asn_set.iter().collect::<Vec<_>>();
    asn_list.sort_unstable_by_key(|asn| asn.domain.as_str());

    let mut buf = String::new();
    let mut iter = asn_list.iter();
    if let Some(asn) = iter.next() {
        buf.push_str(domain_name(&asn.domain));
        for asn in iter {
            buf.push('-');
            buf.push_str(domain_name(&asn.domain));
        }
    }
    buf
}

#[derive(Debug, Clone)]
struct Config {
    /// Where to download the script from. Servers and their corresponding public keys are
    /// extracted from this file.
    script_url: Arc<str>,
    /// How many concurrent DNS requests are made.
    concurrent_dns: usize,
    /// How many concurrent pings are sent.
    concurrent_pings: usize,
    /// How often each IP address is pinged.
    pings_per_ip: usize,
    /// How often a ping to an IP address is retried if it fails.
    ping_retries: usize,
    /// Timeout of each ping.
    ping_timeout: Duration,
    /// The folder where the config ZIP will be saved to.
    config_folder: Arc<str>,
    /// Database to get ASN from IP
    asn_db: Arc<[u8]>,
    /// WireGuard specific config for building the individual configs for each server.
    wireguard: WireguardConfig,
}

#[derive(Debug, Clone)]
struct WireguardConfig {
    /// Private key for the client.
    client_private_key: Arc<str>,
    /// IP address for the client.
    client_address: Arc<str>,
    /// Address of the DNS server WireGuard will use.
    client_dns: Arc<str>,
    /// List of subnets that will be routed through the tunnel.
    client_allowed_ips: Arc<str>,
    /// Preshared key with the WireGuard server.
    server_preshared_key: Arc<str>,
}

impl WireguardConfig {
    /// Try to load the config from environment variables
    pub fn from_env() -> anyhow::Result<WireguardConfig> {
        Ok(WireguardConfig {
            client_private_key: util::env_var("CONFIG_CLIENT_PRIVATE_KEY", None)
                .context("env var CONFIG_CLIENT_PRIVATE_KEY missing")?
                .into(),
            client_address: util::env_var("CONFIG_CLIENT_ADDRESS", None)
                .context("env var CONFIG_CLIENT_ADDRESS missing")?
                .into(),
            client_dns: util::env_var("CONFIG_CLIENT_DNS", None)
                .context("env var CONFIG_CLIENT_DNS missing")?
                .into(),
            client_allowed_ips: util::env_var("CONFIG_CLIENT_ALLOWED_IPS", None)
                .context("env var CONFIG_CLIENT_ALLOWED_IPS missing")?
                .into(),
            server_preshared_key: util::env_var("CONFIG_SERVER_PRESHARED_KEY", None)
                .context("env var CONFIG_SERVER_PRESHARED_KEY missing")?
                .into(),
        })
    }

    /// Create a WireGuard configuration file
    pub fn make_config(&self, server_endpoint: &str, server_public_key: &str) -> String {
        util::replace_all(
            TEMPLATE.to_string(),
            &[
                (TEMPLATE_CLIENT_PRIVATE_KEY, &self.client_private_key),
                (TEMPLATE_CLIENT_ADDRESS, &self.client_address),
                (TEMPLATE_CLIENT_DNS, &self.client_dns),
                (TEMPLATE_CLIENT_ALLOWED_IPS, &self.client_allowed_ips),
                (TEMPLATE_SERVER_PRESHARED_KEY, &self.server_preshared_key),
                (TEMPLATE_SERVER_PUBLIC_KEY, server_public_key),
                (TEMPLATE_SERVER_ENDPOINT, server_endpoint),
            ],
        )
    }
}

impl Config {
    /// Try to load the config from environment variables
    pub async fn from_env() -> anyhow::Result<Config> {
        Ok(Config {
            script_url: util::env_var("SCRIPT_URL", Some(DEFAULT_SCRIPT_URL))
                .context("env var SCRIPT_URL missing")?
                .into(),
            concurrent_dns: util::env_var_parse("CONCURRENT_DNS", Some(DEFAULT_CONCURRENT_DNS))
                .context("env var CONCURRENT_DNS missing")?,
            concurrent_pings: util::env_var_parse(
                "CONCURRENT_PINGS",
                Some(DEFAULT_CONCURRENT_PINGS),
            )
            .context("env var CONCURRENT_PINGS missing")?,
            pings_per_ip: util::env_var_parse("PINGS_PER_IP", Some(DEFAULT_PINGS_PER_IP))
                .context("env var PINGS_PER_IP missing")?,
            ping_retries: util::env_var_parse("PING_RETRIES", Some(DEFAULT_PING_RETRIES))
                .context("env var PING_RETRIES missing")?,
            ping_timeout: Duration::from_millis(
                util::env_var_parse("PING_TIMEOUT_MS", Some(DEFAULT_PING_TIMEOUT_MS))
                    .context("env var PING_TIMEOUT_MS missing")?,
            ),
            config_folder: util::env_var("CONFIG_FOLDER", Some(DEFAULT_CONFIG_FOLDER))
                .context("env var CONFIG_FOLDER missing")?
                .into(),
            asn_db: util::env_var_read_file("ASN_DB_PATH", Some(DEFAULT_ASN_DB_PATH))
                .await
                .context("env var ASN_DB_PATH missing or couldn't read file")?
                .into(),
            wireguard: WireguardConfig::from_env()?,
        })
    }
}

#[derive(Debug, Clone)]
struct Host {
    hostname: Arc<str>,
    public_key: Arc<str>,
    ips_v4: Arc<[Ipv4Addr]>,
    duration: Duration,
    stats: Option<Stats>,
    asn_set: Option<HashSet<Asn>>,
}

/// - Download the script file
/// - Extract hostnames from the script file
/// - Resolve hostnames into IPv4 addresses
async fn get_hostnames(cfg: &Config) -> anyhow::Result<Vec<Host>> {
    let re = lazy_regex::regex!(r#"(?m)^"(\w+):(.*?)"$"#);

    // download the script file
    let resp = reqwest::get(cfg.script_url.as_ref())
        .await
        .context("download wg_confgen.txt")?
        .error_for_status()
        .context("unexpected response status code")?;
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
                    asn_set: None,
                })
            }
        })
        .buffer_unordered(cfg.concurrent_dns)
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

    async fn try_resolved_to_pinged(&mut self, cfg: &Config, bar: &ProgressBar) {
        let JobState::Resolved { client } = &self.state else {
            self.to_error(anyhow::anyhow!("illegal state transition"));
            return;
        };

        let mut pinger = client
            .pinger(self.ip.into(), PingIdentifier(rand::thread_rng().gen()))
            .await;

        // set the timeout
        pinger.timeout(cfg.ping_timeout);

        let payload = [0u8; 32];
        let mut results = Vec::new();

        let mut ping_retries = cfg.ping_retries;

        'outer: for seq in 0..cfg.pings_per_ip {
            // try to ping the ip with some retries
            let error: anyhow::Error = loop {
                match pinger.ping(PingSequence(seq as u16), &payload).await {
                    Ok((_, duration)) => {
                        // ping succeeded
                        results.push(duration);
                        bar.inc(1);

                        // done pinging
                        if results.len() == cfg.pings_per_ip {
                            break 'outer;
                        }

                        // need more pings
                        continue 'outer;
                    }
                    // ping failed but we still have retries left
                    Err(_) if ping_retries > 0 => ping_retries -= 1,
                    // ping failed and we ran out of retries
                    Err(err) => break err.into(),
                };
            };

            // error case - out of retries
            self.to_error(error.context(format!(
                "out of retries to ping {} of {} (seq: {})",
                self.ip, self.host.hostname, seq
            )));
            // accomodate for pings that wont be sent so the bar doesn't ent up partially full
            bar.inc((cfg.pings_per_ip - seq) as u64);

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
    // initialize the logger implementation
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(log::LevelFilter::Info)
        .init();

    // try to load a dotenv file
    match dotenv::dotenv() {
        Ok(path) => log::info!(
            "loaded .env file from {}",
            path.to_str().unwrap_or_default()
        ),
        Err(err) => log::warn!("couldn't load .env file: {}", err),
    };

    let config = Config::from_env()
        .await
        .context("load config from environment variables")?;

    let client =
        surge_ping::Client::new(&surge_ping::Config::default()).context("create ping client")?;
    let mut hosts = get_hostnames(&config).await.context("get ping targets")?;

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
    let bar = util::styled_progress_bar((jobs.len() * config.pings_per_ip) as u64, "Ping IPs");
    let _ = futures::stream::iter(&mut jobs)
        .map(|job| job.try_resolved_to_pinged(&config, &bar))
        .buffer_unordered(config.concurrent_pings)
        .collect::<Vec<()>>()
        .await;
    bar.finish_and_clear();

    // collect the asn names for each hostname
    let mut asn_names = HashMap::<Arc<str>, HashSet<Asn>>::new();
    let asn_db_reader = maxminddb::Reader::from_source(config.asn_db.as_ref())
        .context("malformed asn database")
        .unwrap();

    jobs.iter().for_each(|job| {
        if matches!(job.state, JobState::Err { .. }) {
            return;
        }

        let asn: Asn = asn_db_reader
            .lookup(job.ip.into())
            .context("couldn't find ip address in database")
            .unwrap();

        asn_names
            .entry(job.host.hostname.clone())
            .or_default()
            .insert(asn);
    });
    drop(asn_db_reader);

    // collect the stats of each ip into the corresponding hostname
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

    // add the asn set to the corresponding host
    hosts.iter_mut().for_each(|host| {
        match asn_names.remove_entry(&host.hostname) {
            None => (),
            Some((_, asn_set)) => host.asn_set = Some(asn_set),
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
                "{:>25} ({:>2} IPs): {} (ASN: {:?})",
                host.hostname,
                host.ips_v4.len(),
                stats.format_millis(),
                host.asn_set.as_ref().unwrap(),
            ),
            None => log::info!("{:>25} ({:>2} IPs):", host.hostname, host.ips_v4.len()),
        }
    }

    let archive_folder = PathBuf::from(config.config_folder.as_ref());

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
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // write the configs to the zip archive in memory
    for host in &hosts {
        let asn_formatted = match host.asn_set.as_ref() {
            Some(asn_set) => util::trim_string(format_asn_set(asn_set), 7),
            None => "unknown".to_string(),
        };
        let filename = format!(
            "cs-{:04}ms-{}-{}-{:02}ips.conf",
            host.stats
                .as_ref()
                .map(|stats| (stats.average * 1e3).round() as u64)
                .unwrap_or(9999),
            util::trim_str(host.hostname.strip_suffix(CRYPTOSTORM_SUFFIX).unwrap(), 8),
            &asn_formatted,
            host.ips_v4.len(),
        );

        let config = config
            .wireguard
            .make_config(&format!("{}:443", host.hostname), &host.public_key);

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

    archive_writer
        .finish()
        .context("finish constructing the zip archive in memory")?;

    // write the zip archive to disk
    archive_file
        .write_all(&archive_buffer)
        .await
        .context("write archive to disk")?;

    log::info!(
        "wrote configs to {}",
        tokio::fs::canonicalize(&archive_path)
            .await
            .unwrap_or_else(|_| archive_path.clone())
            .to_str()
            .and_then(|path| path.strip_prefix(r#"\\?\"#))
            .unwrap_or_default()
    );

    Ok((/* 👍 */))
}
