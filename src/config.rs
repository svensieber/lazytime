use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimeRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub onboarding_done: bool,
    pub default_project: String,
    #[serde(default = "default_tracking_stability_seconds")]
    pub tracking_stability_seconds: u64,
    #[serde(default)]
    pub working_hours: BTreeMap<u8, Vec<TimeRange>>,
    #[serde(default = "default_track_reminder_seconds")]
    pub track_reminder_seconds: u64,
    #[serde(default = "default_track_reminder_snooze_seconds")]
    pub track_reminder_snooze_seconds: u64,
    #[serde(default = "default_summary_update_seconds")]
    pub summary_update_seconds: u64,
    pub report_start: Option<String>,
    pub report_end: Option<String>,
    pub db_file: String,
    pub jira_url: Option<String>,
    pub jira_token: Option<String>,
    pub jira_email: Option<String>,
    pub jira_project: Option<String>,
    pub jira_assignee: Option<String>,
    #[serde(default = "default_jira_issue_type")]
    pub jira_issue_type: String,
    #[serde(default = "default_jira_sap_field")]
    pub jira_sap_field: String,
    pub ipc_socket_path: Option<String>,
    #[serde(default)]
    pub theme_preference: ThemePreference,
    #[serde(default)]
    pub sidebar_collapsed: bool,
}

fn default_jira_issue_type() -> String {
    "Story".to_string()
}

fn default_jira_sap_field() -> String {
    "sap_project".to_string()
}

fn default_tracking_stability_seconds() -> u64 {
    60
}

fn default_track_reminder_seconds() -> u64 {
    300
}

fn default_track_reminder_snooze_seconds() -> u64 {
    1800
}

fn default_summary_update_seconds() -> u64 {
    5
}

fn default_config_path() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        return home.join(".config/lazytime/config.json");
    }
    PathBuf::from("./config.json")
}

impl Config {
    pub fn from_path(path: Option<&str>) -> Result<Self> {
        let path = path.map(PathBuf::from).unwrap_or_else(default_config_path);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create config directory {}", parent.display())
                    })?;
                }
                let template = default_config_template(&path);
                let content = serde_json::to_string_pretty(&template)
                    .context("failed to serialize default config template")?;
                fs::write(&path, content)
                    .with_context(|| format!("failed to create config file {}", path.display()))?;
                tracing::info!("created default config file at {}", path.display());
                fs::read_to_string(&path).with_context(|| {
                    format!("failed to read created config file {}", path.display())
                })?
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read config file {}", path.display()));
            }
        };
        let cfg: Config = serde_json::from_str(&raw).context("failed to parse config JSON")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.default_project.trim().is_empty() {
            bail!("default_project must be set and non-empty");
        }
        for (weekday, ranges) in &self.working_hours {
            if *weekday > 6 {
                bail!("working_hours weekday {} out of range (0..6)", weekday);
            }
            let mut previous_end: Option<u32> = None;
            for range in ranges {
                let (start_h, start_m) = parse_hhmm(&range.start)
                    .with_context(|| format!("invalid working_hours start {}", range.start))?;
                let (end_h, end_m) = parse_hhmm(&range.end)
                    .with_context(|| format!("invalid working_hours end {}", range.end))?;
                let start_minutes = (start_h * 60) + start_m;
                let end_minutes = (end_h * 60) + end_m;
                if end_minutes <= start_minutes {
                    bail!(
                        "working_hours weekday {} invalid: end {} must be greater than start {}",
                        weekday,
                        range.end,
                        range.start
                    );
                }
                if let Some(previous_end) = previous_end
                    && start_minutes <= previous_end
                {
                    bail!(
                        "working_hours weekday {} invalid: start {} must be greater than previous end",
                        weekday,
                        range.start
                    );
                }
                previous_end = Some(end_minutes);
            }
        }
        if let Some(start) = &self.report_start {
            NaiveDate::parse_from_str(start, "%Y-%m-%d")
                .with_context(|| format!("invalid report_start date {}", start))?;
        }
        if let Some(end) = &self.report_end {
            NaiveDate::parse_from_str(end, "%Y-%m-%d")
                .with_context(|| format!("invalid report_end date {}", end))?;
        }
        Ok(())
    }

    pub fn db_path(&self) -> &Path {
        Path::new(&self.db_file)
    }

    pub fn ipc_socket_path(&self) -> PathBuf {
        if let Some(path) = &self.ipc_socket_path {
            return PathBuf::from(path);
        }
        #[cfg(all(feature = "ipc-tcp", any(target_os = "macos", target_os = "windows")))]
        return PathBuf::from("127.0.0.1:45555");

        #[cfg(all(feature = "ipc-unix", not(any(target_os = "macos", target_os = "windows"))))]
        if let Some(home) = dirs::home_dir() {
            return home.join(".local/run/lazytime.sock");
        }
        #[cfg(all(feature = "ipc-unix", not(any(target_os = "macos", target_os = "windows"))))]
        return PathBuf::from("/tmp/lazytime.sock");

        #[cfg(all(feature = "ipc-tcp", not(any(target_os = "macos", target_os = "windows"))))]
        return PathBuf::from("127.0.0.1:45555");

        #[allow(unreachable_code)]
        PathBuf::from("lazytime-ipc")
    }
}

fn default_config_template(config_path: &Path) -> Config {
    let db_file = if let Some(data_dir) = dirs::data_local_dir() {
        data_dir.join("lazytime/lazytime.db")
    } else if let Some(parent) = config_path.parent() {
        parent.join("lazytime.db")
    } else {
        PathBuf::from("./lazytime.db")
    };

    Config {
        onboarding_done: false,
        default_project: "Default".to_string(),
        tracking_stability_seconds: default_tracking_stability_seconds(),
        working_hours: BTreeMap::new(),
        track_reminder_seconds: default_track_reminder_seconds(),
        track_reminder_snooze_seconds: default_track_reminder_snooze_seconds(),
        summary_update_seconds: default_summary_update_seconds(),
        report_start: None,
        report_end: None,
        db_file: db_file.to_string_lossy().to_string(),
        jira_url: None,
        jira_token: None,
        jira_email: None,
        jira_project: None,
        jira_assignee: None,
        jira_issue_type: default_jira_issue_type(),
        jira_sap_field: default_jira_sap_field(),
        ipc_socket_path: None,
        theme_preference: ThemePreference::Auto,
        sidebar_collapsed: false,
    }
}

pub fn parse_hhmm(value: &str) -> Result<(u32, u32)> {
    let mut parts = value.split(':');
    let hour_part = parts.next().context("missing hour")?;
    let minute_part = parts.next().context("missing minute")?;
    if minute_part.len() != 2 {
        bail!("minute must be two digits");
    }

    let hour = hour_part.parse::<u32>().context("invalid hour")?;
    let minute = minute_part.parse::<u32>().context("invalid minute")?;
    if parts.next().is_some() {
        bail!("invalid time format");
    }
    if hour > 23 || minute > 59 {
        bail!("time out of range");
    }
    Ok((hour, minute))
}
