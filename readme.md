# cs-ping

IP to ASN resolving is done through a [MaxMindDB](https://maxmind.github.io/MaxMind-DB/) file which can be downloaded [here](https://ipinfo.io/account/data-downloads) as 'Free IP to ASN'.

## App behaviour

- Fetch the list of servers from the `wg_confgen.txt` script
- Resolve each hostname into a list of IPs
- Each IP will get pinged `N` times
- Calculate the average RTT and stdandard deviation for each host
- Profit???

## Configuration

### App configuration

```jsonc
{
  // A link to the `wg_confgen.txt` file.
  "script_url": "https://example.com/wg_confgen.txt",
  // The number of concurrent DNS requests allowed when resolving hostnames.
  "concurrent_dns": 5,
  // The number of concurrent pings allowed overall.
  "concurrent_pings_total": 5,
  // The number of concurrent pings allowed per hostname.
  "concurrent_pings_per_host": 5,
  // The number of times each IP is pinged.
  "pings_per_ip": 50,
  // The number of times a ping to an IP of a host is allowed to fail before
  // the whole host is considered failed and excluded from the results.
  "ping_retries": 3,
  // How long to wait for a ping response before it is considered a timeout.
  "ping_timeout_ms": 500,
  // The directory where the zip file with all the configs is saved.
  "config_folder": "./configs/",
  // The path to the MMDB file to use for resolving IPs to ASNs.
  // If this is not wanted, set to `null`.
  "asn_db_path": "./asn.mmdb",
  // A list of locations to ignore. E.g., `mexico`.
  "location_blacklist": [ ... ],
  // The wireguard specific configuration (see below).
  "wireguard": { ... }
}
```

### Wireguard specific configuration

See [Some Unofficial WireGuard Documentation](https://github.com/pirate/wireguard-docs/blob/master/README.md#config-reference) and `WireguardConfig::make_config` in [`src/context.rs`](src/context.rs).

```jsonc
{
  "client_private_key": "...",
  "client_address": "255.255.255.255/32",
  "client_dns": "255.255.255.255",
  "client_allowed_ips": "0.0.0.0/0",
  "server_preshared_key": "...",
  "pre_up": [ ... ],
  "post_up": [ ... ],
  "pre_down": [ ... ],
  "post_down": [ ... ],
  "persistent_keepalive": "25"
}
```

## How to build

- Make sure you have a working Rust toolchain ([rustup](https://rustup.rs/))
- Clone the repo and `cd` into the cloned repo
- Run `cargo build --release` to compile or `cargo run --release` to compile and run
  - The binary will be in `target/release/`

## To do

- Nothing?

## Notes

- Make sure you're not connected to any VPN server when running this
- DNS queries to resolve domain names to IPs are run through cloudflare using DNS-over-TLS.
- Server IPs are pinged in a random order.

## Contributing

Fork the repo, change some stuff, make sure everything still works, fix any clippy lints (run `cargo clippy`) and submit a PR.
