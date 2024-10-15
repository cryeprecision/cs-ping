#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use context::{Config, Context};

mod context;
mod ping;
mod stats;
mod util;
mod zip;

const CRYPTOSTORM_SUFFIX: &str = ".cstorm.is";

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

#[tokio::main(flavor = "current_thread")]
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
    for result in results {
        let location = result.host.location.as_str();
        let stats = result.stats();
        log::info!("[{:^25}] {}", location, stats.format_millis());
    }

    Ok((/* 👍 */))
}
