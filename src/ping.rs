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
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use owo_colors::OwoColorize;
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use surge_ping::{PingIdentifier, PingSequence};
use tokio::sync::Semaphore;

use crate::context::Context;
use crate::stats::Stats;
use crate::{util, Host};

/// An iterator that yields [`Job`]s for each IP of each host.
///
/// First, a host is chosen uniformly at random. Then, an IP of that host is chosen uniformly at random.
/// The iterator yields a [`Job`] for that IP.
///
/// This allows pining all hosts and IPs in a random order without allocating all the jobs upfront.
struct JobIter {
    /// The gobal context shared by all jobs.
    ctx: Arc<Context>,
    /// The progress bar to update.
    bar: ProgressBar,
    /// The state of each host.
    hosts: Vec<JobIterHostState>,
    /// The indices of the hosts that still have IPs to ping.
    hosts_left: Vec<usize>,
    /// The random number generator used.
    rng: Xoshiro256PlusPlus,
}

struct JobIterHostState {
    /// The host to ping.
    host: Arc<Host>,
    /// A list of all IPs this host resolved to.
    ips: Vec<JobIterIpState>,
    /// The indices of the IPs that still have pings left.
    ips_left: Vec<usize>,
    /// The number of pings that failed for this host.
    host_retries: Arc<AtomicUsize>,
    /// A semaphore to limit the number of concurrent pings to this host.
    semaphore: Arc<Semaphore>,
}

struct JobIterIpState {
    /// The IP to ping.
    ip: Ipv4Addr,
    /// The sequence number of the next ping.
    ///
    /// This is atomic because multiple jobs can ping the same IP concurrently and
    /// the sequence number must also be incremented when a ping fails.
    ip_seq: Arc<AtomicU16>,
    /// The number of pings that have been sent to this IP.
    pings: usize,
}

impl JobIter {
    pub async fn new(
        ctx: Arc<Context>,
        hosts: Vec<Arc<Host>>,
        bar: ProgressBar,
    ) -> anyhow::Result<Self> {
        async fn resolve_hostname(
            ctx: Arc<Context>,
            host: Arc<Host>,
        ) -> (Arc<Host>, anyhow::Result<Vec<Ipv4Addr>>) {
            let result = ctx.resolve_hostname(&host.to_url()).await;
            (host, result)
        }

        let mut host_states = Vec::with_capacity(hosts.len());
        let mut iter = stream::iter(hosts)
            .map(|host| resolve_hostname(Arc::clone(&ctx), Arc::clone(&host)))
            .buffered(ctx.config.concurrent_dns);

        let mut num_ips = 0;
        while let Some((host, result)) = iter.next().await {
            let host_result =
                result.with_context(|| format!("resolve IPs for {}", host.location))?;

            num_ips += host_result.len();
            let mut ips = Vec::with_capacity(host_result.len());
            for ip in host_result {
                let ip_seq = Arc::new(AtomicU16::new(0));
                ips.push(JobIterIpState {
                    ip,
                    ip_seq,
                    pings: 0,
                });
            }

            let host_retries = Arc::new(AtomicUsize::new(0));
            let semaphore = Arc::new(Semaphore::new(ctx.config.concurrent_pings_per_host));
            let ips_left = (0..ips.len()).collect();
            host_states.push(JobIterHostState {
                host,
                ips,
                ips_left,
                host_retries,
                semaphore,
            });
        }
        drop(iter);

        let num_pings = num_ips * ctx.config.pings_per_ip;
        bar.set_length(num_pings as u64);

        let hosts_left = (0..host_states.len()).collect();
        Ok(Self {
            ctx,
            bar,
            hosts: host_states,
            hosts_left,
            rng: Xoshiro256PlusPlus::from_os_rng(),
        })
    }

    /// Panics if the host index is out of bounds.
    pub fn pings_left_for_host(&self, host_idx: usize) -> usize {
        let host_state = &self.hosts[host_idx];
        host_state
            .ips_left
            .iter()
            .map(|&ip_idx| {
                let ip_state = &host_state.ips[ip_idx];
                self.ctx.config.pings_per_ip - ip_state.pings
            })
            .sum()
    }

    pub fn pings_left(&self) -> usize {
        self.hosts_left
            .iter()
            .map(|&host_idx| self.pings_left_for_host(host_idx))
            .sum()
    }

    pub fn failed_hosts(&self) -> Vec<Arc<Host>> {
        self.hosts
            .iter()
            .filter(|host| host.host_retries.load(Ordering::SeqCst) > self.ctx.config.ping_retries)
            .map(|host| Arc::clone(&host.host))
            .collect()
    }
}

impl Iterator for JobIter {
    type Item = Job;

    fn next(&mut self) -> Option<Self::Item> {
        // The way this is implemented is a bit weird.
        // On one call to `next`, the number of pings left for an IP for example
        // is set to zero, but the IP is only removed from the list on the next call to `next`.

        while !self.hosts_left.is_empty() {
            // Choose a random host
            let host_left_idx = self.rng.random_range(0..self.hosts_left.len());
            let host_idx = self.hosts_left[host_left_idx];
            let host_state = &mut self.hosts[host_idx];

            // Don't yield any more jobs for this hose if the retries are exhausted
            if host_state.host_retries.load(Ordering::SeqCst) > self.ctx.config.ping_retries {
                self.hosts_left.remove(host_left_idx);
                self.bar.inc(self.pings_left_for_host(host_idx) as u64);
                continue;
            }

            while !host_state.ips_left.is_empty() {
                // Choose a random IP
                let ip_left_idx = self.rng.random_range(0..host_state.ips_left.len());
                let ip_idx = host_state.ips_left[ip_left_idx];
                let ip_state = &mut host_state.ips[ip_idx];

                // Don't yield any more jobs for this IP if the pings are exhausted
                if ip_state.pings == self.ctx.config.pings_per_ip {
                    host_state.ips_left.remove(ip_left_idx);
                    continue;
                }

                ip_state.pings += 1;
                return Some(Job {
                    ctx: Arc::clone(&self.ctx),
                    host: Arc::clone(&host_state.host),
                    host_retries: Arc::clone(&host_state.host_retries),
                    host_sem: Arc::clone(&host_state.semaphore),
                    ip: ip_state.ip,
                    ip_seq: Arc::clone(&ip_state.ip_seq),
                });
            }

            // If this host has no more IPs to ping, remove it from the list
            self.hosts_left.remove(host_left_idx);
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.pings_left()))
    }
}

struct Job {
    ctx: Arc<Context>,
    host: Arc<Host>,
    host_retries: Arc<AtomicUsize>,
    host_sem: Arc<Semaphore>,
    ip: Ipv4Addr,
    ip_seq: Arc<AtomicU16>,
}

impl Job {
    pub async fn execute_jobs(
        ctx: Arc<Context>,
        hosts: &[Arc<Host>],
    ) -> anyhow::Result<Vec<HostStats>> {
        async fn execute_job(job: Job, bar: &ProgressBar) -> (Job, Option<Duration>) {
            let ctx = job.ctx.as_ref();
            let retries = ctx.config.ping_retries;

            // get a permit to ping this host and limit concurrent pings to the same host
            let host_sem = Arc::clone(&job.host_sem);
            let _permit = host_sem.acquire().await.unwrap();

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
                if job.host_retries.fetch_add(1, Ordering::SeqCst) == retries {
                    bar.suspend(|| {
                        log::warn!(
                            "Pinging {} {} failed too many times: {}",
                            job.host.location.bright_red(),
                            format!("({})", job.ip).bright_black(),
                            err.bright_black(),
                        )
                    });
                }
            }
            (job, None)
        }

        let bar = util::styled_progress_bar(0, "Ping IPs");
        let mut job_iter = JobIter::new(Arc::clone(&ctx), hosts.to_vec(), bar.clone()).await?;

        let mut durations: HashMap<Arc<Host>, HashMap<Ipv4Addr, Vec<Duration>>> =
            HashMap::with_capacity(hosts.len());

        let mut iter = stream::iter(&mut job_iter)
            .map(|job| execute_job(job, &bar))
            .buffer_unordered(ctx.config.concurrent_pings_total);

        while let Some((job, result)) = iter.next().await {
            bar.inc(1);
            let Some(duration) = result else {
                continue;
            };

            let host = durations.entry(Arc::clone(&job.host)).or_default();
            host.entry(job.ip).or_default().push(duration);
        }
        drop(iter);
        bar.abandon();

        let failed_hosts = job_iter.failed_hosts();
        durations.retain(|host, _| !failed_hosts.contains(host));

        let mut stats = durations
            .into_iter()
            .map(|(host, durations)| HostStats { host, durations })
            .collect::<Vec<_>>();

        stats.sort_unstable_by_key(|host| Reverse((host.average_rtt_secs() * 1e6) as u64));
        Ok(stats)
    }
}

pub struct HostStats {
    pub host: Arc<Host>,
    pub durations: HashMap<Ipv4Addr, Vec<Duration>>,
}

impl HostStats {
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

    pub fn stats_for_ip(&self, ip: Ipv4Addr) -> Stats {
        let durations = self.durations.get(&ip).unwrap();
        Stats::from_durations(durations.iter().copied()).unwrap()
    }
}

pub async fn ping_all_ips(
    ctx: Arc<Context>,
    hosts: &[Arc<Host>],
) -> anyhow::Result<Vec<HostStats>> {
    Job::execute_jobs(Arc::clone(&ctx), hosts).await
}
