mod app;
mod config;
mod discovery;
mod packages;
mod package_check;
mod partition;
mod plan;
mod preflight;
mod tui;

use std::{fs, io::IsTerminal, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, bail};
use clap::Parser;
use config::InstallConfig;

#[derive(Debug, Parser)]
#[command(
    name = "artix-setup",
    version,
    about = "A configuration-driven Artix Linux installer prototype"
)]
struct Args {
    /// Load an installation profile from a TOML file.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Print the planned actions and exit. This never runs system commands.
    #[arg(long, alias = "dry-run")]
    plan: bool,
    /// Validate the configuration and exit.
    #[arg(long)]
    validate: bool,
    /// Print read-only hardware and block-device discovery, then exit.
    #[arg(long)]
    hardware: bool,
    /// Check the live environment and selected configuration without changing the host.
    #[arg(long)]
    preflight: bool,
    /// Check planned packages against currently enabled pacman sync databases; does not refresh them.
    #[arg(long)]
    check_packages: bool,
    /// Write the default configuration to FILE and exit.
    #[arg(long, value_name = "FILE")]
    write_default: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if args.hardware {
        println!("{}", discovery::discover().report());
        return Ok(());
    }
    if let Some(path) = args.write_default {
        let contents = toml::to_string_pretty(&InstallConfig::default())
            .context("serialize default configuration")?;
        fs::write(&path, contents)
            .with_context(|| format!("write default configuration to {}", path.display()))?;
        println!("Wrote {}", path.display());
        return Ok(());
    }
    let config = match args.config {
        Some(path) => {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("read configuration {}", path.display()))?;
            toml::from_str::<InstallConfig>(&source)
                .with_context(|| format!("parse configuration {}", path.display()))?
        }
        None => InstallConfig::default(),
    };
    if args.preflight {
        let hardware = discovery::discover();
        let report = preflight::run(&config, &hardware);
        print!("{}", report.render());
        if !report.is_ready() {
            bail!("live environment preflight failed");
        }
        return Ok(());
    }
    if args.check_packages {
        let report = package_check::check(&config, std::path::Path::new("pacman"))?;
        print!("{}", report.render());
        if report.has_problems() {
            bail!("one or more packages are missing or could not be checked");
        }
        return Ok(());
    }
    if args.plan {
        print!("{}", plan::format_plan(&config));
        return Ok(());
    }
    config.validate().map_err(|issues| {
        anyhow::anyhow!(
            "configuration validation failed:\n- {}",
            issues.join("\n- ")
        )
    })?;
    if args.validate {
        println!("Configuration is valid.");
        if config.disk.target.is_none() {
            println!("No target disk is configured; disk operations remain blocked.");
        }
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("interactive mode needs a terminal; use --plan or --validate in scripts");
    }
    let hardware = discovery::discover();
    tui::run(config, hardware)
}
