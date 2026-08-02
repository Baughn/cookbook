//! `mise`: the M1 surface. A thin edge over the store and the domain math —
//! it reads the wall clock and passes it in as data, mutates the corpus
//! through the store, and re-exports (with a git commit) after every change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use jiff::Zoned;
use jiff::civil::{Date, DateTime};
use mise_assistant::views;
use mise_core::rotation::recency;
use mise_core::types::{CookKind, EffortClass, LogEntry, Slug};
use mise_store::pages::{
    DishRefDoc, FridgeDoc, IngredientDoc, LeadTimeDoc, PantryItemDoc, PortionDoc, QueueDoc,
    QueueEntryDoc, RecipeDoc, StateDoc,
};
use mise_store::{DocId, Store};

mod remote;

#[derive(Parser)]
#[command(name = "mise", version, about = "A living cookbook & meal planner")]
struct Cli {
    /// Corpus root. Default: $MISE_ROOT, or ~/cookbook.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize a fresh corpus, or join an existing one from a server.
    Init {
        /// Ignored with --from (the corpus comes from the server).
        #[arg(long, default_value = "home")]
        location: String,
        #[arg(long, default_value_t = 2)]
        headcount: u32,
        /// Join: pull everything from this server and remember it.
        #[arg(long, requires = "token_source")]
        from: Option<String>,
        #[arg(long, group = "token_source")]
        token: Option<String>,
        #[arg(long, group = "token_source")]
        token_file: Option<PathBuf>,
    },
    /// The saved sync server.
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
    },
    /// Sync with the server: push local changes, pull everyone else's.
    Sync {
        /// Override the saved server URL.
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Show the queue with readiness annotations (default), or edit it.
    Queue {
        #[command(subcommand)]
        cmd: Option<QueueCmd>,
    },
    /// Recipes: create and edit.
    Recipe {
        #[command(subcommand)]
        cmd: RecipeCmd,
    },
    /// Pantry updates at a location.
    Pantry {
        #[command(subcommand)]
        cmd: PantryCmd,
    },
    /// What a location's kitchen has: wok, stand mixer, ...
    Equipment {
        #[command(subcommand)]
        cmd: EquipmentCmd,
    },
    /// Fridge and freezer state.
    Fridge {
        #[command(subcommand)]
        cmd: FridgeCmd,
    },
    /// The append-only cook log.
    Log {
        #[command(subcommand)]
        cmd: LogCmd,
    },
    /// Locations: add one, or switch the active location.
    Location {
        #[command(subcommand)]
        cmd: LocationCmd,
    },
    /// Talk to the assistant: the planning thread, or a page's thread.
    Chat {
        /// What to say.
        message: String,
        /// A page's thread (doc id like recipe/mapo-tofu); default: the
        /// global planning thread.
        #[arg(long)]
        page: Option<String>,
        /// Model override; defaults to the client's default.
        #[arg(long)]
        model: Option<String>,
        /// Attach a photo (jpeg/png/webp/gif) — pantry recon from the CLI.
        /// Repeat for several frames of the same recon.
        #[arg(long = "photo")]
        photos: Vec<std::path::PathBuf>,
    },
    /// Regenerate the markdown export and commit it.
    Export,
}

#[derive(Subcommand)]
enum RemoteCmd {
    /// Save the server URL and token for `mise sync`.
    #[command(group = clap::ArgGroup::new("token_source").required(true).args(["token", "token_file"]))]
    Set {
        url: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    Show,
}

#[derive(Subcommand)]
enum QueueCmd {
    /// Add a dish (or an idea) to the queue.
    Add {
        title: String,
        /// Link to a recipe slug.
        #[arg(long)]
        recipe: Option<String>,
        /// Why it's here: "rotating away from curry", "uses the wakame".
        #[arg(long)]
        reason: Option<String>,
        /// Entry id; defaults to a slug of the title.
        #[arg(long)]
        id: Option<String>,
        /// Add to the someday shelf instead of the active queue.
        #[arg(long)]
        someday: bool,
    },
    /// Remove an entry by id.
    Remove {
        id: String,
        #[arg(long)]
        someday: bool,
    },
}

#[derive(Subcommand)]
enum RecipeCmd {
    /// Create a recipe.
    Add {
        slug: String,
        #[arg(long)]
        title: String,
        #[arg(long, default_value_t = 2)]
        servings: u32,
        #[arg(long, default_value = "weekday")]
        effort: EffortClass,
        /// Rotation tags, k=v (cuisine=sichuan). Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Required equipment slug. Repeatable.
        #[arg(long = "equipment")]
        equipment: Vec<String>,
        /// Lead time in minutes (with --lead-step).
        #[arg(long, requires = "lead_step")]
        lead_minutes: Option<u32>,
        /// The act-now step: "start the marinade".
        #[arg(long, requires = "lead_minutes")]
        lead_step: Option<String>,
        /// Method body: a file path, or - for stdin.
        #[arg(long)]
        body: Option<PathBuf>,
    },
    /// Append an ingredient line.
    Ingredient {
        slug: String,
        text: String,
        /// Pantry-item slug this line draws on.
        #[arg(long)]
        link: Option<String>,
    },
    /// Replace the method body from a file (- for stdin).
    Body { slug: String, file: PathBuf },
}

#[derive(Subcommand)]
enum PantryCmd {
    /// Create or update a pantry item. Only the given fields change.
    Set {
        item: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        presence: Option<String>,
        #[arg(long)]
        tier: Option<String>,
        /// Rough purchase date, YYYY-MM-DD; "today" works.
        #[arg(long)]
        bought: Option<String>,
        #[arg(long)]
        note: Option<String>,
        /// Defaults to the active location.
        #[arg(long)]
        location: Option<String>,
    },
    /// Remove an item entirely (usually you want --presence out instead).
    Remove {
        item: String,
        #[arg(long)]
        location: Option<String>,
    },
}

#[derive(Subcommand)]
enum EquipmentCmd {
    /// Add (or annotate) a piece of equipment at a location.
    Add {
        item: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        location: Option<String>,
    },
    Remove {
        item: String,
        #[arg(long)]
        location: Option<String>,
    },
}

#[derive(Subcommand)]
enum FridgeCmd {
    /// Add a cooked batch to the fridge (or a named freezer).
    Add {
        dish: String,
        #[arg(long)]
        servings: u32,
        /// Cooked/frozen date; defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Put it in this freezer instead of the fridge.
        #[arg(long)]
        freezer: Option<String>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        location: Option<String>,
    },
    /// Remove a portion by id (eaten through, defrosted, gone).
    Remove {
        id: String,
        #[arg(long)]
        freezer: Option<String>,
        #[arg(long)]
        location: Option<String>,
    },
}

#[derive(Subcommand)]
enum LogCmd {
    /// Append a cook to the log.
    Add {
        title: String,
        #[arg(long)]
        recipe: Option<String>,
        #[arg(long, default_value = "meal")]
        kind: String,
        /// Servings produced; defaults to the recipe's.
        #[arg(long)]
        servings: Option<u32>,
        #[arg(long, default_value = "fine")]
        verdict: String,
        /// Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Extra rotation tags k=v; recipe tags are inherited. Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        location: Option<String>,
    },
    /// Show rotation recency over the log.
    Rotation {
        /// Trailing window, days.
        #[arg(long, default_value_t = 28)]
        window: u16,
    },
}

#[derive(Subcommand)]
enum LocationCmd {
    Add {
        name: String,
        #[arg(long, default_value_t = 2)]
        headcount: u32,
    },
    /// Switch the active location (sticky; never guessed).
    Use { name: String },
}

fn main() -> Result<()> {
    // Dev convenience: a .env can supply MISE_ROOT.
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    let root = match &cli.root {
        Some(r) => r.clone(),
        None => match std::env::var_os("MISE_ROOT") {
            Some(r) => PathBuf::from(r),
            None => std::env::home_dir()
                .context("no home directory; pass --root")?
                .join("cookbook"),
        },
    };
    let now = Zoned::now();

    match cli.command {
        Cmd::Init { from: Some(url), token, token_file, .. } => {
            let token = read_token(token, token_file)?;
            let url = remote::normalize_url(&url)?;
            let mut store = match Store::create_bare(&root) {
                Ok(store) => store,
                // A bare corpus with no state doc is a join whose first
                // sync died; the fix is retrying, not "already
                // initialized".
                Err(create_err) => match Store::open(&root) {
                    Ok(store) if !store.exists(&DocId::State)? => store,
                    _ => return Err(create_err.into()),
                },
            };
            remote::save(&root, &remote::Remote { url: url.clone(), token: token.clone() })?;
            let outcome = remote::sync(&mut store, &url, &token).with_context(|| {
                format!(
                    "joined, but the first sync failed — fix the URL or token and run \
                     `mise init --from {url}` again"
                )
            })?;
            store.export("sync: joined corpus")?;
            println!(
                "joined corpus at {} from {url}: {}",
                root.display(),
                remote::describe(&outcome),
            );
            Ok(())
        }
        Cmd::Init { location, headcount, .. } => {
            let location = slug(&location)?;
            let mut store = Store::create(&root, &location, headcount, Zoned::now().timestamp())?;
            store.export("init: empty corpus")?;
            println!("initialized corpus at {} (location: {location})", root.display());
            Ok(())
        }
        command => {
            let mut store = Store::open(&root)?;
            run(&mut store, command, &root, now)
        }
    }
}

fn read_token(token: Option<String>, token_file: Option<PathBuf>) -> Result<String> {
    let raw = match (token, token_file) {
        (Some(t), _) => t,
        (None, Some(p)) => {
            std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?
        }
        (None, None) => bail!("need --token or --token-file"),
    };
    must_trim(&raw, "token")
}

fn run(store: &mut Store, command: Cmd, root: &Path, now: Zoned) -> Result<()> {
    let today = now.date();
    let at = now.timestamp();
    match command {
        Cmd::Init { .. } => unreachable!("handled in main"),
        Cmd::Remote { cmd: RemoteCmd::Set { url, token, token_file } } => {
            let url = remote::normalize_url(&url)?;
            let token = read_token(token, token_file)?;
            remote::save(root, &remote::Remote { url: url.clone(), token })?;
            println!("remote: {url}");
            Ok(())
        }
        Cmd::Remote { cmd: RemoteCmd::Show } => {
            match remote::load(root)? {
                Some(r) => println!("{} (token saved)", r.url),
                None => println!("no remote configured; `mise remote set <url> --token ...`"),
            }
            Ok(())
        }
        Cmd::Sync { server, token } => {
            let saved = remote::load(root)?;
            let url = match (server, &saved) {
                (Some(url), _) => remote::normalize_url(&url)?,
                (None, Some(r)) => r.url.clone(),
                (None, None) => bail!("no server: pass --server or `mise remote set`"),
            };
            let token = match (token, saved) {
                (Some(t), _) => t,
                (None, Some(r)) => r.token,
                (None, None) => bail!("no token: pass --token or `mise remote set`"),
            };
            let outcome = remote::sync(store, &url, &token)?;
            if !outcome.docs_updated.is_empty() || outcome.log_added > 0 {
                store.export(&format!("sync: {}", remote::describe(&outcome)))?;
            }
            println!("sync: {}", remote::describe(&outcome));
            Ok(())
        }
        Cmd::Export => {
            store.export("cli: export")?;
            println!("exported to {}", store.export_dir().display());
            Ok(())
        }
        Cmd::Queue { cmd: None } => show_queue(store, now.datetime()),
        Cmd::Queue { cmd: Some(QueueCmd::Add { title, recipe, reason, id, someday }) } => {
            let title = must_trim(&title, "title")?;
            let recipe = recipe.map(|r| slug(&r)).transpose()?;
            if let Some(r) = &recipe
                && !store.exists(&DocId::Recipe(r.clone()))?
            {
                bail!("no recipe {r}; `mise recipe add {r}` first, or omit --recipe");
            }
            let id = match id {
                Some(id) => slug(&id)?,
                None => slug(&slugify(&title))?,
            };
            let doc_id = if someday { DocId::Someday } else { DocId::Queue };
            let msg = format!("cli: queue add {id}");
            store.modify::<QueueDoc>(&doc_id, &msg, at, |q| {
                q.entries.insert(
                    id.to_string(),
                    QueueEntryDoc {
                        dishes: vec![DishRefDoc {
                            recipe: recipe.as_ref().map(|r| r.to_string()),
                            title,
                        }],
                        reason: reason.and_then(|r| opt_trim(&r)),
                        added: today.to_string(),
                    },
                );
            })?;
            store.export(&msg)?;
            println!("queued {id}");
            Ok(())
        }
        Cmd::Queue { cmd: Some(QueueCmd::Remove { id, someday }) } => {
            let doc_id = if someday { DocId::Someday } else { DocId::Queue };
            let msg = format!("cli: queue remove {id}");
            // Presence is captured inside the closure: `modify` returns the
            // post-mutation doc, where the entry is absent either way.
            let mut found = false;
            store.modify::<QueueDoc>(&doc_id, &msg, at, |q| {
                found = q.entries.remove(&id).is_some();
            })?;
            if !found {
                bail!("no such queue entry {id}");
            }
            store.export(&msg)?;
            println!("removed {id}");
            Ok(())
        }
        Cmd::Chat { message, page, model, photos } => {
            run_chat(store, message, page, model, photos, now)
        }
        Cmd::Recipe { cmd } => run_recipe(store, cmd, at),
        Cmd::Equipment { cmd } => run_equipment(store, cmd, at),
        Cmd::Pantry { cmd } => run_pantry(store, cmd, today, at),
        Cmd::Fridge { cmd } => run_fridge(store, cmd, today, at),
        Cmd::Log { cmd } => run_log(store, cmd, today, at),
        Cmd::Location { cmd } => match cmd {
            LocationCmd::Add { name, headcount } => {
                let name = slug(&name)?;
                let msg = format!("cli: location add {name}");
                store.add_location(&name, headcount, &msg, at)?;
                store.export(&msg)?;
                println!("added location {name}");
                Ok(())
            }
            LocationCmd::Use { name } => {
                let name = slug(&name)?;
                let msg = format!("cli: location use {name}");
                let state = store.modify::<StateDoc>(&DocId::State, &msg, at, |s| {
                    if s.locations.contains_key(name.as_str()) {
                        s.active_location = name.to_string();
                    }
                })?;
                if state.active_location != name.as_str() {
                    bail!("no location {name}; `mise location add {name}` first");
                }
                store.export(&msg)?;
                println!("active location: {name}");
                Ok(())
            }
        },
    }
}

fn run_chat(
    store: &mut Store,
    message: String,
    page: Option<String>,
    model: Option<String>,
    photos: Vec<std::path::PathBuf>,
    now: Zoned,
) -> Result<()> {
    use std::io::Write as _;

    use base64::Engine as _;
    use mise_assistant::client::AnthropicClient;
    use mise_assistant::context::provenance;
    use mise_assistant::exchange::{ExchangeEvent, run_exchange};
    use mise_store::threads::ThreadId;

    let message = must_trim(&message, "message")?;
    let photos = photos
        .iter()
        .map(|path| {
            let media_type = match path.extension().and_then(|e| e.to_str()) {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                other => bail!("can't tell the image type from extension {other:?}"),
            };
            let bytes = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
            Ok(mise_assistant::recon::Photo {
                media_type: media_type.to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let thread = match &page {
        Some(p) => ThreadId::parse(p).map_err(|e| anyhow::Error::msg(e.to_string()))?,
        None => ThreadId::Planning,
    };
    if let ThreadId::Page(id) = &thread
        && !store.exists(id)?
    {
        bail!("no page {id} to talk about; create it first");
    }
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY not set (a git-ignored .env works for dev)")?;
    let mut client = AnthropicClient::new(api_key);
    if let Some(m) = model {
        client = client.with_model(m);
    }

    // `now` anchors the exchange; the reply's later stamp comes from a
    // fresh reading so the transcript sorts in conversation order.
    let mut first = Some(now);
    let mut clock = move || first.take().unwrap_or_else(jiff::Zoned::now);
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(run_exchange(
        &mut client,
        &mut mise_assistant::fetch::HttpFetch::new(),
        store,
        &thread,
        &message,
        &photos,
        &mut clock,
        &mut |event| match event {
            ExchangeEvent::TextDelta(d) => {
                print!("{d}");
                let _ = std::io::stdout().flush();
            }
            ExchangeEvent::ToolCall { name } => eprintln!("  ⚙ {name}"),
            ExchangeEvent::Proposal(p) => {
                for line in &p.lines {
                    eprintln!("  ⇒ {}: {} ({})", line.item, line.presence, line.reason);
                }
                eprintln!("  (proposal only — apply with `mise pantry`, nothing was changed)");
            }
        },
    ));
    println!();

    let mut summary: String = message.chars().take(60).collect();
    if summary.len() < message.len() {
        summary.push('…');
    }
    if let Err(e) = result {
        // Earlier tool rounds may have mutated the store; the readable
        // backup must not sit behind it just because the exchange died.
        let _ = store.export(&format!("{} (failed): {summary}", provenance(&thread)));
        return Err(e.into());
    }
    store.export(&format!("{}: {summary}", provenance(&thread)))?;
    Ok(())
}

fn run_recipe(store: &mut Store, cmd: RecipeCmd, at: jiff::Timestamp) -> Result<()> {
    match cmd {
        RecipeCmd::Add {
            slug: s,
            title,
            servings,
            effort,
            tags,
            equipment,
            lead_minutes,
            lead_step,
            body,
        } => {
            let s = slug(&s)?;
            if servings == 0 {
                bail!("servings must be at least 1");
            }
            let equipment = equipment.iter().map(|e| slug(e)).collect::<Result<Vec<_>>>()?;
            let body_text = body.map(|p| read_body(&p)).transpose()?.unwrap_or_default();
            let doc = RecipeDoc {
                schema_version: mise_store::pages::SCHEMA_VERSION,
                title: must_trim(&title, "title")?,
                servings,
                effort: effort.to_string(),
                lead: lead_minutes.map(|minutes| LeadTimeDoc {
                    minutes,
                    act_now_step: lead_step.unwrap_or_default().trim().to_string(),
                }),
                tags: parse_tags(&tags)?,
                equipment,
                ingredients: vec![],
                source: None,
                status: mise_core::types::RecipeStatus::Active,
                body: body_text.as_str().into(),
            };
            let msg = format!("cli: recipe add {s}");
            store.create_doc(&DocId::Recipe(s.clone()), &doc, &msg, at)?;
            store.export(&msg)?;
            println!("added recipe {s}");
            Ok(())
        }
        RecipeCmd::Ingredient { slug: s, text, link } => {
            let s = slug(&s)?;
            let link = link.map(|l| slug(&l)).transpose()?;
            let text = must_trim(&text, "ingredient text")?;
            let msg = format!("cli: recipe {s}: ingredient");
            store.modify::<RecipeDoc>(&DocId::Recipe(s.clone()), &msg, at, |r| {
                r.ingredients.push(IngredientDoc {
                    text,
                    pantry: link.clone(),
                });
            })?;
            store.export(&msg)?;
            println!("added ingredient to {s}");
            Ok(())
        }
        RecipeCmd::Body { slug: s, file } => {
            let s = slug(&s)?;
            let body = read_body(&file)?;
            let msg = format!("cli: recipe {s}: body");
            // Char-safe diff splice; autosurgeon's Text::update is
            // byte-indexed and breaks on non-ASCII (see Store::update_body).
            store.update_body(&DocId::Recipe(s.clone()), &body, &msg, at)?;
            store.export(&msg)?;
            println!("updated body of {s}");
            Ok(())
        }
    }
}

fn run_equipment(store: &mut Store, cmd: EquipmentCmd, at: jiff::Timestamp) -> Result<()> {
    match cmd {
        EquipmentCmd::Add { item, note, location } => {
            let item = slug(&item)?;
            let loc = resolve_location(store, location)?;
            let msg = format!("cli: equipment {loc}: add {item}");
            store.modify::<mise_store::pages::EquipmentDoc>(
                &DocId::Equipment(loc.clone()),
                &msg,
                at,
                |e| {
                    e.items.insert(
                        item.to_string(),
                        note.as_deref().map(|n| n.trim().to_string()).unwrap_or_default(),
                    );
                },
            )?;
            store.export(&msg)?;
            println!("equipment {loc}: added {item}");
            Ok(())
        }
        EquipmentCmd::Remove { item, location } => {
            let loc = resolve_location(store, location)?;
            let msg = format!("cli: equipment {loc}: remove {item}");
            let mut found = false;
            store.modify::<mise_store::pages::EquipmentDoc>(
                &DocId::Equipment(loc.clone()),
                &msg,
                at,
                |e| {
                    found = e.items.remove(&item).is_some();
                },
            )?;
            if !found {
                bail!("no such equipment {item} at {loc}");
            }
            store.export(&msg)?;
            println!("equipment {loc}: removed {item}");
            Ok(())
        }
    }
}

fn run_pantry(store: &mut Store, cmd: PantryCmd, today: Date, at: jiff::Timestamp) -> Result<()> {
    match cmd {
        PantryCmd::Set { item, name, presence, tier, bought, note, location } => {
            let item = slug(&item)?;
            let loc = resolve_location(store, location)?;
            if let Some(p) = &presence {
                p.parse::<mise_core::types::Presence>().map_err(anyhow::Error::msg)?;
            }
            let tier = tier.map(|t| slug(&t)).transpose()?;
            let bought = bought.map(|b| parse_date(&b, today)).transpose()?;
            let msg = format!("cli: pantry {loc}: set {item}");
            store.modify::<mise_store::pages::PantryDoc>(&DocId::Pantry(loc.clone()), &msg, at, |p| {
                let entry = p.items.entry(item.to_string()).or_insert_with(|| PantryItemDoc {
                    name: item.as_str().replace('-', " "),
                    presence: "have".to_string(),
                    bought: None,
                    tier: None,
                    note: None,
                });
                if let Some(n) = name.as_ref().and_then(|n| opt_trim(n)) {
                    entry.name = n;
                }
                if let Some(p) = &presence {
                    entry.presence = p.clone();
                }
                if let Some(t) = &tier {
                    entry.tier = Some(t.to_string());
                }
                if let Some(b) = &bought {
                    entry.bought = Some(b.to_string());
                }
                if let Some(n) = &note {
                    entry.note = opt_trim(n);
                }
            })?;
            store.export(&msg)?;
            println!("pantry {loc}: {item} updated");
            Ok(())
        }
        PantryCmd::Remove { item, location } => {
            let loc = resolve_location(store, location)?;
            let msg = format!("cli: pantry {loc}: remove {item}");
            let mut found = false;
            store.modify::<mise_store::pages::PantryDoc>(&DocId::Pantry(loc.clone()), &msg, at, |p| {
                found = p.items.remove(&item).is_some();
            })?;
            if !found {
                bail!("no such pantry item {item} at {loc}");
            }
            store.export(&msg)?;
            println!("pantry {loc}: {item} removed");
            Ok(())
        }
    }
}

fn run_fridge(store: &mut Store, cmd: FridgeCmd, today: Date, at: jiff::Timestamp) -> Result<()> {
    match cmd {
        FridgeCmd::Add { dish, servings, date, freezer, id, location } => {
            let loc = resolve_location(store, location)?;
            let dish = must_trim(&dish, "dish")?;
            let date = date.map(|d| parse_date(&d, today)).transpose()?.unwrap_or(today);
            let msg = format!("cli: fridge {loc}: add {dish}");
            store.modify::<FridgeDoc>(&DocId::Fridge(loc.clone()), &msg, at, |f| {
                let portions = match &freezer {
                    Some(name) => f.freezers.entry(name.trim().to_string()).or_default(),
                    None => &mut f.fridge,
                };
                let id = id.clone().unwrap_or_else(|| {
                    (1..)
                        .map(|n| format!("p{n}"))
                        .find(|c| !portions.contains_key(c))
                        .expect("unbounded candidate ids")
                });
                portions.insert(
                    id,
                    PortionDoc { dish: dish.clone(), servings, date: date.to_string() },
                );
            })?;
            store.export(&msg)?;
            println!("fridge {loc}: added {dish} ({servings} servings)");
            Ok(())
        }
        FridgeCmd::Remove { id, freezer, location } => {
            let loc = resolve_location(store, location)?;
            let msg = format!("cli: fridge {loc}: remove {id}");
            let mut found = false;
            store.modify::<FridgeDoc>(&DocId::Fridge(loc.clone()), &msg, at, |f| {
                match &freezer {
                    Some(name) => {
                        if let Some(portions) = f.freezers.get_mut(name.trim()) {
                            found = portions.remove(&id).is_some();
                            if portions.is_empty() {
                                f.freezers.remove(name.trim());
                            }
                        }
                    }
                    None => {
                        found = f.fridge.remove(&id).is_some();
                    }
                }
            })?;
            if !found {
                match &freezer {
                    Some(name) => bail!("no such portion {id} in freezer {}", name.trim()),
                    None => bail!("no such portion {id} in the fridge"),
                }
            }
            store.export(&msg)?;
            println!("fridge {loc}: removed {id}");
            Ok(())
        }
    }
}

fn run_log(store: &mut Store, cmd: LogCmd, today: Date, at: jiff::Timestamp) -> Result<()> {
    match cmd {
        LogCmd::Add { title, recipe, kind, servings, verdict, date, tags, location } => {
            let loc = resolve_location(store, location)?;
            let kind: CookKind = kind.parse().map_err(anyhow::Error::msg)?;
            let recipe = recipe.map(|r| slug(&r)).transpose()?;
            let date = date.map(|d| parse_date(&d, today)).transpose()?.unwrap_or(today);
            let mut entry_tags = BTreeMap::new();
            let mut servings_default = None;
            if let Some(r) = &recipe {
                let doc: RecipeDoc = store.get(&DocId::Recipe(r.clone()))?;
                entry_tags = doc.tags.clone();
                servings_default = Some(doc.servings);
            }
            entry_tags.extend(parse_tags(&tags)?);
            let servings = servings
                .or(servings_default)
                .context("no --servings and no recipe to take a default from")?;
            let entry = LogEntry {
                date,
                kind,
                recipe,
                title: must_trim(&title, "title")?,
                location: loc.to_string(),
                servings,
                verdict: verdict.trim().to_string(),
                tags: entry_tags,
            };
            store.append_log(&entry, &format!("cli: log {}", entry.title), at)?;
            store.export(&format!("cli: log {}", entry.title))?;
            println!("logged: {} on {} at {}", entry.title, entry.date, loc);
            Ok(())
        }
        LogCmd::Rotation { window } => {
            let log = store.log_entries()?;
            let rec = recency(&log, today, window);
            if rec.is_empty() {
                println!("no tagged cooks in the log yet");
                return Ok(());
            }
            println!("rotation recency (window: {window} days)");
            for ((axis, value), r) in &rec {
                println!(
                    "  {axis}={value}: last {} ({} days ago), {} in window",
                    r.last_made,
                    r.days_since(today),
                    r.in_window,
                );
            }
            Ok(())
        }
    }
}

// ------------------------------------------------------------ queue view --

/// One structured view, rendered by `mise-assistant::views` — the same
/// rendering the assistant tool and the JSON API use. The CLI's only
/// divergence is the empty-queue hint, which can name a command.
fn show_queue(store: &mut Store, now: DateTime) -> Result<()> {
    let view = views::queue_view(store, now)?;
    print!(
        "{}",
        views::render_queue_status(&view, Some("(empty — `mise queue add <title>`)"))
    );
    Ok(())
}

// --------------------------------------------------------------- helpers --

fn slug(s: &str) -> Result<Slug> {
    Slug::new(s.trim()).map_err(|e| anyhow::Error::msg(e.to_string()))
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn must_trim(s: &str, what: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() {
        bail!("{what} must not be empty");
    }
    Ok(t.to_string())
}

fn opt_trim(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn parse_tags(tags: &[String]) -> Result<BTreeMap<String, String>> {
    tags.iter()
        .map(|t| {
            let (k, v) = t.split_once('=').with_context(|| format!("tag {t:?} is not k=v"))?;
            Ok((must_trim(k, "tag axis")?, must_trim(v, "tag value")?))
        })
        .collect()
}

fn parse_date(s: &str, today: Date) -> Result<Date> {
    if s.trim() == "today" {
        return Ok(today);
    }
    s.trim().parse().with_context(|| format!("bad date {s:?} (want YYYY-MM-DD)"))
}

fn resolve_location(store: &Store, requested: Option<String>) -> Result<Slug> {
    match requested {
        Some(l) => {
            let l = slug(&l)?;
            let state: StateDoc = store.get(&DocId::State)?;
            if !state.locations.contains_key(l.as_str()) {
                bail!("no location {l}; `mise location add {l}` first");
            }
            Ok(l)
        }
        None => {
            let state: StateDoc = store.get(&DocId::State)?;
            slug(&state.active_location)
        }
    }
}

fn read_body(path: &std::path::Path) -> Result<String> {
    let raw = if path.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    };
    Ok(raw.trim().to_string())
}
