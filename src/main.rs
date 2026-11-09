use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

mod history;
mod lock;
mod models;
mod prune;
mod restore;
mod sync;
mod util;

use history::list_snapshots;
use prune::{run_prune, PruneOptions};
use restore::{run_restore, RestoreOptions};
use sync::{run_create, SyncOptions};
use util::parse_sources;

#[derive(Parser, Debug)]
#[command(
    name = "vcopy",
    author = "Sami Lehtinen",
    version = "0.1.0",
    about = "Browseable zero-link backup tool with history",
    long_about = "vcopy creates directly browseable, uncompressed filesystem mirrors on external drives and non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS) while preserving a full historical change record in .version-history/."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Source paths followed by destination directory (<SOURCES>... <DEST>)
    #[arg(value_name = "SOURCES... DEST", trailing_var_arg = true)]
    args: Vec<PathBuf>,

    /// Read source paths from a text file (one path per line, # for comments)
    #[arg(short = 'f', long = "files-from", value_name = "FILE")]
    files_from: Option<PathBuf>,

    /// Maximum number of snapshots to keep (overrides GFS retention)
    #[arg(short = 'n', long = "max-versions", value_name = "N")]
    max_versions: Option<usize>,

    /// Number of hourly snapshot buckets to retain in GFS mode
    #[arg(long = "keep-hourly", default_value_t = 24, value_name = "N")]
    keep_hourly: usize,

    /// Number of daily snapshot buckets to retain in GFS mode
    #[arg(long = "keep-daily", default_value_t = 7, value_name = "N")]
    keep_daily: usize,

    /// Number of weekly snapshot buckets to retain in GFS mode
    #[arg(long = "keep-weekly", default_value_t = 4, value_name = "N")]
    keep_weekly: usize,

    /// Number of monthly snapshot buckets to retain in GFS mode
    #[arg(long = "keep-monthly", default_value_t = 12, value_name = "N")]
    keep_monthly: usize,

    /// Number of yearly snapshot buckets to retain in GFS mode
    #[arg(long = "keep-yearly", default_value_t = 1, value_name = "N")]
    keep_yearly: usize,

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
    #[arg(short = 'p', long = "preserve-permissions")]
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
        dest: PathBuf,
    },
    /// Restore files from a specific snapshot or point in time
    Restore {
        /// Target snapshot timestamp (YYYY-MM-DD_HHMMSS) or 'latest'
        #[arg(long = "target-time", value_name = "TIMESTAMP")]
        target_time: Option<String>,

        /// Apply saved permissions to restored files
        #[arg(short = 'p', long = "preserve-permissions")]
        preserve_permissions: bool,

        /// Print restored file paths
        #[arg(short = 'v', long = "verbose")]
        verbose: bool,

        /// Backup directory containing mirror and .version-history/
        #[arg(value_name = "DEST")]
        dest: PathBuf,

        /// Target directory where restored files will be written
        #[arg(value_name = "OUTPUT_DIR")]
        target_out: PathBuf,
    },
    /// List all snapshots in destination with summary statistics
    List {
        /// Backup directory containing .version-history/
        #[arg(value_name = "DEST")]
        dest: PathBuf,
    },
}

fn separate_sources_and_dest(
    mut paths: Vec<PathBuf>,
    files_from: Option<&PathBuf>,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    if paths.is_empty() {
        return Err("Error: Destination directory required.".to_string());
    }
    let dest = paths.pop().unwrap();
    let sources = parse_sources(&paths, files_from)
        .map_err(|e| format!("Error reading --files-from: {}", e))?;

    if sources.is_empty() {
        return Err("Error: No sources provided via arguments or --files-from.".to_string());
    }
    Ok((sources, dest))
}

fn execute() -> Result<(), String> {
    let cli = Cli::parse();

    let sync_opts = SyncOptions {
        flatten: cli.flatten,
        dry_run: cli.dry_run,
        verbose: cli.verbose,
        preserve_permissions: cli.preserve_permissions,
    };

    let prune_opts = PruneOptions {
        max_versions: cli.max_versions,
        keep_hourly: cli.keep_hourly,
        keep_daily: cli.keep_daily,
        keep_weekly: cli.keep_weekly,
        keep_monthly: cli.keep_monthly,
        keep_yearly: cli.keep_yearly,
        dry_run: cli.dry_run,
        verbose: cli.verbose,
    };

    match cli.command {
        None => {
            if cli.args.is_empty() && cli.files_from.is_none() {
                eprintln!("Error: destination or sources missing. Run with --help.");
                process::exit(1);
            }
            let (sources, dest) = separate_sources_and_dest(cli.args, cli.files_from.as_ref())?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
            run_prune(&prune_opts, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Auto { paths }) => {
            let combined = if paths.is_empty() { cli.args } else { paths };
            let (sources, dest) = separate_sources_and_dest(combined, cli.files_from.as_ref())?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
            run_prune(&prune_opts, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Create { paths }) => {
            let combined = if paths.is_empty() { cli.args } else { paths };
            let (sources, dest) = separate_sources_and_dest(combined, cli.files_from.as_ref())?;
            run_create(&sync_opts, &sources, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Prune {
            max_versions,
            keep_hourly,
            keep_daily,
            keep_weekly,
            keep_monthly,
            keep_yearly,
            dry_run,
            verbose,
            dest,
        }) => {
            let specific_prune_opts = PruneOptions {
                max_versions: max_versions.or(cli.max_versions),
                keep_hourly: keep_hourly.unwrap_or(cli.keep_hourly),
                keep_daily: keep_daily.unwrap_or(cli.keep_daily),
                keep_weekly: keep_weekly.unwrap_or(cli.keep_weekly),
                keep_monthly: keep_monthly.unwrap_or(cli.keep_monthly),
                keep_yearly: keep_yearly.unwrap_or(cli.keep_yearly),
                dry_run: dry_run || cli.dry_run,
                verbose: verbose || cli.verbose,
            };
            run_prune(&specific_prune_opts, &dest).map_err(|e| e.to_string())?;
        }
        Some(Commands::Restore {
            target_time,
            preserve_permissions,
            verbose,
            dest,
            target_out,
        }) => {
            let restore_opts = RestoreOptions {
                target_time: target_time.or(cli.target_time),
                verbose: verbose || cli.verbose,
                preserve_permissions: preserve_permissions || cli.preserve_permissions,
            };
            run_restore(&restore_opts, &dest, &target_out).map_err(|e| e.to_string())?;
        }
        Some(Commands::List { dest }) => {
            run_list(&dest)?;
        }
    }

    Ok(())
}

fn run_list(dest: &Path) -> Result<(), String> {
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
