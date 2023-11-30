use std::str::FromStr;

use indicatif::{ProgressBar, ProgressStyle};

/// Create a progress bar with custom settings
pub fn styled_progress_bar(len: u64, msg: &'static str) -> ProgressBar {
    // Binary counter - https://raw.githubusercontent.com/sindresorhus/cli-spinners/master/spinners.json
    const BAR_FORMAT: &str = "{spinner} {msg} [{pos:>4}/{len:<4}] [{bar:25.cyan/grey}] {eta}";
    const BAR_TICK_CHARS: &str = "⠁⠂⠃⠄⠅⠆⠇⡀⡁⡂⡃⡄⡅⡆⡇⠈⠉⠊⠋⠌⠍⠎⠏⡈⡉⡊⡋⡌⡍⡎⡏⠐⠑⠒⠓⠔⠕⠖⠗⡐⡑⡒⡓⡔⡕⡖⡗⠘⠙⠚⠛⠜\
        ⠝⠞⠟⡘⡙⡚⡛⡜⡝⡞⡟⠠⠡⠢⠣⠤⠥⠦⠧⡠⡡⡢⡣⡤⡥⡦⡧⠨⠩⠪⠫⠬⠭⠮⠯⡨⡩⡪⡫⡬⡭⡮⡯⠰⠱⠲⠳⠴⠵⠶⠷⡰⡱⡲⡳⡴⡵⡶⡷⠸⠹⠺⠻⠼⠽⠾⠿⡸⡹⡺⡻⡼⡽⡾⡿⢀\
        ⢁⢂⢃⢄⢅⢆⢇⣀⣁⣂⣃⣄⣅⣆⣇⢈⢉⢊⢋⢌⢍⢎⢏⣈⣉⣊⣋⣌⣍⣎⣏⢐⢑⢒⢓⢔⢕⢖⢗⣐⣑⣒⣓⣔⣕⣖⣗⢘⢙⢚⢛⢜⢝⢞⢟⣘⣙⣚⣛⣜⣝⣞⣟⢠⢡⢢⢣⢤⢥⢦⢧⣠⣡⣢⣣⣤\
        ⣥⣦⣧⢨⢩⢪⢫⢬⢭⢮⢯⣨⣩⣪⣫⣬⣭⣮⣯⢰⢱⢲⢳⢴⢵⢶⢷⣰⣱⣲⣳⣴⣵⣶⣷⢸⢹⢺⢻⢼⢽⢾⢿⣸⣹⣺⣻⣼⣽⣾⣿";

    ProgressBar::new(len).with_message(msg).with_style(
        ProgressStyle::default_bar()
            .tick_chars(BAR_TICK_CHARS)
            .template(BAR_FORMAT)
            .unwrap()
            .progress_chars("=> "),
    )
}

/// Load the environment variable of fall back to default
pub fn env_var(key: &str, default: Option<&str>) -> Option<String> {
    let Ok(var) = std::env::var(key) else {
        return default.map(String::from);
    };
    Some(var)
}

/// Load and parse the environment variable or fall back to default
pub fn env_var_parse<T: FromStr>(key: &str, default: Option<T>) -> Option<T> {
    let Ok(str) = std::env::var(key) else {
        return default;
    };
    str.parse().ok().or(default)
}

/// Load the environment variable and try to read the file it points to
pub async fn env_var_read_file(key: &str, default: Option<&str>) -> Option<Vec<u8>> {
    let path = std::env::var(key)
        .ok()
        .or_else(|| default.map(String::from))?;
    tokio::fs::read(path).await.ok()
}

pub fn replace_all(mut string: String, from_to: &[(&str, &str)]) -> String {
    from_to.iter().for_each(|&(from, to)| {
        string = string.replace(from, to);
    });
    string
}

pub fn trim_string(mut input: String, max_chars: usize) -> String {
    if let Some((byte_idx, _)) = input.char_indices().nth(max_chars) {
        input.truncate(byte_idx);
    }
    input
}

pub fn trim_str(mut input: &str, max_chars: usize) -> &str {
    if let Some((byte_idx, _)) = input.char_indices().nth(max_chars) {
        input = &input[..byte_idx];
    }
    input
}
