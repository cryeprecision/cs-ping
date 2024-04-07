# cs-ping

IP to ASN resolving is done through a [MaxMindDB](https://maxmind.github.io/MaxMind-DB/) file which can be downloaded [here](https://ipinfo.io/account/data-downloads) as 'Free IP to ASN'.

## Environment variables

### Generated WireGuard configs

- `CONFIG_CLIENT_ADDRESS`
- `CONFIG_CLIENT_ALLOWED_IPS`
- `CONFIG_CLIENT_DNS`
- `CONFIG_CLIENT_PRIVATE_KEY`
- `CONFIG_SERVER_PRESHARED_KEY`

### App behaviour

- `ASN_DB_PATH`: Path to the `asn.mmdb` file
- `CONCURRENT_DNS`: How many DNS queries are run concurrently
- `CONCURRENT_PINGS`: How many pings are run concurrently
- `CONFIG_FOLDER`: Generated configs will be saved to this folder
- `PING_RETRIES`: How often a ping will be retried before the server is considered down
- `PING_TIMEOUT_MS`: How long to wait for a ping reply
- `PINGS_PER_IP`: How many pings are run (and then averaged) for each IP
- `SCRIPT_URL`: URL to the `wg_confgen.txt`

## How to build

- Make sure you have a working Rust toolchain ([rustup](https://rustup.rs/))
- Clone the repo and `cd` into the cloned repo
- Run `cargo build --release` to compile or `cargo run --release` to compile and run
  - The binary will be in `target/release/`

## Notes

- Make sure you're not connected to any VPN server when running this or else the ping values will be meaningless.
- DNS queries to resolve domain names to IPs are run through cloudflare using DNS-over-TLS.
- Server IPs are pinged in a random order.

## Contributing

Fork the repo, change some stuff, make sure everything still works, fix any clippy lints (run `cargo clippy`) and submit a PR.
