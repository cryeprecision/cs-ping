use std::cmp::Reverse;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use rand::seq::SliceRandom as _;

use crate::context::Context;
use crate::stats::Stats;
use crate::{util, Host};

struct Job {
    host: Arc<Host>,
    ip: Ipv4Addr,
    ctx: Arc<Context>,
}

impl Job {
    pub async fn from_hosts(hosts: &[Arc<Host>], ctx: Arc<Context>) -> anyhow::Result<Vec<Self>> {
        async fn from_host(
            host: Arc<Host>,
            ctx: Arc<Context>,
        ) -> (Arc<Host>, anyhow::Result<Vec<Ipv4Addr>>) {
            let result = ctx.resolve_hostname(&host.to_url()).await;
            (host, result)
        }

        let mut jobs = Vec::with_capacity(hosts.len());
        let mut iter = stream::iter(hosts)
            .map(|host| from_host(Arc::clone(host), Arc::clone(&ctx)))
            .buffer_unordered(ctx.config.concurrent_dns);

        while let Some((host, result)) = iter.next().await {
            let ips = result?;
            anyhow::ensure!(
                !ips.is_empty(),
                "DNS query returned no IPs for {} (possibly IPv6 only)",
                host.location.as_str()
            );
            jobs.extend(ips.into_iter().map(|ip| Self {
                host: Arc::clone(&host),
                ip,
                ctx: Arc::clone(&ctx),
            }));
        }

        // randomize order of jobs for a delay between requests to ips of the same hostname
        jobs.shuffle(&mut rand::thread_rng());
        Ok(jobs)
    }

    pub async fn execute_jobs(jobs: &[Self]) -> anyhow::Result<HashMap<Ipv4Addr, Vec<Duration>>> {
        async fn execute_job(job: &Job, bar: ProgressBar) -> (&Job, anyhow::Result<Vec<Duration>>) {
            let result = job.ctx.ping_ip(job.ip).await;
            bar.inc(1);
            (job, result)
        }

        anyhow::ensure!(!jobs.is_empty(), "no jobs to execute");
        let ctx = Arc::clone(&jobs[0].ctx);

        let bar = util::styled_progress_bar(jobs.len() as u64, "Ping IPs");
        let mut iter = stream::iter(jobs)
            .map(|job| execute_job(job, bar.clone()))
            .buffer_unordered(ctx.config.concurrent_pings);

        let mut results = HashMap::new();
        while let Some((job, result)) = iter.next().await {
            match result {
                Err(err) => {
                    bar.suspend(|| {
                        let Job { ip, host, .. } = job;
                        let Host { location, .. } = host.as_ref();
                        log::warn!("Pinging {location} ({ip}) failed: {err}",)
                    });
                }
                Ok(result) => {
                    results.insert(job.ip, result);
                }
            }
        }

        bar.finish_and_clear();
        Ok(results)
    }
}

pub struct HostStats {
    pub host: Arc<Host>,
    pub ips: Vec<Ipv4Addr>,
    pub ip_stats: HashMap<Ipv4Addr, Stats>,
}

impl HostStats {
    fn collect_results(
        jobs: &[Job],
        results: &HashMap<Ipv4Addr, Vec<Duration>>,
        hosts: &[Arc<Host>],
    ) -> Vec<Self> {
        let ip_to_host = jobs
            .iter()
            .map(|job| (job.ip, Arc::clone(&job.host)))
            .collect::<HashMap<_, _>>();

        let mut results = hosts
            .iter()
            .filter_map(|host| {
                let host_ips = jobs
                    .iter()
                    .filter_map(|job| (ip_to_host.get(&job.ip) == Some(host)).then_some(job.ip))
                    .collect::<Vec<_>>();

                let durations = host_ips
                    .iter()
                    .filter_map(|&ip| {
                        let durations = results.get(&ip)?;
                        let stats = Stats::from_durations(durations.iter().copied())?;
                        Some((ip, stats))
                    })
                    .collect::<HashMap<_, _>>();

                if host_ips.len() != durations.len() {
                    log::warn!(
                        "{}/{} ips unreachable for {}",
                        host_ips.len() - durations.len(),
                        host_ips.len(),
                        host.location.as_str(),
                    );
                    return None;
                }

                Some(Self {
                    host: Arc::clone(host),
                    ips: host_ips,
                    ip_stats: durations,
                })
            })
            .collect::<Vec<_>>();

        results.sort_unstable_by_key(|host| Reverse((host.average_rtt_secs() * 1e6) as u64));
        results
    }

    fn average_rtt_secs(&self) -> f64 {
        assert!(
            !self.ip_stats.is_empty(),
            "no stats for {}",
            self.host.location.as_str()
        );
        let sum = self
            .ip_stats
            .values()
            .map(|stats| stats.average)
            .sum::<f64>();
        let count = self.ip_stats.len() as f64;

        sum / count
    }

    pub fn average_stats(&self) -> Stats {
        assert!(
            !self.ip_stats.is_empty(),
            "no stats for {}",
            self.host.location.as_str()
        );

        if self.ip_stats.len() == 1 {
            return self.ip_stats.values().next().unwrap().clone();
        }

        Stats::from_durations(
            self.ip_stats
                .values()
                .map(|stats| Duration::from_secs_f64(stats.average)),
        )
        .unwrap()
    }
}

pub async fn ping_all_ips(
    ctx: Arc<Context>,
    hosts: &[Arc<Host>],
) -> anyhow::Result<Vec<HostStats>> {
    let jobs = Job::from_hosts(hosts, Arc::clone(&ctx)).await?;
    let results = Job::execute_jobs(&jobs).await?;
    let stats = HostStats::collect_results(&jobs, &results, hosts);
    Ok(stats)
}
