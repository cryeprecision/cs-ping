//! Module for pinging hosts and collecting statistics.
//!
//! ## How It Works
//!
//! - [`Job::from_hosts`] resolves the hostnames into IPs and creates a [`Job`] for each IP.
//! - The vector of [`Jobs`](Job) is shuffled to randomize the order in which the IPs are pinged.
//! - [`Job::execute_jobs`] pings the IPs concurrently and collects the results.
//! - [`HostStats::collect_results`] groups the results by hostname and calculates the statistics.
//! - The statistics are sorted by average RTT and returned.

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
    pub durations: HashMap<Ipv4Addr, Vec<Duration>>,
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
                let ips = jobs
                    .iter()
                    .filter_map(|job| (ip_to_host.get(&job.ip) == Some(host)).then_some(job.ip))
                    .collect::<Vec<_>>();

                let durations = ips
                    .iter()
                    .filter_map(|&ip| Some((ip, results.get(&ip)?.clone())))
                    .collect::<HashMap<_, _>>();

                if ips.len() != durations.len() {
                    log::warn!(
                        "{}/{} ips unreachable for {}",
                        ips.len() - durations.len(),
                        ips.len(),
                        host.location.as_str(),
                    );
                    return None;
                }

                Some(Self {
                    host: Arc::clone(host),
                    ips,
                    durations,
                })
            })
            .collect::<Vec<_>>();

        results.sort_unstable_by_key(|host| Reverse((host.average_rtt_secs() * 1e6) as u64));
        results
    }

    pub fn average_rtt_secs(&self) -> f64 {
        assert!(
            !self.durations.is_empty(),
            "no durations for any ip of {}",
            self.host.location
        );

        let (sum, count) = self
            .durations
            .values()
            .fold((0.0, 0), |(sum, count), next| {
                (
                    sum + next.iter().map(Duration::as_secs_f64).sum::<f64>(),
                    count + next.len(),
                )
            });

        sum / (count as f64)
    }

    pub fn stats(&self) -> Stats {
        assert!(
            !self.durations.is_empty(),
            "no durations for any ip of {}",
            self.host.location
        );
        Stats::from_durations(self.durations.values().flatten().copied()).unwrap()
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
