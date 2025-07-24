use std::collections::HashSet;
use std::fmt::Write as _;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{TimeZone as _, Utc};
use maxminddb::Reader;
use reqwest::tls;
use serde::{Deserialize, Deserializer};
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::{Name, TokioAsyncResolver};

pub struct Context {
    pub resolver: TokioAsyncResolver,
    pub reqwest: reqwest::Client,
    pub surge_ping: surge_ping::Client,
    pub next_ping_id: AtomicU16,
    pub config: Config,
}

impl Context {
    pub fn new(config: Config) -> anyhow::Result<Context> {
        Ok(Context {
            resolver: TokioAsyncResolver::tokio(
                ResolverConfig::cloudflare_tls(),
                ResolverOpts::default(),
            ),
            reqwest: reqwest::Client::builder()
                .min_tls_version(tls::Version::TLS_1_2)
                .build()
                .context("create reqwest client")?,
            surge_ping: surge_ping::Client::new(&surge_ping::Config::new())
                .context("create surge_ping client")?,
            next_ping_id: AtomicU16::new(0),
            config,
        })
    }

    pub async fn download_confgen(&self) -> anyhow::Result<String> {
        let script_url = self.config.script_url.as_str();
        self.reqwest
            .get(script_url)
            .send()
            .await
            .with_context(|| format!("download script from {script_url}"))?
            .error_for_status()
            .with_context(|| format!("unexpected status code from {script_url}"))?
            .text()
            .await
            .with_context(|| format!("read script from {script_url}"))
    }

    pub async fn resolve_hostname(&self, hostname: &str) -> anyhow::Result<Vec<Ipv4Addr>> {
        let name = Name::from_utf8(hostname)
            .with_context(|| format!("convert host {hostname} to a name"))?;

        self.resolver
            .ipv4_lookup(name)
            .await
            .with_context(|| format!("resolve ip of {hostname}"))
            .map(|ips| ips.into_iter().map(Ipv4Addr::from).collect::<Vec<_>>())
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Where to download the script from. Servers and their corresponding public keys are
    /// extracted from this file.
    pub script_url: String,
    /// How many concurrent DNS requests are made.
    pub concurrent_dns: usize,
    /// How many concurrent pings are sent.
    pub concurrent_pings_total: usize,
    pub concurrent_pings_per_host: usize,
    /// How often each IP address is pinged.
    pub pings_per_ip: usize,
    /// How often a ping to an IP address is retried if it fails.
    pub ping_retries: usize,
    /// List of locations to ignore
    pub location_blacklist: HashSet<String>,
    /// Timeout of each ping.
    #[serde(deserialize_with = "Config::deserialize_duration_ms")]
    #[serde(rename = "ping_timeout_ms")]
    pub ping_timeout: Duration,
    /// The folder where the config ZIP will be saved to.
    pub config_folder: PathBuf,
    /// Database to get ASN from IP
    #[serde(deserialize_with = "Config::deserialize_mmdb")]
    #[serde(rename = "asn_db_path")]
    pub asn_mmdb: Option<Reader<Vec<u8>>>,
    /// WireGuard specific config for building the individual configs for each server.
    pub wireguard: WireguardConfig,
}

impl Config {
    fn deserialize_mmdb<'de, D>(deserializer: D) -> Result<Option<Reader<Vec<u8>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(path) = Option::<PathBuf>::deserialize(deserializer)? else {
            return Ok(None);
        };

        let content = std::fs::read(&path)
            .with_context(|| format!("read mmdb file at {}", path.display()))
            .map_err(serde::de::Error::custom)?;

        let reader = Reader::from_source(content)
            .context("create mmdb reader")
            .map_err(serde::de::Error::custom)?;

        let build_epoch = i64::try_from(reader.metadata.build_epoch)
            .context("convert build epoch to i64")
            .map_err(serde::de::Error::custom)?;
        let build_date = Utc
            .timestamp_opt(build_epoch, 0)
            .earliest()
            .context("convert build epoch to date")
            .map_err(serde::de::Error::custom)?;

        let mmdb_age = Utc::now() - build_date;
        if mmdb_age > chrono::Duration::days(30) {
            log::warn!("mmdb file is older than 30 days ({mmdb_age}), consider updating it",);
        }

        Ok(Some(reader))
    }

    fn deserialize_duration_ms<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ms = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(ms))
    }

    pub fn read(path: &Path) -> anyhow::Result<Config> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .with_context(|| format!("open config file at {}", path.display()))?;

        let reader = std::io::BufReader::new(file);
        let config = serde_json::from_reader(reader).context("parse config file")?;
        Ok(config)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WireguardConfig {
    /// Private key for the client.
    pub client_private_key: String,
    /// IP address for the client.
    pub client_address: String,
    /// Address of the DNS server WireGuard will use.
    pub client_dns: String,
    /// List of subnets that will be routed through the tunnel.
    pub client_allowed_ips: String,
    /// Preshared key with the WireGuard server.
    pub server_preshared_key: Option<String>,
    /// Commands executed before the interface is brought up.
    pub pre_up: Vec<String>,
    /// Commands executed after the interface is brought up.
    pub post_up: Vec<String>,
    /// Commands executed before the interface is brought down.
    pub pre_down: Vec<String>,
    /// Commands executed after the interface is brought down.
    pub post_down: Vec<String>,
    /// Persistent keepalive
    pub persistent_keepalive: Option<String>,
}

impl WireguardConfig {
    /// Create a WireGuard configuration file
    pub fn make_config(&self, server_endpoint: &str, server_public_key: &str) -> String {
        let mut buffer = String::new();

        writeln!(buffer, "[Interface]").unwrap();
        writeln!(buffer, "PrivateKey = {}", self.client_private_key).unwrap();
        writeln!(buffer, "Address = {}", self.client_address).unwrap();
        writeln!(buffer, "DNS = {}", self.client_dns).unwrap();

        for pre_up in &self.pre_up {
            writeln!(buffer, "PreUp = {pre_up}").unwrap();
        }
        for post_up in &self.post_up {
            writeln!(buffer, "PostUp = {post_up}").unwrap();
        }
        for pre_down in &self.pre_down {
            writeln!(buffer, "PreDown = {pre_down}").unwrap();
        }
        for post_down in &self.post_down {
            writeln!(buffer, "PostDown = {post_down}").unwrap();
        }

        writeln!(buffer).unwrap();
        writeln!(buffer, "[Peer]").unwrap();
        writeln!(buffer, "PublicKey = {server_public_key}").unwrap();
        writeln!(buffer, "Endpoint = {server_endpoint}").unwrap();
        if let Some(preshared_key) = &self.server_preshared_key {
            writeln!(buffer, "PresharedKey = {preshared_key}").unwrap();
        }
        if let Some(persistent_keepalive) = &self.persistent_keepalive {
            writeln!(buffer, "PersistentKeepalive = {persistent_keepalive}").unwrap();
        }
        writeln!(buffer, "AllowedIPs = {}", self.client_allowed_ips).unwrap();

        buffer
    }
}
