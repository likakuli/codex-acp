//! Codex ACP - An Agent Client Protocol implementation for Codex.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use agent_client_protocol::ByteStreams;
use codex_config::CONFIG_TOML_FILE;
use codex_core::config::{Config, ConfigOverrides};
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use toml::{Value as TomlValue, map::Map as TomlMap};
use tracing_subscriber::EnvFilter;

mod codex_agent;
mod thread;

const CODEX_ACP_DISABLED_PLUGINS_ENV: &str = "CODEX_ACP_DISABLED_PLUGINS";
const DEFAULT_CODEX_ACP_DISABLED_PLUGINS: &[&str] =
    &["computer-use@openai-bundled", "browser-use@openai-bundled"];

fn default_codex_acp_disabled_plugins() -> Vec<String> {
    DEFAULT_CODEX_ACP_DISABLED_PLUGINS
        .iter()
        .map(|plugin| (*plugin).to_string())
        .collect()
}

fn parse_disabled_plugins_override(value: Option<&str>) -> Vec<String> {
    match value {
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|plugin| !plugin.is_empty())
            .map(ToString::to_string)
            .collect(),
        None => default_codex_acp_disabled_plugins(),
    }
}

fn disabled_plugins_from_env() -> Vec<String> {
    match std::env::var(CODEX_ACP_DISABLED_PLUGINS_ENV) {
        Ok(value) => parse_disabled_plugins_override(Some(value.as_str())),
        Err(std::env::VarError::NotPresent | std::env::VarError::NotUnicode(_)) => {
            default_codex_acp_disabled_plugins()
        }
    }
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

fn apply_disabled_plugins_to_user_config(user_config: &mut TomlValue, disabled_plugins: &[String]) {
    if disabled_plugins.is_empty() {
        return;
    }

    let root = ensure_table(user_config);
    let plugins = root
        .entry("plugins".to_string())
        .or_insert_with(empty_toml_table);
    let plugins = ensure_table(plugins);

    for plugin_id in disabled_plugins {
        let plugin = plugins
            .entry(plugin_id.clone())
            .or_insert_with(empty_toml_table);
        let plugin = ensure_table(plugin);
        plugin.insert("enabled".to_string(), TomlValue::Boolean(false));
    }
}

fn apply_codex_acp_plugin_policy(config: &mut Config) {
    let disabled_plugins = disabled_plugins_from_env();
    if disabled_plugins.is_empty() {
        tracing::debug!("codex-acp plugin disable policy is empty");
        return;
    }

    let mut user_config = config
        .config_layer_stack
        .get_user_layer()
        .map(|layer| layer.config.clone())
        .unwrap_or_else(empty_toml_table);
    apply_disabled_plugins_to_user_config(&mut user_config, &disabled_plugins);

    let user_config_path =
        AbsolutePathBuf::resolve_path_against_base(CONFIG_TOML_FILE, &config.codex_home);
    config.config_layer_stack = config
        .config_layer_stack
        .with_user_config(&user_config_path, user_config);
    tracing::info!(?disabled_plugins, "applied codex-acp plugin disable policy");
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
    apply_codex_acp_plugin_policy(&mut config);

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
    fn codex_acp_plugin_policy_disables_default_host_plugins() {
        let mut user_config = toml::Value::Table(toml::toml! {
            model = "gpt-5"
            [plugins."browser-use@openai-bundled"]
            enabled = true
            extra = "keep"
        });

        apply_disabled_plugins_to_user_config(
            &mut user_config,
            &default_codex_acp_disabled_plugins(),
        );

        assert_eq!(
            plugin_enabled(&user_config, "computer-use@openai-bundled"),
            Some(false)
        );
        assert_eq!(
            plugin_enabled(&user_config, "browser-use@openai-bundled"),
            Some(false)
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
    fn codex_acp_plugin_policy_allows_empty_override_to_disable_policy() {
        assert!(parse_disabled_plugins_override(Some(" , ")).is_empty());
    }

    #[test]
    fn codex_acp_plugin_policy_parses_custom_override() {
        assert_eq!(
            parse_disabled_plugins_override(Some(" foo ,bar,, baz ")),
            vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]
        );
    }
}
