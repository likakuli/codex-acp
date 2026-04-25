//! Codex ACP - An Agent Client Protocol implementation for Codex.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use agent_client_protocol::ByteStreams;
use codex_config::CONFIG_TOML_FILE;
use codex_core::config::{Config, ConfigOverrides};
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::CliConfigOverrides;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use toml::{Value as TomlValue, map::Map as TomlMap};
use tracing_subscriber::EnvFilter;

mod codex_agent;
mod thread;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PluginCliOverrides {
    enable_plugins: Vec<String>,
    disable_plugins: Vec<String>,
}

impl PluginCliOverrides {
    pub fn new(enable_plugins: Vec<String>, disable_plugins: Vec<String>) -> Result<Self, String> {
        let enable_plugins = normalize_plugin_ids(enable_plugins);
        let disable_plugins = normalize_plugin_ids(disable_plugins);
        let enabled = enable_plugins.iter().collect::<HashSet<_>>();

        if let Some(conflict) = disable_plugins
            .iter()
            .find(|plugin_id| enabled.contains(plugin_id))
        {
            return Err(format!(
                "plugin `{conflict}` cannot be both enabled and disabled"
            ));
        }

        Ok(Self {
            enable_plugins,
            disable_plugins,
        })
    }

    fn is_empty(&self) -> bool {
        self.enable_plugins.is_empty() && self.disable_plugins.is_empty()
    }
}

fn normalize_plugin_ids(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|plugin_id| !plugin_id.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|plugin_id| seen.insert(plugin_id.clone()))
        .collect()
}

fn set_plugin_enabled(user_config: &mut TomlValue, plugin_id: &str, enabled: bool) {
    let root = ensure_table(user_config);
    let plugins = root
        .entry("plugins".to_string())
        .or_insert_with(empty_toml_table);
    let plugins = ensure_table(plugins);
    let plugin = plugins
        .entry(plugin_id.to_string())
        .or_insert_with(empty_toml_table);
    let plugin = ensure_table(plugin);
    plugin.insert("enabled".to_string(), TomlValue::Boolean(enabled));
}

fn apply_plugin_cli_overrides_to_user_config(
    user_config: &mut TomlValue,
    plugin_overrides: &PluginCliOverrides,
) {
    for plugin_id in &plugin_overrides.enable_plugins {
        set_plugin_enabled(user_config, plugin_id, true);
    }

    for plugin_id in &plugin_overrides.disable_plugins {
        set_plugin_enabled(user_config, plugin_id, false);
    }
}

fn apply_codex_acp_plugin_overrides(config: &mut Config, plugin_overrides: &PluginCliOverrides) {
    if plugin_overrides.is_empty() {
        tracing::debug!("codex-acp plugin overrides are empty");
        return;
    }

    let mut user_config = config
        .config_layer_stack
        .get_user_layer()
        .map(|layer| layer.config.clone())
        .unwrap_or_else(empty_toml_table);
    apply_plugin_cli_overrides_to_user_config(&mut user_config, plugin_overrides);

    let user_config_path =
        AbsolutePathBuf::resolve_path_against_base(CONFIG_TOML_FILE, &config.codex_home);
    config.config_layer_stack = config
        .config_layer_stack
        .with_user_config(&user_config_path, user_config);
    tracing::info!(
        enable_plugins = ?plugin_overrides.enable_plugins,
        disable_plugins = ?plugin_overrides.disable_plugins,
        "applied codex-acp plugin CLI overrides"
    );
}

fn empty_toml_table() -> TomlValue {
    TomlValue::Table(TomlMap::new())
}

fn ensure_table(value: &mut TomlValue) -> &mut TomlMap<String, TomlValue> {
    if !value.is_table() {
        *value = empty_toml_table();
    }

    value
        .as_table_mut()
        .expect("value was normalized to a TOML table")
}

/// Run the Codex ACP agent.
///
/// This sets up an ACP agent that communicates over stdio, bridging
/// the ACP protocol with the existing codex-rs infrastructure.
///
/// # Errors
///
/// If unable to parse the config or start the program.
pub async fn run_main(
    codex_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
    plugin_overrides: PluginCliOverrides,
) -> std::io::Result<()> {
    // Install a simple subscriber so `tracing` output is visible.
    // Users can control the log level with `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Parse CLI overrides and load configuration
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;

    let config_overrides = ConfigOverrides {
        codex_linux_sandbox_exe: codex_linux_sandbox_exe.clone(),
        ..ConfigOverrides::default()
    };

    let mut config =
        Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, config_overrides)
            .await
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("error loading config: {e}"),
                )
            })?;
    apply_codex_acp_plugin_overrides(&mut config, &plugin_overrides);

    // Apply residency requirement so the HTTP client sends the
    // x-openai-internal-codex-residency header on all requests.
    codex_login::default_client::set_default_client_residency_requirement(
        config.enforce_residency.value(),
    );

    let agent = Arc::new(codex_agent::CodexAgent::new(
        config,
        codex_linux_sandbox_exe,
    )?);

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    agent
        .serve(ByteStreams::new(stdout, stdin))
        .await
        .map_err(|e| std::io::Error::other(format!("ACP error: {e}")))?;

    Ok(())
}

// Re-export the MCP server types for compatibility
pub use codex_mcp_server::{
    CodexToolCallParam, CodexToolCallReplyParam, ExecApprovalElicitRequestParams,
    ExecApprovalResponse, PatchApprovalElicitRequestParams, PatchApprovalResponse,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_enabled(value: &toml::Value, plugin_id: &str) -> Option<bool> {
        value
            .get("plugins")
            .and_then(|plugins| plugins.get(plugin_id))
            .and_then(|plugin| plugin.get("enabled"))
            .and_then(toml::Value::as_bool)
    }

    #[test]
    fn empty_plugin_overrides_preserve_config_plugins() {
        let mut user_config = toml::Value::Table(toml::toml! {
            model = "gpt-5"
            [plugins."browser-use@openai-bundled"]
            enabled = true
            extra = "keep"
        });

        apply_plugin_cli_overrides_to_user_config(&mut user_config, &PluginCliOverrides::default());

        assert_eq!(
            plugin_enabled(&user_config, "browser-use@openai-bundled"),
            Some(true)
        );
        assert_eq!(
            plugin_enabled(&user_config, "computer-use@openai-bundled"),
            None
        );
        assert_eq!(
            user_config.get("model").and_then(toml::Value::as_str),
            Some("gpt-5")
        );
        assert_eq!(
            user_config
                .get("plugins")
                .and_then(|plugins| plugins.get("browser-use@openai-bundled"))
                .and_then(|plugin| plugin.get("extra"))
                .and_then(toml::Value::as_str),
            Some("keep")
        );
    }

    #[test]
    fn plugin_cli_overrides_set_enabled_state_and_preserve_extra_fields() {
        let mut user_config = toml::Value::Table(toml::toml! {
            model = "gpt-5"
            [plugins."browser-use@openai-bundled"]
            enabled = true
            extra = "keep"
        });
        let plugin_overrides = PluginCliOverrides::new(
            vec!["documents@openai-primary-runtime".to_string()],
            vec!["browser-use@openai-bundled".to_string()],
        )
        .expect("valid plugin overrides");

        apply_plugin_cli_overrides_to_user_config(&mut user_config, &plugin_overrides);

        assert_eq!(
            plugin_enabled(&user_config, "documents@openai-primary-runtime"),
            Some(true)
        );
        assert_eq!(
            plugin_enabled(&user_config, "browser-use@openai-bundled"),
            Some(false)
        );
        assert_eq!(
            user_config
                .get("plugins")
                .and_then(|plugins| plugins.get("browser-use@openai-bundled"))
                .and_then(|plugin| plugin.get("extra"))
                .and_then(toml::Value::as_str),
            Some("keep")
        );
    }

    #[test]
    fn plugin_cli_overrides_normalize_comma_lists_and_deduplicate() {
        assert_eq!(
            PluginCliOverrides::new(
                vec![" foo ,bar,, foo ".to_string()],
                vec![" baz ".to_string()],
            )
            .expect("valid plugin overrides"),
            PluginCliOverrides {
                enable_plugins: vec!["foo".to_string(), "bar".to_string()],
                disable_plugins: vec!["baz".to_string()],
            }
        );
    }

    #[test]
    fn plugin_cli_overrides_reject_conflicts() {
        let err = PluginCliOverrides::new(vec!["foo".to_string()], vec!["foo".to_string()])
            .expect_err("conflicting plugin overrides should fail");

        assert!(err.contains("cannot be both enabled and disabled"));
    }
}
