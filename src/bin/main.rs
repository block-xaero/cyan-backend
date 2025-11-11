// src/bin/main.rs
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use colored::*;
use rusqlite::{params, Connection};
use std::ffi::CString;
use std::io::{self, Write};
use std::path::{PathBuf};
use std::env;

// FFI from the library crate
use cyan_backend::{
    cyan_init,
    cyan_set_data_dir,
    cyan_set_discovery_key,
    cyan_create_group,
};

#[derive(Parser, Debug)]
#[command(name = "cyan-cli")]
#[command(about = "Cyan P2P Whiteboard CLI")]
struct Cli {
    /// Port offset for this instance (so you can run multiple in parallel)
    #[arg(long, default_value_t = 0)]
    port_offset: u16,

    /// Data dir (defaults to ./cyan-data-{port_offset})
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Discovery key; all peers must share the same key to see each other
    #[arg(long, default_value = "cyan-dev")]
    discovery_key: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start an interactive prompt
    Interactive,
    /// Create a group (name, icon, color)
    CreateGroup {
        name: String,
        #[arg(default_value = "")]
        icon: String,
        #[arg(default_value = "#0088cc")]
        color: String,
    },
    /// List local groups from the DB
    ListGroups,
}

struct CyanCli {
    data_dir_abs: PathBuf,   // absolute path (critical when lib chdirs)
    prompt_tag: String,
}

impl CyanCli {
    fn new(data_dir_abs: PathBuf, port_offset: u16) -> Self {
        Self {
            data_dir_abs,
            prompt_tag: format!("cyan@{}>", port_offset),
        }
    }

    fn init_backend(&self, discovery_key: &str) -> Result<()> {
        // Make sure the directory exists BEFORE we pass it to the backend
        std::fs::create_dir_all(&self.data_dir_abs)?;

        println!("{}", format!("🚀 Initializing Cyan instance in {:?}…", &self.data_dir_abs).cyan());

        // Tell the backend where to put cyan.db/blobs
        // The backend will chdir() into this directory.
        let path_c = CString::new(self.data_dir_abs.to_string_lossy().into_owned())?;
        if !unsafe { cyan_set_data_dir(path_c.as_ptr()) } {
            return Err(anyhow!("Failed to set data directory"));
        }

        // Both processes must share the SAME discovery key
        let key_c = CString::new(discovery_key.to_string())?;
        if !unsafe { cyan_set_discovery_key(key_c.as_ptr()) } {
            return Err(anyhow!("Failed to set discovery key"));
        }

        // Spin up the actors
        if !unsafe { cyan_init() } {
            return Err(anyhow!("Failed to initialize Cyan backend"));
        }

        println!("{}", "✅ Cyan initialized!".green());
        println!("📂 Data directory: {:?}", &self.data_dir_abs);
        println!();

        Ok(())
    }

    async fn interactive_mode(&self) -> Result<()> {
        println!("🎨 Cyan Interactive Mode");
        println!("Type 'help' for commands, 'quit' to exit");
        println!();

        let mut line = String::new();
        loop {
            print!("{} ", self.prompt_tag);
            io::stdout().flush().ok();

            line.clear();
            if io::stdin().read_line(&mut line)? == 0 {
                break;
            }
            let cmd = line.trim();
            match cmd {
                "quit" | "exit" => break,
                "help" => Self::print_help(),
                "lg" | "list-groups" => {
                    self.list_groups().await?;
                }
                _ if cmd.starts_with("create-group") => {
                    // create-group <name>; icon/color defaulted
                    let name = cmd.split_once(' ')
                        .map(|(_, rest)| rest.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("New Group");
                    self.create_group(name.to_string(), "".to_string(), "#0088cc".to_string()).await?;
                }
                "" => {},
                other => {
                    eprintln!("Unknown command: {other}");
                }
            }
        }
        Ok(())
    }

    async fn create_group(&self, name: String, icon: String, color: String) -> Result<()> {
        let name_c = CString::new(name.clone())?;
        let icon_c = CString::new(icon)?;
        let color_c = CString::new(color)?;
        let ok = unsafe { cyan_create_group(name_c.as_ptr(), icon_c.as_ptr(), color_c.as_ptr(), 0) };
        if !ok {
            return Err(anyhow!("cyan_create_group failed"));
        }
        println!("{}", "Group create sent.".green());
        Ok(())
    }

    async fn list_groups(&self) -> Result<()> {
        // IMPORTANT: use absolute path so we don't double-prefix after lib chdir().
        let db_path = self.data_dir_abs.join("cyan.db");
        let conn = Connection::open(db_path)?;
        let mut stmt = conn.prepare("SELECT id, name, color, created_at FROM groups ORDER BY created_at ASC")?;
        let mut rows = stmt.query([])?;
        println!("📋 Groups:\n────────────────────────────────────────────────────────────");
        let mut any = false;
        while let Some(row) = rows.next()? {
            any = true;
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let color: String = row.get(2)?;
            let ts: i64 = row.get(3)?;
            let short = &id[..8];
            println!("📁 \"{}\" ({})", name, short);
            println!("  🎨 Color: {}", color);
            println!("  📅 Created: {}", chrono::NaiveDateTime::from_timestamp_opt(ts, 0).unwrap_or_default());
            println!();
        }
        if !any {
            println!("No groups found. Create one with 'create-group'!");
        }
        Ok(())
    }

    fn print_help() {
        println!("Commands:");
        println!("  help                 Show this help");
        println!("  lg | list-groups     List groups");
        println!("  create-group <name>  Create a group (icon/color defaulted)");
        println!("  quit                 Exit");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Build the default relative data dir
    let data_dir_rel = cli
        .data_dir
        .unwrap_or_else(|| PathBuf::from(format!("./cyan-data-{}", cli.port_offset)));

    // Convert to ABSOLUTE path BEFORE calling the backend (which will chdir)
    let cwd = env::current_dir()?;
    let data_dir_abs = if data_dir_rel.is_absolute() {
        data_dir_rel
    } else {
        cwd.join(data_dir_rel)
    };

    let cyan_cli = CyanCli::new(data_dir_abs.clone(), cli.port_offset);
    cyan_cli.init_backend(&cli.discovery_key)?;

    match cli.command {
        Some(Commands::Interactive) | None => cyan_cli.interactive_mode().await?,
        Some(Commands::CreateGroup { name, icon, color }) => cyan_cli.create_group(name, icon, color).await?,
        Some(Commands::ListGroups) => cyan_cli.list_groups().await?,
    }

    Ok(())
}