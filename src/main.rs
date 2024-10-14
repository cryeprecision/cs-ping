#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use context::{Config, Context};
use serde::Deserialize;

mod context;
mod ping;
mod stats;
mod util;

const CRYPTOSTORM_SUFFIX: &str = ".cstorm.is";

#[derive(Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
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
    let hosts = Host::from_confgen(&confgen);

    let results = ping::ping_all_ips(Arc::clone(&ctx), &hosts).await?;
    for result in results {
        let location = result.host.location.as_str();
        let stats = result.average_stats();
        log::info!("{:^25}: {}", location, stats.format_millis());
    }

    // println!("{:#?}", results);

    Ok((/* 👍 */))
}
