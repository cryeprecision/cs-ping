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

- `ASN_DB_PATH`
- `CONCURRENT_DNS`
- `CONCURRENT_PINGS`
- `CONFIG_FOLDER`
- `PING_RETRIES`
- `PING_TIMEOUT_MS`
- `PINGS_PER_IP`
- `SCRIPT_URL`

## How to build

1) Clone the repo
2) `cd` into the cloned repo
3) Run `cargo build --release`
4) Binary will be in `target/release/`

## Contributing

Fork the repo, change some stuff, make sure everything still works, fix any clippy lints (run `cargo clippy`) and submit a PR.
