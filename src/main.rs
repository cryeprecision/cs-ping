mod ping;
mod stats;

use ping::JobRunner;

const SCRIPT_URL: &str = "https://cryptostorm.is/wg_confgen.txt";

/// Downloads the script-file and extracts all server locations
async fn get_hostnames() -> Vec<String> {
    let re = regex::Regex::new("(?m)^\"(\\w+):(:?.*?)\"$").expect("parse regex");
    let resp = reqwest::get(SCRIPT_URL).await.expect("download script");
    let text = resp.text().await.expect("get response content");

    return re
        .captures_iter(&text)
        .map(|c| {
            let mut s = c[1].to_string();
            s.push_str(".cstorm.is:443");
            return s;
        })
        .collect();
}

pub async fn ping_all_ips() {
    let results = JobRunner::new(get_hostnames().await)
        .waiting_to_resolved()
        .resolved_to_pinged(None, None)
        .await
        .finalize();

    for (location, result) in results.into_iter() {
        match result {
            Some(stats) => println!("{:^30} -> {}", location, stats),
            None => println!("{:^30} -> TIMEOUT", location),
        }
    }
}

fn main() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(ping_all_ips());
}
