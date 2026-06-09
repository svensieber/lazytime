#[cfg(feature = "popup-ui")]
mod app;
#[cfg(feature = "popup-ui")]
mod color;
#[cfg(all(feature = "popup-ui", target_os = "macos"))]
mod macos_status;
#[cfg(feature = "popup-ui")]
mod style;
#[cfg(feature = "popup-ui")]
mod table;
#[cfg(feature = "popup-ui")]
mod views;

use crate::config::Config;
use anyhow::Result;

pub fn run(config: &Config, config_path: Option<&str>) -> Result<()> {
    #[cfg(feature = "popup-ui")]
    {
        return app::run(config, config_path);
    }

    #[cfg(not(feature = "popup-ui"))]
    {
        let _ = (config, config_path);
        anyhow::bail!("GUI support requires the popup-ui feature")
    }
}
