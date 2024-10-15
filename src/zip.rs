use std::io::Write as _;
use std::sync::Arc;

use anyhow::Context as _;
use chrono::Local;
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

use crate::context::Context;
use crate::ping::HostStats;
use crate::util;

#[derive(Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
pub struct Asn {
    pub asn: String,
    pub domain: String,
    pub name: String,
}

pub async fn write_configs(ctx: Arc<Context>, host_stats: &[HostStats]) -> anyhow::Result<()> {
    let archive_folder = ctx.config.config_folder.canonicalize().with_context(|| {
        format!(
            "canonicalize config folder at {}",
            ctx.config.config_folder.display()
        )
    })?;

    // create the directory where the configs will be saved if it doesn't exist yet
    if !tokio::fs::metadata(&archive_folder)
        .await
        .map_or(false, |metadata| metadata.is_dir())
    {
        tokio::fs::create_dir(&archive_folder)
            .await
            .with_context(|| format!("create config folder at {}", archive_folder.display()))?;
    }

    let mut archive_path = archive_folder.to_path_buf();
    archive_path.push(format!(
        "cs-wg-{}.zip",
        Local::now().format("%Y-%m-%d_%H-%M-%S")
    ));

    let mut archive_file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&archive_path)
        .await
        .with_context(|| {
            format!(
                "open archive file for writing at {}",
                archive_path.display()
            )
        })?;

    // assemble the zip archive in memory
    let mut archive_buffer = Vec::<u8>::with_capacity(4096);
    let mut archive_writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive_buffer));
    let archive_options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // write the configs to the zip archive in memory
    for host_stats in host_stats {
        let HostStats { host, .. } = host_stats;
        let ips = host_stats.durations.keys().copied().collect::<Vec<_>>();

        let filename = format!(
            "cs-{:04}ms-{}-{:02}ips.conf",
            (host_stats.average_rtt_secs() * 1e3).round() as u64,
            util::trim_str(host.location.as_str(), 8),
            ips.len(),
        );

        archive_writer
            .start_file(&filename, archive_options)
            .context("start writing file to archive in memory")?;

        let config = ctx
            .config
            .wireguard
            .make_config(&host.to_endpoint(), &host.public_key);

        archive_writer
            .write_all(config.as_bytes())
            .with_context(|| {
                format!(
                    "write contents of config for {} to archive buffer",
                    host.location
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

    log::info!("wrote configs to {}", archive_path.display());
    Ok(())
}
