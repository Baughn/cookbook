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
    /// File holding the Anthropic API key for the assistant. Falls back to
    /// $CREDENTIALS_DIRECTORY/anthropic (systemd), then $ANTHROPIC_API_KEY.
    /// Without any, the server runs sync-only.
    #[arg(long)]
    anthropic_key_file: Option<PathBuf>,
    /// Model for the assistant.
    #[arg(long, default_value = mise_assistant::client::DEFAULT_MODEL)]
    model: String,
    /// Anthropic API endpoint override (proxies, scripted E2E fakes).
    #[arg(long, default_value = mise_assistant::client::DEFAULT_BASE_URL)]
    anthropic_base_url: String,
    /// Directory with the built web app; served at /. Omit for sync/API only.
    #[arg(long)]
    static_dir: Option<PathBuf>,
    /// Create the corpus if it doesn't exist yet.
    #[arg(long)]
    init: bool,
    /// Location for --init.
    #[arg(long, default_value = "home")]
    init_location: String,
    #[arg(long, default_value_t = 2)]
    init_headcount: u32,
}

fn read_anthropic_key(args: &Args) -> Option<String> {
    let path = args.anthropic_key_file.clone().or_else(|| {
        std::env::var_os("CREDENTIALS_DIRECTORY").map(|d| PathBuf::from(d).join("anthropic"))
    });
    let raw = match path {
        Some(p) if p.exists() => std::fs::read_to_string(&p).ok()?,
        _ => std::env::var("ANTHROPIC_API_KEY").ok()?,
    };
    let key = raw.trim().to_string();
    (!key.is_empty()).then_some(key)
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
    // Dev convenience: a .env can supply MISE_TOKEN / MISE_ROOT. In
    // production there is no .env; the token arrives via LoadCredential.
    let _ = dotenvy::dotenv();
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
            let mut store = Store::create(&root, &location, args.init_headcount, jiff::Timestamp::now())?;
            store.export("init: empty corpus")?;
            info!("initialized corpus at {}", root.display());
            store
        }
        Err(e) => return Err(e.into()),
    };

    let mut state = AppState::new(store, token);
    match read_anthropic_key(&args) {
        Some(api_key) => {
            info!("assistant enabled (model {})", args.model);
            state = state.with_chat(mise_server::ChatConfig {
                api_key,
                model: args.model.clone(),
                base_url: args.anthropic_base_url.clone(),
            });
        }
        None => info!("no Anthropic key; running sync-only"),
    }
    if let Some(dir) = &args.static_dir {
        state = state.with_static_dir(dir.clone());
    }

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    info!("serving corpus {} on {}", root.display(), args.listen);
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
