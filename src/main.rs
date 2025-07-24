use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use context::{Config, Context};
use owo_colors::OwoColorize;
use zip::Asn;

mod context;
mod ping;
mod stats;
mod util;
mod zip;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Host {
    pub location: String,
    pub public_key: String,
}

impl Host {
    pub fn from_confgen(confgen: &str) -> Vec<Arc<Self>> {
        // extract hostnames and corresponding public keys from the txt file
        lazy_regex::regex!(r#"(?m)^"(\w+):(.*?)"$"#)
            .captures_iter(confgen)
            .map(|capture| {
                Arc::new(Self {
                    location: capture.get(1).unwrap().as_str().to_string(),
                    public_key: capture.get(2).unwrap().as_str().to_string(),
                })
            })
            .collect()
    }

    pub fn to_url(&self) -> String {
        format!("{}.cstorm.is", self.location)
    }

    pub fn to_endpoint(&self) -> String {
        format!("{}.cstorm.is:443", self.location)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // initialize the logger implementation
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(log::LevelFilter::Info)
        .init();

    // read the config and initialize the context
    let config = Config::read(Path::new("./config.json")).context("load config")?;
    let ctx = Arc::new(Context::new(config).context("initialize context")?);

    // get the hostnames and their corresponding public keys
    let confgen = ctx.download_confgen().await.context("download confgen")?;
    let mut hosts = Host::from_confgen(&confgen);
    hosts.retain(|host| !ctx.config.location_blacklist.contains(&host.location));

    let results = ping::ping_all_ips(Arc::clone(&ctx), &hosts).await?;

    zip::write_configs(Arc::clone(&ctx), &results)
        .await
        .context("write configs")?;

    for result in &results {
        let location = result.host.location.as_str();
        let stats = result.stats();

        log::info!("[{:^25}] {}", location.bold(), stats.format_millis());

        if let Some(mmdb) = ctx.config.asn_mmdb.as_ref() {
            let mut ips = result.durations.keys().copied().collect::<Vec<_>>();
            ips.sort_unstable();

            for ip in ips {
                let Ok(Some(asn)) = mmdb.lookup::<Asn>(ip.into()) else {
                    log::warn!("ASN info not found in DB for IP: {ip}");
                    continue;
                };
                let stats = result.stats_for_ip(ip);

                log::info!(
                    "{}",
                    format!(
                        "[{:^25}] {} {}",
                        location,
                        stats.format_millis(),
                        format!(
                            "{domain} ({name}) {ip}",
                            domain = asn.domain,
                            name = asn.name,
                            ip = ip,
                        )
                        .bright_black(),
                    )
                    .bright_black()
                );
            }
        }
    }

    Ok((/* 👍 */))
}
