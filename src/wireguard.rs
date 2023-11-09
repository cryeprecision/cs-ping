use anyhow::Context;

const TEMPLATE_CLIENT_PRIVATE_KEY: &str = "{{ PRIVATE_KEY }}";
const TEMPLATE_CLIENT_ADDRESS: &str = "{{ ADDRESS }}";
const TEMPLATE_CLIENT_DNS: &str = "{{ DNS }}";
const TEMPLATE_SERVER_PRESHARED_KEY: &str = "{{ PRESHARED_KEY }}";
const TEMPLATE_SERVER_PUBLIC_KEY: &str = "{{ PUBLIC_KEY }}";
const TEMPLATE_SERVER_ENDPOINT: &str = "{{ ENDPOINT }}";

const TEMPLATE: &str = "\
[Interface]
PrivateKey = {{ PRIVATE_KEY }}
Address = {{ ADDRESS }}
DNS = {{ DNS }}

[Peer]
Presharedkey = {{ PRESHARED_KEY }}
PublicKey = {{ PUBLIC_KEY }}
Endpoint = {{ ENDPOINT }}
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 25
";

pub struct Config {
    client_private_key: String,
    client_address: String,
    client_dns: String,
    server_preshared_key: String,
    server_public_key: String,
    server_endpoint: String,
}

impl Config {
    pub fn from_env(hostname: &str, public_key: &str) -> anyhow::Result<Config> {
        Ok(Config {
            client_private_key: crate::util::env_var("CONFIG_CLIENT_PRIVATE_KEY", None)
                .context("env var CONFIG_CLIENT_PRIVATE_KEY missing")?,
            client_address: crate::util::env_var("CONFIG_CLIENT_ADDRESS", None)
                .context("env var CONFIG_CLIENT_ADDRESS missing")?,
            client_dns: crate::util::env_var("CONFIG_CLIENT_DNS", None)
                .context("env var CONFIG_CLIENT_DNS missing")?,
            server_preshared_key: crate::util::env_var("CONFIG_SERVER_PRESHARED_KEY", None)
                .context("env var CONFIG_SERVER_PRESHARED_KEY missing")?,
            server_public_key: public_key.to_string(),
            server_endpoint: format!("{}:443", hostname),
        })
    }
}

impl ToString for Config {
    fn to_string(&self) -> String {
        crate::util::replace_all(
            TEMPLATE.to_string(),
            &[
                (TEMPLATE_CLIENT_PRIVATE_KEY, &self.client_private_key),
                (TEMPLATE_CLIENT_ADDRESS, &self.client_address),
                (TEMPLATE_CLIENT_DNS, &self.client_dns),
                (TEMPLATE_SERVER_PRESHARED_KEY, &self.server_preshared_key),
                (TEMPLATE_SERVER_PUBLIC_KEY, &self.server_public_key),
                (TEMPLATE_SERVER_ENDPOINT, &self.server_endpoint),
            ],
        )
    }
}
