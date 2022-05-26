# cs-ping

Pings all server-locations extracted from this file [cryptostorm.is/wg_confgen.txt](https://cryptostorm.is/wg_confgen.txt)

## Currently hardcoded

- `MAX_CONCURRENT_PINGS`
  - Number of concurrent pings
- `MAX_TIMEOUT_RETIRES`
  - How often a ping to the same IP is allowed to time-out
- `PING_COUNT`
  - How many pings are used for statistics
- `SCRIPT_URL`
  - Link to `wg_confgen.txt`

## How to build

1) Clone the repo
2) `cd` into the root of the repo
3) `cargo build --release`
4) Binary will be in `./target/release/`
