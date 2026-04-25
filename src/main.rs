use anyhow::Result;
use clap::{ArgAction, Parser};
use codex_acp::PluginCliOverrides;
use codex_arg0::arg0_dispatch_or_else;
use codex_utils_cli::CliConfigOverrides;

#[derive(Parser, Debug)]
struct AcpCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    /// Enable a Codex plugin for this ACP process, overriding config.toml.
    #[arg(
        long = "enable-plugin",
        value_name = "PLUGIN_ID",
        action = ArgAction::Append,
        value_delimiter = ',',
        global = true,
    )]
    enable_plugins: Vec<String>,

    /// Disable a Codex plugin for this ACP process, overriding config.toml.
    #[arg(
        long = "disable-plugin",
        value_name = "PLUGIN_ID",
        action = ArgAction::Append,
        value_delimiter = ',',
        global = true,
    )]
    disable_plugins: Vec<String>,
}

fn main() -> Result<()> {
    arg0_dispatch_or_else(|args| async move {
        let cli = AcpCli::parse();
        let plugin_overrides = PluginCliOverrides::new(cli.enable_plugins, cli.disable_plugins)
            .map_err(anyhow::Error::msg)?;
        codex_acp::run_main(
            args.codex_linux_sandbox_exe,
            cli.config_overrides,
            plugin_overrides,
        )
        .await?;
        Ok(())
    })
}
