use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

mod config;
mod history;
mod lock;
mod models;
mod prune;
mod restore;
mod sync;
mod util;

use config::ConfigFile;
use history::list_snapshots;
use prune::{run_prune, PruneOptions};
use restore::{run_restore, RestoreOptions};
use sync::{run_create, SyncOptions};
use util::parse_sources;

#[derive(Parser, Debug)]
#[command(
    name = "vcopy",
    author = "Sami Lehtinen",
    version = "0.2.0",
    about = "Browseable zero-link backup tool with history",
    long_about = "vcopy creates directly browseable, uncompressed filesystem mirrors on external drives and non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS) while preserving a full historical change record in .version-history/."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Source paths followed by destination directory (<SOURCES>... <DEST>)
    #[arg(value_name = "SOURCES... DEST", trailing_var_arg = true)]
    args: Vec<PathBuf>,

    /// Configuration file (JSON or TOML) containing backup settings
    #[arg(short = 'c', long = "config", value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// Destination directory (overrides positional destination or config)
    #[arg(
        short = 'd',
        long = "dest",
        alias = "destination",
        value_name = "DEST",
        global = true
    )]
    dest: Option<PathBuf>,

    /// Read source paths from a text file, glob pattern, or stdin ('-' for stdin) (one path per line, # for comments). Can be specified multiple times.
    #[arg(
        short = 'f',
        long = "paths-from",
        alias = "files-from",
        visible_alias = "sources-from",
        value_name = "PATTERN_OR_STDIN",
        global = true,
        action = clap::ArgAction::Append,
        value_delimiter = ','
    )]
    paths_from: Vec<String>,

    /// Maximum number of snapshots to keep (overrides GFS retention)
    #[arg(short = 'n', long = "max-versions", value_name = "N")]
    max_versions: Option<usize>,

    /// Number of hourly snapshot buckets to retain in GFS mode (default: 24)
    #[arg(long = "keep-hourly", value_name = "N")]
    keep_hourly: Option<usize>,

    /// Number of daily snapshot buckets to retain in GFS mode (default: 7)
    #[arg(long = "keep-daily", value_name = "N")]
    keep_daily: Option<usize>,

    /// Number of weekly snapshot buckets to retain in GFS mode (default: 4)
    #[arg(long = "keep-weekly", value_name = "N")]
    keep_weekly: Option<usize>,

    /// Number of monthly snapshot buckets to retain in GFS mode (default: 12)
    #[arg(long = "keep-monthly", value_name = "N")]
    keep_monthly: Option<usize>,

    /// Number of yearly snapshot buckets to retain in GFS mode (default: 1)
    #[arg(long = "keep-yearly", value_name = "N")]
    keep_yearly: Option<usize>,

    /// Perform a trial run without making any filesystem modifications
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Print detailed information about copied, modified, and deleted files
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Copy files into destination root without source directory prefixes
    #[arg(long = "flatten")]
    flatten: bool,

    /// Target snapshot timestamp (YYYY-MM-DD_HHMMSS or prefix) or 'latest'
    #[arg(long = "target-time", value_name = "TIMESTAMP")]
    target_time: Option<String>,

    /// Save and restore file permissions in snapshot metadata (default: disabled)
    #[arg(short = 'p', long = "preserve-permissions", global = true)]
    preserve_permissions: bool,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Commands {
    /// Auto create backup and prune old snapshots (default)
    Auto {
        /// Source paths followed by destination directory (<SOURCES>... <DEST>)
        #[arg(value_name = "SOURCES... DEST", trailing_var_arg = true)]
        paths: Vec<PathBuf>,
    },
    /// Create a new backup snapshot without pruning
    Create {
        /// Source paths followed by destination directory (<SOURCES>... <DEST>)
        #[arg(value_name = "SOURCES... DEST", trailing_var_arg = true)]
        paths: Vec<PathBuf>,
    },
    /// Prune old snapshots according to retention policy
    Prune {
        /// Maximum number of snapshots to keep (overrides GFS retention)
        #[arg(short = 'n', long = "max-versions", value_name = "N")]
        max_versions: Option<usize>,

        /// Number of hourly snapshot buckets to retain
        #[arg(long = "keep-hourly", value_name = "N")]
        keep_hourly: Option<usize>,

        /// Number of daily snapshot buckets to retain
        #[arg(long = "keep-daily", value_name = "N")]
        keep_daily: Option<usize>,

        /// Number of weekly snapshot buckets to retain
        #[arg(long = "keep-weekly", value_name = "N")]
        keep_weekly: Option<usize>,

        /// Number of monthly snapshot buckets to retain
        #[arg(long = "keep-monthly", value_name = "N")]
        keep_monthly: Option<usize>,

        /// Number of yearly snapshot buckets to retain
        #[arg(long = "keep-yearly", value_name = "N")]
        keep_yearly: Option<usize>,

        /// Simulate pruning without deleting snapshot directories
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Print details about snapshots being kept and pruned
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Destination backup directory containing .version-history/
        #[arg(value_name = "DEST")]
        dest: Option<PathBuf>,
    },
    /// Restore files from a specific snapshot or point in time
    Restore {
        /// Target snapshot timestamp (YYYY-MM-DD_HHMMSS) or 'latest'
        #[arg(long = "target-time", value_name = "TIMESTAMP")]
        target_time: Option<String>,

        /// Print restored file paths
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Backup directory containing mirror and .version-history/
        #[arg(value_name = "DEST")]
        dest: Option<PathBuf>,

        /// Target directory where restored files will be written
        #[arg(value_name = "OUTPUT_DIR")]
        target_out: Option<PathBuf>,
    },
    /// List all snapshots in destination with summary statistics
    List {
        /// Backup directory containing .version-history/
        #[arg(value_name = "DEST")]
        dest: Option<PathBuf>,
    },
}

fn resolve_sources_and_dest(
    cli_paths: Vec<PathBuf>,
    cli_dest: Option<&PathBuf>,
    cli_paths_from: &[String],
    config: Option<&ConfigFile>,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    let mut sources = Vec::new();
    let dest: PathBuf;

    let config_dest = config.and_then(|c| c.dest.clone());
    let mut effective_paths_from = cli_paths_from.to_vec();
    if let Some(cfg) = config {
        effective_paths_from.extend(cfg.get_paths_from());
    }

    if let Some(d) = cli_dest {
        dest = d.clone();
        sources.extend(cli_paths);
    } else if !cli_paths.is_empty() {
        if cli_paths.len() == 1 {
            if let Some(cd) = config_dest {
                sources.push(cli_paths[0].clone());
                dest = cd;
            } else {
                dest = cli_paths[0].clone();
            }
        } else {
            let mut p = cli_paths;
            dest = p.pop().unwrap();
            sources.extend(p);
        }
    } else if let Some(cd) = config_dest {
        dest = cd;
    } else {
        return Err(
            "Error: Destination directory required via arguments, --dest, or --config.".to_string(),
        );
    }

    if let Some(cfg) = config {
        sources.extend(cfg.get_sources());
    }

    let parsed_sources = parse_sources(&sources, &effective_paths_from)
        .map_err(|e| format!("Error reading source paths: {}", e))?;

    if parsed_sources.is_empty() {
        return Err(
            "Error: No source paths provided via arguments, --paths-from, or --config.".to_string(),
        );
    }

    let mut unique = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for s in parsed_sources {
        if seen.insert(s.clone()) {
            unique.push(s);
        }
    }

    Ok((unique, dest))
}

fn execute() -> Result<(), String> {
    let cli = Cli::parse();

    let cfg = match &cli.config {
        Some(p) => Some(crate::config::load_config(p)?),
        None => None,
    };

    let flatten = cli.flatten || cfg.as_ref().and_then(|c| c.flatten).unwrap_or(false);
    let dry_run = cli.dry_run || cfg.as_ref().and_then(|c| c.dry_run).unwrap_or(false);
    let verbose = cli.verbose || cfg.as_ref().and_then(|c| c.verbose).unwrap_or(false);
    let preserve_permissions = cli.preserve_permissions
        || cfg
            .as_ref()
            .and_then(|c| c.preserve_permissions)
            .unwrap_or(false);

    let max_versions = cli
        .max_versions
        .or_else(|| cfg.as_ref().and_then(|c| c.max_versions));
    let keep_hourly = cli
        .keep_hourly
        .or_else(|| cfg.as_ref().and_then(|c| c.keep_hourly))
        .unwrap_or(24);
    let keep_daily = cli
        .keep_daily
        .or_else(|| cfg.as_ref().and_then(|c| c.keep_daily))
        .unwrap_or(7);
    let keep_weekly = cli
        .keep_weekly
        .or_else(|| cfg.as_ref().and_then(|c| c.keep_weekly))
        .unwrap_or(4);
    let keep_monthly = cli
        .keep_monthly
        .or_else(|| cfg.as_ref().and_then(|c| c.keep_monthly))
        .unwrap_or(12);
    let keep_yearly = cli
        .keep_yearly
        .or_else(|| cfg.as_ref().and_then(|c| c.keep_yearly))
        .unwrap_or(1);
    let target_time = cli
        .target_time
        .or_else(|| cfg.as_ref().and_then(|c| c.target_time.clone()));

    let sync_opts = SyncOptions {
        flatten,
        dry_run,
        verbose,
        preserve_permissions,
    };

    let prune_opts = PruneOptions {
        max_versions,
        keep_hourly,
        keep_daily,
        keep_weekly,
        keep_monthly,
        keep_yearly,
        dry_run,
        verbose,
    };

    let effective_command = cli.command.or_else(|| {
        cfg.as_ref()
            .and_then(|c| c.command.as_deref())
            .and_then(|cmd_str| match cmd_str.to_ascii_lowercase().as_str() {
                "prune" => Some(Commands::Prune {
                    max_versions: None,
                    keep_hourly: None,
                    keep_daily: None,
                    keep_weekly: None,
                    keep_monthly: None,
                    keep_yearly: None,
                    dry_run: false,
                    verbose: false,
                    dest: None,
                }),
                "restore" => Some(Commands::Restore {
                    target_time: None,
                    verbose: false,
                    dest: None,
                    target_out: None,
                }),
                "list" => Some(Commands::List { dest: None }),
                "create" => Some(Commands::Create { paths: Vec::new() }),
                "auto" => Some(Commands::Auto { paths: Vec::new() }),
                _ => None,
            })
    });

    match effective_command {
        None => {
            if cli.args.is_empty()
                && cli.dest.is_none()
                && cli.paths_from.is_empty()
                && cfg.is_none()
            {
                eprintln!("Error: destination or sources missing. Run with --help.");
                process::exit(1);
            }
            let (sources, dest) = resolve_sources_and_dest(
                cli.args,
                cli.dest.as_ref(),
                &cli.paths_from,
                cfg.as_ref(),
            )?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
            run_prune(&prune_opts, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Auto { paths }) => {
            let combined = if paths.is_empty() { cli.args } else { paths };
            let (sources, dest) = resolve_sources_and_dest(
                combined,
                cli.dest.as_ref(),
                &cli.paths_from,
                cfg.as_ref(),
            )?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
            run_prune(&prune_opts, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Create { paths }) => {
            let combined = if paths.is_empty() { cli.args } else { paths };
            let (sources, dest) = resolve_sources_and_dest(
                combined,
                cli.dest.as_ref(),
                &cli.paths_from,
                cfg.as_ref(),
            )?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Prune {
            max_versions: p_max_versions,
            keep_hourly: p_keep_hourly,
            keep_daily: p_keep_daily,
            keep_weekly: p_keep_weekly,
            keep_monthly: p_keep_monthly,
            keep_yearly: p_keep_yearly,
            dry_run: p_dry_run,
            verbose: p_verbose,
            dest: p_dest,
        }) => {
            let specific_prune_opts = PruneOptions {
                max_versions: p_max_versions.or(max_versions),
                keep_hourly: p_keep_hourly.unwrap_or(keep_hourly),
                keep_daily: p_keep_daily.unwrap_or(keep_daily),
                keep_weekly: p_keep_weekly.unwrap_or(keep_weekly),
                keep_monthly: p_keep_monthly.unwrap_or(keep_monthly),
                keep_yearly: p_keep_yearly.unwrap_or(keep_yearly),
                dry_run: p_dry_run || dry_run,
                verbose: p_verbose || verbose,
            };
            let effective_dest = p_dest
                .or_else(|| cli.dest.clone())
                .or_else(|| cfg.as_ref().and_then(|c| c.dest.clone()))
                .ok_or_else(|| {
                    "Error: Destination directory required via argument, --dest, or --config."
                        .to_string()
                })?;
            run_prune(&specific_prune_opts, &effective_dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Restore {
            target_time: r_target_time,
            verbose: r_verbose,
            dest: r_dest,
            target_out: r_target_out,
        }) => {
            let restore_opts = RestoreOptions {
                target_time: r_target_time.or_else(|| target_time.clone()),
                verbose: r_verbose || verbose,
                preserve_permissions,
            };
            let effective_dest = r_dest
                .or_else(|| cli.dest.clone())
                .or_else(|| cfg.as_ref().and_then(|c| c.dest.clone()))
                .ok_or_else(|| {
                    "Error: Backup destination directory required via argument, --dest, or --config."
                        .to_string()
                })?;
            let effective_out = r_target_out
                .or_else(|| cfg.as_ref().and_then(|c| c.target_out.clone()))
                .ok_or_else(|| {
                    "Error: Restore output directory required via argument or --config.".to_string()
                })?;
            run_restore(&restore_opts, &effective_dest, &effective_out)
                .map_err(|e| e.to_string())?;
        }
        Some(Commands::List { dest: l_dest }) => {
            let effective_dest = l_dest
                .or_else(|| cli.dest.clone())
                .or_else(|| cfg.as_ref().and_then(|c| c.dest.clone()))
                .ok_or_else(|| {
                    "Error: Backup destination directory required via argument, --dest, or --config."
                        .to_string()
                })?;
            run_list(&effective_dest)?;
        }
    }

    Ok(())
}

fn run_list(dest: &Path) -> Result<(), String> {
    if history::is_flat_repo(dest) {
        if let Some(flat) = history::load_flat_manifest(dest) {
            println!(
                "{:<26} {:<10} {:<10} {:<10} {:<15}",
                "Snapshot (flat)", "Copied", "Modified", "Deleted", "Transferred"
            );
            println!("{}", "-".repeat(75));

            for s in &flat.snapshots {
                println!(
                    "{:<26} {:<10} {:<10} {:<10} {:<15}",
                    s.snapshot_name,
                    s.summary.files_copied,
                    s.summary.files_modified,
                    s.summary.files_deleted,
                    format!("{} bytes", s.summary.bytes_transferred)
                );
            }

            println!(
                "{:<26} {:<10} {:<10} {:<10} {:<15}",
                format!("{} (live)", flat.baseline.snapshot_name),
                flat.baseline.summary.files_copied,
                flat.baseline.summary.files_modified,
                flat.baseline.summary.files_deleted,
                format!("{} bytes", flat.baseline.summary.bytes_transferred)
            );
            return Ok(());
        }
    }

    let snapshots = list_snapshots(dest).map_err(|e| e.to_string())?;
    let baseline = history::load_baseline(dest);
    if snapshots.is_empty() && baseline.is_none() {
        println!("No snapshots found in {}", dest.display());
        return Ok(());
    }

    println!(
        "{:<26} {:<10} {:<10} {:<10} {:<15}",
        "Snapshot", "Copied", "Modified", "Deleted", "Transferred"
    );
    println!("{}", "-".repeat(75));

    for s in &snapshots {
        if let Ok(m) = history::load_manifest(&s.path) {
            println!(
                "{:<26} {:<10} {:<10} {:<10} {:<15}",
                s.name,
                m.summary.files_copied,
                m.summary.files_modified,
                m.summary.files_deleted,
                format!("{} bytes", m.summary.bytes_transferred)
            );
        } else {
            println!("{:<26} (corrupt or unreadable manifest)", s.name);
        }
    }

    if let Some(b) = baseline {
        let is_newer = snapshots
            .last()
            .is_none_or(|last| b.snapshot_name.as_str() > last.name.as_str());
        if is_newer && !b.snapshot_name.is_empty() {
            println!(
                "{:<26} {:<10} {:<10} {:<10} {:<15}",
                format!("{} (live)", b.snapshot_name),
                b.summary.files_copied,
                b.summary.files_modified,
                b.summary.files_deleted,
                format!("{} bytes", b.summary.bytes_transferred)
            );
        }
    }

    Ok(())
}

fn main() {
    if let Err(err) = execute() {
        eprintln!("{}", err);
        process::exit(1);
    }
}
