//! A newer release for the Android app: the `releases` crate (the same check as the
//! desktop's) with the two APKs of the release and their SHA-256. The app downloads and
//! installs the file itself (Android's download service and package installer).
//!
//! The check talks to GitHub (or to a test server in a debug build) and blocks: call it
//! off the main thread.

use releases::{Release, VERSION, check_latest, fetch_latest, fetch_sums, sum_in};

/// GitHub's answer for the latest release (a debug build may ask a test server).
#[uniffi::export]
pub fn release_api() -> String {
    releases::API.to_owned()
}

/// An APK of a release.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ApkFile {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// From the release's `SHA256SUMS.txt`, lowercase hex.
    pub sha256: String,
}

/// A release newer than this app.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppRelease {
    pub version: String,
    /// Its page: what is new in it.
    pub page: String,
    /// The build without the engine's network (the network already in the app stays).
    pub light: Option<ApkFile>,
    /// The build with the network inside.
    pub full: Option<ApkFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ReleaseCheck {
    /// This app is the latest release (or newer: a build from the sources), or the newer
    /// release has nothing for Android (a desktop-only one).
    UpToDate {
        latest: String,
    },
    Newer {
        release: AppRelease,
    },
    /// A newer release whose light APK cannot be checked: no `SHA256SUMS.txt`, or no sum
    /// for the file in it.
    Unverifiable {
        release: AppRelease,
    },
    /// No answer: `detail` for the log.
    Failed {
        detail: String,
    },
}

const LIGHT: &str = "-android-noengine.apk";
const FULL: &str = "-android.apk";

/// Asks `api` (a test server) for the latest release; a newer one comes with its APKs.
#[uniffi::export]
pub fn check_release(api: String) -> ReleaseCheck {
    let client = format!("ChessPenguin/{VERSION} (Android)");
    checked(fetch_latest(&api, &client), &client)
}

/// Asks the site for the latest release, counting this installation for the day (its
/// random `install` id and `platform`: "android" or "android-noengine"); without the
/// site, GitHub. A newer release comes with its APKs.
#[uniffi::export]
pub fn check_release_counted(install: String, platform: String) -> ReleaseCheck {
    let client = format!("ChessPenguin/{VERSION} (Android)");
    checked(check_latest(&install, &platform, &client), &client)
}

fn checked(found: Result<Release, String>, client: &str) -> ReleaseCheck {
    let release = match found {
        Ok(r) => r,
        Err(detail) => return ReleaseCheck::Failed { detail },
    };
    if !release.newer() || release.asset_ending(LIGHT).is_none() {
        return ReleaseCheck::UpToDate {
            latest: release.version,
        };
    }
    let sums = if release.sums().is_some() {
        match fetch_sums(&release, client) {
            Ok(s) => s,
            Err(detail) => return ReleaseCheck::Failed { detail },
        }
    } else {
        String::new()
    };
    let found = app_release(&release, &sums);
    if found.light.as_ref().is_some_and(|a| !a.sha256.is_empty()) {
        ReleaseCheck::Newer { release: found }
    } else {
        ReleaseCheck::Unverifiable { release: found }
    }
}

/// The release's APKs with their sums ("" when `sums` has none for a file).
fn app_release(release: &Release, sums: &str) -> AppRelease {
    let apk = |suffix: &str| {
        let a = release.asset_ending(suffix)?;
        Some(ApkFile {
            name: a.name.clone(),
            url: a.url.clone(),
            size: a.size,
            sha256: sum_in(sums, &a.name).unwrap_or_default(),
        })
    };
    AppRelease {
        version: release.version.clone(),
        page: release.page.clone(),
        light: apk(LIGHT),
        full: apk(FULL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use releases::Asset;

    fn release(names: &[&str]) -> Release {
        Release {
            version: "9.9.9".into(),
            page: "https://example.org/v9.9.9".into(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    url: format!("https://example.org/{n}"),
                    size: 7,
                })
                .collect(),
        }
    }

    #[test]
    fn both_apks_with_their_sums() {
        let r = release(&[
            "ChessPenguin-9.9.9-android.apk",
            "ChessPenguin-9.9.9-android-noengine.apk",
            "SHA256SUMS.txt",
        ]);
        let sums =
            "aa  ChessPenguin-9.9.9-android.apk\nbb  ChessPenguin-9.9.9-android-noengine.apk\n";
        let a = app_release(&r, sums);
        let light = a.light.unwrap();
        assert_eq!(light.name, "ChessPenguin-9.9.9-android-noengine.apk");
        assert_eq!(light.sha256, "bb");
        assert_eq!(a.full.unwrap().sha256, "aa");
    }

    #[test]
    fn an_apk_without_its_sum_has_none() {
        let r = release(&["ChessPenguin-9.9.9-android-noengine.apk", "SHA256SUMS.txt"]);
        assert_eq!(app_release(&r, "").light.unwrap().sha256, "");
        let r = release(&["ChessPenguin-9.9.9-windows-x86_64.zip"]);
        assert!(app_release(&r, "").light.is_none());
    }
}
