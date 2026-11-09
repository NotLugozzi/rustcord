use base64::Engine;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Serialize;
use std::sync::OnceLock;

pub const APP_NAME: &str = "Rustcord";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub os: String,
    pub arch: String,
    pub os_version: Option<String>,
    pub locale: Option<String>,
}

impl Runtime {
    pub fn detect() -> Self {
        Self {
            os: os_display_name(std::env::consts::OS),
            arch: std::env::consts::ARCH.to_string(),
            os_version: detect_os_version(),
            locale: detect_locale(|key| std::env::var(key).ok()),
        }
    }
}

fn os_display_name(os: &str) -> String {
    match os {
        "linux" => "Linux".into(),
        "windows" => "Windows".into(),
        "macos" => "Mac OS X".into(),
        "freebsd" => "FreeBSD".into(),
        "openbsd" => "OpenBSD".into(),
        "netbsd" => "NetBSD".into(),
        other => other.into(),
    }
}

#[cfg(target_os = "linux")]
fn detect_os_version() -> Option<String> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    clean_field(&release)
}

#[cfg(not(target_os = "linux"))]
fn detect_os_version() -> Option<String> {
    None
}

fn detect_locale(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| var(key))
        .find_map(|value| parse_posix_locale(&value))
}

fn parse_posix_locale(value: &str) -> Option<String> {
    let base = value.split(['.', '@']).next()?.trim();
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    let tag = base.replace('_', "-");
    let valid = tag.split('-').all(|part| {
        !part.is_empty() && part.len() <= 8 && part.chars().all(|c| c.is_ascii_alphanumeric())
    });
    valid.then_some(tag)
}

fn clean_field(value: &str) -> Option<String> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    browser: String,
    user_agent: String,
    runtime: Runtime,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SuperProperties<'a> {
    pub os: &'a str,
    pub browser: &'a str,
    pub device: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_locale: Option<&'a str>,
    pub browser_user_agent: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<&'a str>,
    pub os_arch: &'a str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct GatewayProperties<'a> {
    pub os: &'a str,
    pub browser: &'a str,
    pub device: &'a str,
}

impl ClientIdentity {
    pub fn current() -> &'static ClientIdentity {
        static CURRENT: OnceLock<ClientIdentity> = OnceLock::new();
        CURRENT.get_or_init(|| ClientIdentity::rustcord(Runtime::detect()))
    }

    fn rustcord(runtime: Runtime) -> Self {
        let user_agent = format!(
            "{APP_NAME}/{APP_VERSION} ({}; {})",
            runtime.os, runtime.arch
        );
        Self {
            browser: APP_NAME.into(),
            user_agent,
            runtime,
        }
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn super_properties(&self) -> SuperProperties<'_> {
        SuperProperties {
            os: &self.runtime.os,
            browser: &self.browser,
            device: "",
            system_locale: self.runtime.locale.as_deref(),
            browser_user_agent: &self.user_agent,
            browser_version: product_version(&self.user_agent, &self.browser),
            os_version: self.runtime.os_version.as_deref(),
            os_arch: &self.runtime.arch,
        }
    }

    pub fn super_properties_json(&self) -> String {
        serde_json::to_string(&self.super_properties()).unwrap_or_default()
    }

    pub fn super_properties_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.super_properties_json())
    }

    pub fn gateway_properties(&self) -> GatewayProperties<'_> {
        GatewayProperties {
            os: &self.runtime.os,
            browser: &self.browser,
            device: &self.browser,
        }
    }

    pub fn http_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&self.user_agent) {
            headers.insert(header::USER_AGENT, value);
        }
        if let Ok(value) = HeaderValue::from_str(&self.super_properties_b64()) {
            headers.insert("X-Super-Properties", value);
        }
        headers
    }
}

fn product_version<'a>(user_agent: &'a str, product: &str) -> Option<&'a str> {
    user_agent.split_whitespace().find_map(|token| {
        let (name, version) = token.split_once('/')?;
        (name == product && !version.is_empty()).then_some(version)
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::{ClientIdentity, Runtime};

    pub const WINDOWS_CHROME_UA: &str =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
    pub const LINUX_FIREFOX_UA: &str =
        "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";

    pub fn windows_chrome() -> ClientIdentity {
        ClientIdentity {
            browser: "Chrome".into(),
            user_agent: WINDOWS_CHROME_UA.into(),
            runtime: Runtime {
                os: "Windows".into(),
                arch: "x86_64".into(),
                os_version: Some("10".into()),
                locale: Some("en-US".into()),
            },
        }
    }

    pub fn linux_firefox() -> ClientIdentity {
        ClientIdentity {
            browser: "Firefox".into(),
            user_agent: LINUX_FIREFOX_UA.into(),
            runtime: Runtime {
                os: "Linux".into(),
                arch: "x86_64".into(),
                os_version: None,
                locale: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use serde_json::Value;

    fn decoded(identity: &ClientIdentity) -> Value {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(identity.super_properties_b64())
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn bare_runtime() -> Runtime {
        Runtime {
            os: "Linux".into(),
            arch: "x86_64".into(),
            os_version: None,
            locale: None,
        }
    }

    #[test]
    fn super_properties_serialize_to_a_json_object() {
        let json: Value =
            serde_json::from_str(&ClientIdentity::current().super_properties_json()).unwrap();
        assert!(json.is_object());
        assert_eq!(json["browser"], APP_NAME);
    }

    #[test]
    fn the_base64_header_decodes_to_the_same_json() {
        let identity = ClientIdentity::current();
        let expected: Value = serde_json::from_str(&identity.super_properties_json()).unwrap();
        assert_eq!(decoded(identity), expected);
    }

    #[test]
    fn the_runtime_reports_the_os_it_was_built_for() {
        let runtime = Runtime::detect();
        assert_eq!(runtime.arch, std::env::consts::ARCH);
        #[cfg(target_os = "linux")]
        {
            assert_eq!(runtime.os, "Linux");
            assert!(
                runtime.os_version.is_some(),
                "kernel release should be readable on linux"
            );
        }
        #[cfg(target_os = "windows")]
        assert_eq!(runtime.os, "Windows");
        #[cfg(target_os = "macos")]
        assert_eq!(runtime.os, "Mac OS X");
    }

    #[test]
    fn the_user_agent_names_rustcord_and_the_real_platform() {
        let identity = ClientIdentity::rustcord(bare_runtime());
        assert_eq!(
            identity.user_agent(),
            format!("Rustcord/{APP_VERSION} (Linux; x86_64)")
        );
        assert_eq!(
            identity.super_properties().browser_version,
            Some(APP_VERSION)
        );
    }

    #[test]
    fn the_header_user_agent_always_matches_browser_user_agent() {
        for identity in [
            ClientIdentity::current().clone(),
            windows_chrome(),
            linux_firefox(),
        ] {
            let headers = identity.http_headers();
            let header_ua = headers.get(header::USER_AGENT).unwrap().to_str().unwrap();
            assert_eq!(decoded(&identity)["browser_user_agent"], header_ua);
        }
    }

    #[test]
    fn browser_versions_are_parsed_from_the_user_agent() {
        assert_eq!(
            windows_chrome().super_properties().browser_version,
            Some("131.0.0.0")
        );
        assert_eq!(
            linux_firefox().super_properties().browser_version,
            Some("133.0")
        );
        assert_eq!(
            product_version("Rustcord/ (Linux; x86_64)", "Rustcord"),
            None
        );
        assert_eq!(product_version("curl/8.0", "Rustcord"), None);
    }

    #[test]
    fn fixtures_never_leak_into_the_production_identity() {
        let production = ClientIdentity::current();
        for fixture in [windows_chrome(), linux_firefox()] {
            assert_ne!(production, &fixture);
            assert_ne!(production.user_agent(), fixture.user_agent());
        }
        assert!(production.user_agent().starts_with("Rustcord/"));
        assert!(!production.user_agent().contains("Mozilla"));
    }

    #[test]
    fn missing_metadata_is_omitted_rather_than_invented() {
        let json = decoded(&ClientIdentity::rustcord(bare_runtime()));
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for absent in [
            "system_locale",
            "os_version",
            "client_build_number",
            "release_channel",
            "has_client_mods",
            "launch_signature",
        ] {
            assert!(!keys.contains(&absent), "{absent} should not be sent");
        }
        assert_eq!(json["device"], "");
    }

    #[test]
    fn gateway_properties_share_the_runtime_os_and_name_rustcord() {
        let identity = ClientIdentity::rustcord(bare_runtime());
        let gateway = serde_json::to_value(identity.gateway_properties()).unwrap();
        assert_eq!(
            gateway,
            serde_json::json!({ "os": "Linux", "browser": "Rustcord", "device": "Rustcord" })
        );
        assert_eq!(gateway["os"], decoded(&identity)["os"]);
    }

    #[test]
    fn posix_locales_become_bcp47_tags() {
        assert_eq!(parse_posix_locale("en_US.UTF-8").as_deref(), Some("en-US"));
        assert_eq!(parse_posix_locale("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(parse_posix_locale("fr").as_deref(), Some("fr"));
        for unusable in ["", "C", "POSIX", "C.UTF-8", "en_US;rm -rf", "_"] {
            assert_eq!(parse_posix_locale(unusable), None, "{unusable}");
        }
    }

    #[test]
    fn locale_lookup_respects_posix_precedence_and_skips_unset_values() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            detect_locale(env(&[("LANG", "en_GB.UTF-8"), ("LC_ALL", "ja_JP.UTF-8")])).as_deref(),
            Some("ja-JP")
        );
        assert_eq!(
            detect_locale(env(&[("LC_ALL", "C"), ("LANG", "pt_BR.UTF-8")])).as_deref(),
            Some("pt-BR")
        );
        assert_eq!(detect_locale(env(&[])), None);
    }

    #[test]
    fn unknown_platforms_pass_through_and_odd_bytes_are_stripped() {
        assert_eq!(os_display_name("haiku"), "haiku");
        assert_eq!(clean_field("  6.1.0-rc1\n"), Some("6.1.0-rc1".into()));
        assert_eq!(clean_field("\u{0}\u{7}\n"), None);
    }
}
