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
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use rand::seq::SliceRandom as _;
use surge_ping::{PingIdentifier, PingSequence};
use tokio::sync::Semaphore;

use crate::context::Context;
use crate::stats::Stats;
use crate::{util, Host};

struct Job {
    ctx: Arc<Context>,
    host: Arc<Host>,
    host_retries: Arc<AtomicUsize>,
    host_sem: Arc<Semaphore>,
    ip: Ipv4Addr,
    ip_seq: Arc<AtomicU16>,
    index: usize,
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

        let mut jobs = Vec::new();
        let mut iter = stream::iter(hosts)
            .map(|host| from_host(Arc::clone(host), Arc::clone(&ctx)))
            .buffer_unordered(ctx.config.concurrent_dns);

        while let Some((host, result)) = iter.next().await {
            let ips = result?;
            anyhow::ensure!(
                !ips.is_empty(),
                "DNS query returned no IPs for {} (possibly IPv6 only)",
                host.location
            );

            let host_failed = Arc::new(AtomicUsize::new(0));
            let host_sem = Arc::new(Semaphore::new(ctx.config.concurrent_pings_per_host));
            jobs.reserve(ctx.config.pings_per_ip * ips.len());
            for ip in ips {
                let ip_seq = Arc::new(AtomicU16::new(0));
                jobs.extend((0..ctx.config.pings_per_ip).map(|index| Self {
                    ctx: Arc::clone(&ctx),
                    host: Arc::clone(&host),
                    host_retries: Arc::clone(&host_failed),
                    host_sem: Arc::clone(&host_sem),
                    ip,
                    ip_seq: Arc::clone(&ip_seq),
                    index,
                }));
            }

            anyhow::ensure!(
                jobs.len() <= u16::MAX as usize,
                "too many jobs: {} > {}",
                jobs.len(),
                ctx.config.concurrent_pings_total
            );
        }

        // randomize order of jobs for a delay between requests to ips of the same hostname
        jobs.shuffle(&mut rand::thread_rng());
        Ok(jobs)
    }

    pub async fn execute_jobs(jobs: &[Self]) -> anyhow::Result<Vec<(&Job, Duration)>> {
        async fn execute_job<'a>(job: &'a Job, bar: &ProgressBar) -> (&'a Job, Option<Duration>) {
            let ctx = job.ctx.as_ref();
            let retries = ctx.config.ping_retries;

            // check, if the host this job is for has had too many errors already
            if job.host_retries.load(Ordering::SeqCst) > retries {
                return (job, None);
            }
            // get a permit to ping this host and limit concurrent pings to the same host
            let _permit = job.host_sem.acquire().await.unwrap();

            let ping_id = ctx.next_ping_id.fetch_add(1, Ordering::SeqCst);
            let mut pinger = ctx
                .surge_ping
                .pinger(job.ip.into(), PingIdentifier(ping_id))
                .await;

            // set the timeout
            pinger.timeout(ctx.config.ping_timeout);

            let payload = [0u8; 56];
            while job.host_retries.load(Ordering::SeqCst) <= retries {
                let seq = PingSequence(job.ip_seq.fetch_add(1, Ordering::SeqCst));
                let err = match pinger.ping(seq, &payload).await {
                    Ok((_, duration)) => return (job, Some(duration)),
                    Err(err) => err,
                };
                // if this error is the one that is one too many, log it
                if job.host_retries.fetch_add(1, Ordering::SeqCst) == retries + 1 {
                    let Host { location, .. } = job.host.as_ref();
                    let Job { ip, .. } = job;
                    bar.suspend(|| log::warn!("Pinging {location} ({ip}) failed: {err}"));
                }
            }
            (job, None)
        }

        anyhow::ensure!(!jobs.is_empty(), "no jobs to execute");
        let ctx = Arc::clone(&jobs[0].ctx);

        let num_pings = jobs.len() as u64;
        let bar = util::styled_progress_bar(num_pings, "Ping IPs");

        let mut iter = stream::iter(jobs)
            .map(|job| execute_job(job, &bar))
            .buffer_unordered(ctx.config.concurrent_pings_total);

        let mut results = Vec::with_capacity(jobs.len());
        while let Some((job, result)) = iter.next().await {
            bar.inc(1);
            if let Some(duration) = result {
                results.push((job, duration));
            }
        }

        bar.finish_and_clear();
        Ok(results)
    }
}

pub struct HostStats {
    pub host: Arc<Host>,
    pub durations: HashMap<Ipv4Addr, Vec<Duration>>,
}

impl HostStats {
    fn collect_results(results: &Vec<(&Job, Duration)>) -> Vec<Self> {
        let mut durations: HashMap<Arc<Host>, HashMap<Ipv4Addr, Vec<Duration>>> = HashMap::new();
        let mut failed_hosts = HashSet::new();
        for (job, duration) in results {
            if job.host_retries.load(Ordering::SeqCst) > job.ctx.config.ping_retries {
                failed_hosts.insert(Arc::clone(&job.host));
                continue;
            }
            let host = durations.entry(Arc::clone(&job.host)).or_default();
            let ip = host.entry(job.ip).or_default();
            ip.push(*duration);
        }
        durations.retain(|host, _| !failed_hosts.contains(host));

        let mut stats = durations
            .into_iter()
            .map(|(host, durations)| HostStats { host, durations })
            .collect::<Vec<_>>();

        stats.sort_unstable_by_key(|host| Reverse((host.average_rtt_secs() * 1e6) as u64));
        stats
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
    let stats = HostStats::collect_results(&results);
    Ok(stats)
}
