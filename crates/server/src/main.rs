use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use mise_core::types::Slug;
use mise_server::{AppState, app};
use mise_store::Store;
use tracing::info;

#[derive(Parser)]
#[command(name = "mise-server", version, about = "Sync server for the Mise corpus")]
struct Args {
    /// Corpus root. Default: $MISE_ROOT, or ~/cookbook.
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:7920")]
    listen: SocketAddr,
    /// File holding the bearer token. Falls back to
    /// $CREDENTIALS_DIRECTORY/token (systemd), then $MISE_TOKEN.
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Create the corpus if it doesn't exist yet.
    #[arg(long)]
    init: bool,
    /// Location for --init.
    #[arg(long, default_value = "home")]
    init_location: String,
    #[arg(long, default_value_t = 2)]
    init_headcount: u32,
}

fn read_token(args: &Args) -> Result<String> {
    let path = args
        .token_file
        .clone()
        .or_else(|| {
            std::env::var_os("CREDENTIALS_DIRECTORY").map(|d| PathBuf::from(d).join("token"))
        });
    let token = match path {
        Some(p) => std::fs::read_to_string(&p)
            .with_context(|| format!("reading token file {}", p.display()))?,
        None => std::env::var("MISE_TOKEN")
            .context("no --token-file, $CREDENTIALS_DIRECTORY, or $MISE_TOKEN")?,
    };
    let token = token.trim().to_string();
    if token.len() < 16 {
        bail!("refusing a bearer token shorter than 16 characters");
    }
    Ok(token)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let root = match &args.root {
        Some(r) => r.clone(),
        None => match std::env::var_os("MISE_ROOT") {
            Some(r) => PathBuf::from(r),
            None => std::env::home_dir().context("no home directory; pass --root")?.join("cookbook"),
        },
    };
    let token = read_token(&args)?;

    let store = match Store::open(&root) {
        Ok(store) => store,
        Err(mise_store::StoreError::NoCorpus(_)) if args.init => {
            let location = Slug::new(args.init_location.as_str())
                .map_err(|e| anyhow::Error::msg(e.to_string()))?;
            let mut store = Store::create(&root, &location, args.init_headcount)?;
            store.export("init: empty corpus")?;
            info!("initialized corpus at {}", root.display());
            store
        }
        Err(e) => return Err(e.into()),
    };

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    info!("serving corpus {} on {}", root.display(), args.listen);
    axum::serve(listener, app(AppState::new(store, token)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
