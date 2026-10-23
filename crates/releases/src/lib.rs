//! The latest ChessPenguin release on GitHub: its version, its files and their SHA-256
//! from the release's `SHA256SUMS.txt`. The desktop app updates itself with it, the
//! Android app offers its new APK; both are one release with one version (Cargo.toml).

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

/// GitHub's answer for the latest release.
pub const API: &str = "https://api.github.com/repos/vrubelroman/chessPenguin/releases/latest";

/// Where people download ChessPenguin.
pub const DOWNLOAD_PAGE: &str = "https://chess.vrubel.xyz/";

/// The version of this build (the workspace's: the desktop and the phone are one release).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The file of the release with every file's SHA-256.
pub const SUMS: &str = "SHA256SUMS.txt";

/// "0.7.1" / "v0.7.1" → (0, 7, 1).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let n = |s: Option<&str>| -> Option<u32> {
        s?.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    };
    Some((n(it.next())?, n(it.next())?, n(it.next()).unwrap_or(0)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The latest release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Its page (what is new in it).
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Newer than this build.
    pub fn newer(&self) -> bool {
        self.newer_than(VERSION)
    }

    pub fn newer_than(&self, current: &str) -> bool {
        match (parse_version(&self.version), parse_version(current)) {
            (Some(v), Some(c)) => v > c,
            _ => false,
        }
    }

    /// The file whose name ends with `suffix` ("-android-noengine.apk").
    pub fn asset_ending(&self, suffix: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name.ends_with(suffix))
    }

    pub fn sums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == SUMS)
    }
}

/// GitHub's answer for a release, as a [`Release`].
pub fn parse_release(v: &Value) -> Option<Release> {
    let tag = v["tag_name"].as_str()?;
    let assets = v["assets"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some(Asset {
                name: a["name"].as_str()?.to_owned(),
                url: a["browser_download_url"].as_str()?.to_owned(),
                size: a["size"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        page: v["html_url"].as_str().unwrap_or(DOWNLOAD_PAGE).to_owned(),
        assets,
    })
}

/// The SHA-256 of `name` in the text of a `SHA256SUMS.txt` (`<hash>  <name>` lines, as
/// `sha256sum` writes them; a `*` before the name marks binary mode).
pub fn sum_in(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_lowercase())
    })
}

/// An HTTP client that says who asks (`client`: "ChessPenguin/0.15.0 (Android)").
/// `body` bounds the time to read an answer's body: a stalled connection ends then.
fn agent(client: &str, body: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(format!("{client} (+{DOWNLOAD_PAGE})"))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Asks `api` (GitHub's [`API`], or a test server with the same answer) for the latest
/// release. `Err` with a short reason when there is no answer.
pub fn fetch_latest(api: &str, client: &str) -> Result<Release, String> {
    let text = agent(client, Duration::from_secs(30))
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    parse_release(&v).ok_or_else(|| "unexpected answer".to_owned())
}

/// Opens a file of the release for reading; a whole download has half an hour.
pub fn open(asset: &Asset, client: &str) -> Result<Box<dyn Read + Send>, String> {
    let resp = agent(client, Duration::from_secs(1800))
        .get(&asset.url)
        .call()
        .map_err(|e| e.to_string())?;
    Ok(Box::new(resp.into_body().into_reader()))
}

/// The text of the release's `SHA256SUMS.txt`.
pub fn fetch_sums(release: &Release, client: &str) -> Result<String, String> {
    let sums = release.sums().ok_or("the release has no SHA256SUMS.txt")?;
    let mut text = String::new();
    open(sums, client)?
        .take(1 << 20)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.7.1"), Some((0, 7, 1)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("x"), None);
        let r = |v: &str| Release {
            version: v.into(),
            page: String::new(),
            assets: Vec::new(),
        };
        assert!(r("0.10.0").newer_than("0.9.9"));
        assert!(!r("0.14.0").newer_than("0.14.0"));
        assert!(!r("0.13.9").newer_than("0.14.0"));
        assert!(!r("garbage").newer_than("0.14.0"));
    }

    #[test]
    fn github_answer() {
        let v: Value = serde_json::from_str(
            r#"{"tag_name":"v0.7.1","html_url":"https://github.com/x/releases/tag/v0.7.1",
                "assets":[{"name":"SHA256SUMS.txt","browser_download_url":"https://x/s","size":10},
                          {"name":"ChessPenguin-0.7.1-android-noengine.apk","browser_download_url":"https://x/a","size":9}]}"#,
        )
        .unwrap();
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, "0.7.1");
        assert_eq!(r.page, "https://github.com/x/releases/tag/v0.7.1");
        assert_eq!(r.sums().map(|a| a.url.as_str()), Some("https://x/s"));
        assert_eq!(
            r.asset_ending("-android-noengine.apk").map(|a| a.size),
            Some(9)
        );
        assert!(r.asset_ending("-android.apk").is_none());
    }

    #[test]
    fn sums() {
        let text = "AB12  ChessPenguin-1-android.apk\ncd34 *ChessPenguin-1-android-noengine.apk\n";
        assert_eq!(
            sum_in(text, "ChessPenguin-1-android.apk").as_deref(),
            Some("ab12")
        );
        assert_eq!(
            sum_in(text, "ChessPenguin-1-android-noengine.apk").as_deref(),
            Some("cd34")
        );
        assert_eq!(sum_in(text, "other"), None);
    }
}
