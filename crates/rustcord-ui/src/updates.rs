use serde::Deserialize;

const REPOSITORY: &str = "NotLugozzi/rustcord";
const WORKFLOW: &str = "build.yml";
const BRANCH: &str = "main";
pub const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Off,
    Releases,
    Builds,
}

impl Channel {
    pub const ALL: [Self; 3] = [Self::Off, Self::Releases, Self::Builds];

    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Releases => "releases",
            Self::Builds => "builds",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|channel| channel.key() == key).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Don't check",
            Self::Releases => "Releases",
            Self::Builds => "Development builds (GitHub Actions)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate { latest: String },
    Available { latest: String, behind_by: Option<u64>, url: String },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Check {
    pub channel: Channel,
    pub outcome: Outcome,
    pub at: chrono::DateTime<chrono::Local>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct Runs {
    workflow_runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    head_sha: String,
    html_url: String,
    run_number: u64,
    created_at: String,
}

#[derive(Deserialize)]
struct Comparison {
    status: String,
    ahead_by: u64,
}

struct Latest {
    label: String,
    reference: String,
    url: String,
    published: Option<String>,
}

async fn get<T: serde::de::DeserializeOwned>(client: &reqwest::Client, path: &str) -> Result<T, String> {
    let response = client
        .get(format!("https://api.github.com/repos/{REPOSITORY}/{path}"))
        .header("User-Agent", "rustcord-update-check")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("couldn't reach GitHub: {error}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("GitHub is rate limiting update checks; try again later".into());
    }
    if !status.is_success() {
        return Err(format!("GitHub answered {status}"));
    }
    response.json().await.map_err(|error| format!("GitHub sent something unexpected: {error}"))
}

async fn latest(client: &reqwest::Client, channel: Channel) -> Result<Latest, String> {
    match channel {
        Channel::Off => Err("update checks are off".into()),
        Channel::Releases => {
            let release: Release = get(client, "releases/latest").await?;
            Ok(Latest { label: release.tag_name.clone(), reference: release.tag_name, url: release.html_url, published: release.published_at })
        }
        Channel::Builds => {
            let runs: Runs = get(client, &format!("actions/workflows/{WORKFLOW}/runs?branch={BRANCH}&status=success&per_page=1")).await?;
            let run = runs.workflow_runs.into_iter().next().ok_or("there are no successful builds yet")?;
            let short = run.head_sha.chars().take(7).collect::<String>();
            Ok(Latest { label: format!("build #{} ({short})", run.run_number), reference: run.head_sha, url: run.html_url, published: Some(run.created_at) })
        }
    }
}

pub fn judge(current_commit: Option<&str>, current_tag: Option<&str>, current_date: Option<&str>, latest_reference: &str, latest_published: Option<&str>, comparison: Option<(&str, u64)>) -> Option<Option<u64>> {
    if current_tag == Some(latest_reference) || current_commit.is_some_and(|commit| latest_reference.starts_with(commit)) {
        return None;
    }
    if let Some((status, ahead_by)) = comparison {
        return match status {
            "identical" | "behind" => None,
            _ if ahead_by == 0 => None,
            _ => Some(Some(ahead_by)),
        };
    }
    match (current_date, latest_published.map(|published| &published[..published.len().min(10)])) {
        (Some(ours), Some(theirs)) if theirs <= ours => None,
        _ => Some(None),
    }
}

pub async fn check(client: &reqwest::Client, channel: Channel) -> Outcome {
    use crate::build_info;
    let latest = match latest(client, channel).await {
        Ok(latest) => latest,
        Err(error) => return Outcome::Failed(error),
    };
    let comparison = match build_info::commit() {
        Some(commit) => get::<Comparison>(client, &format!("compare/{commit}...{}", latest.reference)).await.ok(),
        None => None,
    };
    match judge(
        build_info::commit(),
        build_info::tag(),
        build_info::commit_date(),
        &latest.reference,
        latest.published.as_deref(),
        comparison.as_ref().map(|comparison| (comparison.status.as_str(), comparison.ahead_by)),
    ) {
        None => Outcome::UpToDate { latest: latest.label },
        Some(behind_by) => Outcome::Available { latest: latest.label, behind_by, url: latest.url },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip_through_their_keys() {
        for channel in Channel::ALL {
            assert_eq!(Channel::from_key(channel.key()), channel);
        }
        assert_eq!(Channel::from_key("nonsense"), Channel::Off);
    }

    #[test]
    fn the_same_tag_or_commit_is_up_to_date() {
        assert_eq!(judge(Some("abc1234"), Some("0.2.0"), None, "0.2.0", None, None), None);
        assert_eq!(judge(Some("abc1234"), None, None, "abc1234def", None, None), None);
    }

    #[test]
    fn the_comparison_decides_when_available() {
        assert_eq!(judge(Some("abc1234"), None, None, "0.3.0", None, Some(("ahead", 12))), Some(Some(12)));
        assert_eq!(judge(Some("abc1234"), None, None, "0.3.0", None, Some(("diverged", 3))), Some(Some(3)));
        assert_eq!(judge(Some("abc1234"), None, None, "0.3.0", None, Some(("behind", 0))), None);
        assert_eq!(judge(Some("abc1234"), None, None, "0.3.0", None, Some(("identical", 0))), None);
    }

    #[test]
    fn without_a_commit_the_dates_decide() {
        assert_eq!(judge(None, None, Some("2026-11-20"), "0.3.0", Some("2026-11-07T23:59:36Z"), None), None);
        assert_eq!(judge(None, None, Some("2026-11-01"), "0.3.0", Some("2026-11-07T23:59:36Z"), None), Some(None));
        assert_eq!(judge(None, None, None, "0.3.0", None, None), Some(None));
    }
}
