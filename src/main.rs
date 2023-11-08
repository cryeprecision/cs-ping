use std::env;
use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use trust_dns_resolver::config::*;
use trust_dns_resolver::{Name, TokioAsyncResolver};

mod host_lookup;
mod ping;
mod stats;

const DEFAULT_SCRIPT_URL: &str = "https://cryptostorm.is/wg_confgen.txt";

#[derive(Debug, Clone)]
struct Host {
    name: Arc<str>,
    ips: Vec<Ipv4Addr>,
}

/// - Download the script file
/// - Extract hostnames from the script file
/// - Resolve hostnames into IPv4 addresses
async fn get_hostnames() -> anyhow::Result<Vec<Host>> {
    let re = lazy_regex::regex!(r#"(?m)^"(\w+):(:?.*?)"$"#);
    let url = env::var("SCRIPT_URL").unwrap_or_else(|_| DEFAULT_SCRIPT_URL.to_string());

    // download the script file
    let resp = reqwest::get(url).await.context("download wg_confgen.txt")?;
    let text = resp.text().await.context("download wg_confgen.txt")?;

    // extract hostnames from the txt file
    let hosts = re
        .captures_iter(&text)
        .map(|capture| -> anyhow::Result<Arc<str>> {
            let location = capture.get(1).context("get capture group")?.as_str();
            Ok(Arc::from(format!("{}.cstorm.is.", location)))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let resolver =
        TokioAsyncResolver::tokio(ResolverConfig::cloudflare_tls(), ResolverOpts::default());
    let bar = indicatif::ProgressBar::new(hosts.len() as u64).with_message("Resolve Hostnames");

    // resolve hostnames into ip addresses
    let hosts = futures::stream::iter(hosts)
        .map(|hostname| {
            let resolver = &resolver;
            let bar = &bar;
            async move {
                let name = Name::from_utf8(hostname.as_ref()).context("convert host to a name")?;
                let resolved = resolver
                    .ipv4_lookup(name)
                    .await
                    .with_context(|| format!("resolve ip of {}", hostname));

                bar.inc(1);
                Ok::<_, anyhow::Error>(Host {
                    name: hostname.clone(),
                    ips: resolved?.into_iter().map(Ipv4Addr::from).collect(),
                })
            }
        })
        .buffer_unordered(5)
        .collect::<Vec<_>>()
        .await;

    bar.finish_and_clear();
    hosts.into_iter().collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let hosts = get_hostnames().await.unwrap();
    for host in hosts {
        println!("{}: {:?}", host.name, &host.ips[..host.ips.len().min(5)]);
    }
}
