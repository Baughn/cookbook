//! Manual prompt-quality evals against the real API. See README.md.
//!
//! Each scenario seeds a corpus, runs real exchanges, and prints a report:
//! mechanical checks scored automatically, transcript attached for human
//! judgment. Exit code is the number of failed mechanical checks, so a
//! shell loop can still notice regressions — but this is not CI and never
//! will be.

use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use jiff::Zoned;
use mise_assistant::client::AnthropicClient;
use mise_assistant::exchange::{ExchangeEvent, run_exchange};
use mise_core::types::{CookKind, LogEntry, Slug};
use mise_store::pages::{LeadTimeDoc, PantryItemDoc, RecipeDoc};
use mise_store::threads::ThreadId;
use mise_store::{DocId, Store};

struct Report {
    checks: Vec<(String, bool)>,
}

impl Report {
    fn check(&mut self, name: &str, pass: bool) {
        println!("  [{}] {name}", if pass { "PASS" } else { "FAIL" });
        self.checks.push((name.to_string(), pass));
    }
}

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

/// A lived-in corpus: recipes with tags and lead time, a stocked pantry,
/// a log leaning hard on curry, a steering agenda.
fn seed(root: &std::path::Path) -> Result<Store> {
    let mut store = Store::create(root, &slug("home"), 2, Zoned::now().timestamp())?;
    let p = "eval: seed";
    type Seed = (&'static str, &'static str, BTreeMap<String, String>, Option<LeadTimeDoc>);
    let recipes: [Seed; 4] = [
        (
            "duck-curry",
            "Duck curry",
            BTreeMap::from([
                ("cuisine".into(), "thai".into()),
                ("protein".into(), "duck".into()),
                ("format".into(), "curry".into()),
            ]),
            None,
        ),
        (
            "mapo-tofu",
            "Mapo tofu",
            BTreeMap::from([
                ("cuisine".into(), "sichuan".into()),
                ("protein".into(), "pork".into()),
                ("format".into(), "braise".into()),
            ]),
            Some(LeadTimeDoc { minutes: 60, act_now_step: "defrost the pork".into() }),
        ),
        (
            "dal-tadka",
            "Dal tadka",
            BTreeMap::from([
                ("cuisine".into(), "indian".into()),
                ("protein".into(), "legume".into()),
                ("format".into(), "stew".into()),
            ]),
            None,
        ),
        (
            "focaccia",
            "Focaccia",
            BTreeMap::from([("format".into(), "bake".into())]),
            Some(LeadTimeDoc { minutes: 180, act_now_step: "mix the dough".into() }),
        ),
    ];
    for (s, title, tags, lead) in recipes {
        store.create_doc(
            &DocId::Recipe(slug(s)),
            &RecipeDoc {
                schema_version: 1,
                title: title.into(),
                servings: 4,
                effort: "weekday".into(),
                lead,
                tags,
                equipment: vec![],
                ingredients: vec![],
                source: None,
                status: "active".into(),
                body: "Cook it well.".into(),
            },
            p,
            Zoned::now().timestamp(),
        )?;
    }
    store.modify::<mise_store::pages::PantryDoc>(&DocId::Pantry(slug("home")), p, Zoned::now().timestamp(), |d| {
        for (item, presence) in
            [("eggs", "have"), ("red-lentils", "have"), ("coconut-milk", "low"), ("flour", "have")]
        {
            d.items.insert(
                item.into(),
                PantryItemDoc {
                    name: item.replace('-', " "),
                    presence: presence.into(),
                    bought: None,
                    tier: Some("shop".into()),
                    note: None,
                },
            );
        }
    })?;
    store.modify::<mise_store::pages::SteeringDoc>(&DocId::Steering, p, Zoned::now().timestamp(), |d| {
        d.entries.insert("skill".into(), "yeast baking beyond sourdough".into());
    })?;
    let today = Zoned::now().date();
    for (days_ago, recipe, title) in
        [(2, "duck-curry", "Duck curry"), (5, "duck-curry", "Duck curry"), (7, "dal-tadka", "Dal tadka")]
    {
        store.append_log(&LogEntry {
            date: today.saturating_sub(jiff::Span::new().days(days_ago)),
            kind: CookKind::Meal,
            recipe: Some(slug(recipe)),
            title: title.into(),
            location: "home".into(),
            servings: 4,
            verdict: "fine".into(),
            tags: BTreeMap::new(),
        }, "eval: seed log", Zoned::now().timestamp())?;
    }
    Ok(store)
}

async fn chat(store: &mut Store, thread: &ThreadId, message: &str) -> Result<Vec<String>> {
    let (tools, _) = chat_with(store, thread, message, mise_assistant::fetch::HttpFetch::new()).await?;
    Ok(tools)
}

/// Real model, chosen network: scenarios script the fetch so a fixture
/// page stands in for the live web. Returns (tools used, final reply).
async fn chat_with<F: mise_assistant::fetch::Fetch>(
    store: &mut Store,
    thread: &ThreadId,
    message: &str,
    fetcher: F,
) -> Result<(Vec<String>, String)> {
    let (tools, reply, _) = chat_full(store, thread, message, fetcher, None).await?;
    Ok((tools, reply))
}

/// The full-fat variant: photo attached, proposals captured.
async fn chat_full<F: mise_assistant::fetch::Fetch>(
    store: &mut Store,
    thread: &ThreadId,
    message: &str,
    mut fetcher: F,
    photo: Option<&mise_assistant::recon::Photo>,
) -> Result<(Vec<String>, String, Vec<mise_assistant::recon::Proposal>)> {
    println!("\n>>> {message}\n");
    let mut client = AnthropicClient::new(
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?,
    );
    let mut clock = Zoned::now;
    let mut proposals = Vec::new();
    let exchange = run_exchange(&mut client, &mut fetcher, store, thread, message, photo, &mut clock, &mut |e| {
        match e {
            ExchangeEvent::TextDelta(d) => print!("{d}"),
            ExchangeEvent::ToolCall { name } => println!("  ⚙ {name}"),
            ExchangeEvent::Proposal(p) => proposals.push(p.clone()),
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("\n");
    Ok((exchange.tools_used, exchange.reply, proposals))
}

async fn plan_week(report: &mut Report) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut store = seed(&dir.path().join("corpus"))?;
    let tools = chat(
        &mut store,
        &ThreadId::Planning,
        "Plan the next three or four days of dinners.",
    )
    .await?;

    report.check(
        "looked at the corpus before proposing (queue_status / read_page / search)",
        tools.iter().any(|t| ["queue_status", "read_page", "search", "list_pages"].contains(&t.as_str())),
    );
    let queue: mise_store::pages::QueueDoc = store.get(&DocId::Queue)?;
    report.check("queue gained at least two entries", queue.entries.len() >= 2);
    report.check(
        "every queued entry carries a reason",
        !queue.entries.is_empty() && queue.entries.values().all(|e| e.reason.is_some()),
    );
    report.check(
        "did not queue duck curry again (rotation: twice this week already)",
        !queue.entries.values().any(|e| {
            e.dishes.iter().any(|d| d.recipe.as_deref() == Some("duck-curry"))
        }),
    );
    Ok(())
}

async fn pantry_in_passing(report: &mut Report) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut store = seed(&dir.path().join("corpus"))?;
    let tools = chat(
        &mut store,
        &ThreadId::Planning,
        "By the way — we're out of eggs, and I picked up duck legs at the butcher.",
    )
    .await?;

    report.check("touched the pantry", tools.iter().any(|t| t == "pantry_set"));
    let pantry: mise_store::pages::PantryDoc =
        store.get(&DocId::Pantry(slug("home")))?;
    report.check(
        "eggs are now out",
        pantry.items.get("eggs").is_some_and(|i| i.presence == "out"),
    );
    report.check(
        "duck legs recorded as present",
        pantry.items.values().any(|i| i.name.contains("duck") && i.presence == "have"),
    );
    Ok(())
}

async fn debrief(report: &mut Report) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut store = seed(&dir.path().join("corpus"))?;
    let before: RecipeDoc = store.get(&DocId::Recipe(slug("mapo-tofu")))?;
    let thread = ThreadId::Page(DocId::Recipe(slug("mapo-tofu")));
    let tools = chat(
        &mut store,
        &thread,
        "Made this tonight — doubled the doubanjiang and it was way better. \
         Got six servings; two are in the fridge as leftovers.",
    )
    .await?;

    report.check("logged the cook", tools.iter().any(|t| t == "log_append"));
    let log = store.log_entries()?;
    report.check(
        "log entry references the recipe",
        log.iter().any(|e| e.recipe.as_ref().is_some_and(|r| r.as_str() == "mapo-tofu")),
    );
    let after: RecipeDoc = store.get(&DocId::Recipe(slug("mapo-tofu")))?;
    report.check(
        "folded the lesson into the recipe page",
        after.body.as_str() != before.body.as_str()
            || after.ingredients != before.ingredients
            || after.title != before.title,
    );
    let fridge: mise_store::pages::FridgeDoc = store.get(&DocId::Fridge(slug("home")))?;
    report.check(
        "leftovers landed in the fridge",
        fridge.fridge.values().any(|p| p.servings == 2),
    );
    Ok(())
}

/// A messy real-world-shaped page: no JSON-LD, the recipe buried under a
/// life story. The extraction strips the chrome; the *model* must strip
/// the narration and draft in house style — that's what's being judged.
async fn draft_from_url(report: &mut Report) -> Result<()> {
    struct FixtureFetch;
    impl mise_assistant::fetch::Fetch for FixtureFetch {
        async fn fetch(&mut self, _url: &str) -> std::result::Result<String, String> {
            Ok(include_str!("../fixtures/tonkatsu.html").to_string())
        }
    }

    let dir = tempfile::tempdir()?;
    let mut store = seed(&dir.path().join("corpus"))?;
    let seeded: Vec<String> =
        store.list("recipe")?.iter().map(std::string::ToString::to_string).collect();
    let url = "https://grandmas-kitchen-stories.example/tonkatsu";
    let (tools, _) = chat_with(
        &mut store,
        &ThreadId::Drafting,
        &format!("Ran into this and want to keep it: {url} — add it to the cookbook."),
        FixtureFetch,
    )
    .await?;

    report.check("fetched the page", tools.iter().any(|t| t == "fetch_url"));
    report.check("created a recipe", tools.iter().any(|t| t == "recipe_add"));
    let new_ids: Vec<DocId> = store
        .list("recipe")?
        .into_iter()
        .filter(|id| !seeded.contains(&id.to_string()))
        .collect();
    report.check("exactly one new recipe page", new_ids.len() == 1);
    if let Some(id) = new_ids.first() {
        let recipe: RecipeDoc = store.get(id)?;
        report.check("nobody asked to cook it: status draft", recipe.status == "draft");
        report.check("the source URL is on the page", recipe.source.as_deref() == Some(url));
        report.check("ingredients made it over", recipe.ingredients.len() >= 4);
        report.check("a real method body", recipe.body.as_str().len() > 100);
        let page = format!(
            "{} {} {}",
            recipe.title,
            recipe.ingredients.iter().map(|i| i.text.clone()).collect::<Vec<_>>().join(" "),
            recipe.body.as_str(),
        );
        report.check(
            "the life story stayed on the blog",
            !page.contains("Mrs. Tanaka") && !page.contains("rainy") && !page.contains("Osaka"),
        );
    }
    Ok(())
}

/// A page whose numbers are computed client-side: the fetch delivers the
/// shape of a recipe with none of its quantities. The right move is to
/// stop and ask — not to invent a baseline and bury the caveat in the
/// page.
async fn calculator_page(report: &mut Report) -> Result<()> {
    struct FixtureFetch;
    impl mise_assistant::fetch::Fetch for FixtureFetch {
        async fn fetch(&mut self, _url: &str) -> std::result::Result<String, String> {
            Ok(include_str!("../fixtures/pancake-calculator.html").to_string())
        }
    }

    let dir = tempfile::tempdir()?;
    let mut store = seed(&dir.path().join("corpus"))?;
    let seeded = store.list("recipe")?.len();
    let (tools, reply) = chat_with(
        &mut store,
        &ThreadId::Drafting,
        "Keep this one: https://absurdly-optimized.example/pancakes?tang=4&fluff=5 \
         — add it to the cookbook.",
        FixtureFetch,
    )
    .await?;

    report.check("fetched the page", tools.iter().any(|t| t == "fetch_url"));
    report.check(
        "did not invent a recipe from a quantity-less page",
        !tools.iter().any(|t| t == "recipe_add") && store.list("recipe")?.len() == seeded,
    );
    // Whether the ask reads as a question or an imperative ("read me the
    // list") is human judgment; mechanically we check the reply put the
    // gap in front of the user at all.
    let named_the_gap = ["amount", "quantit", "number", "slider"]
        .iter()
        .any(|w| reply.to_lowercase().contains(w));
    report.check("named the missing quantities in the reply", named_the_gap);
    Ok(())
}

/// Photo recon against real shelves. Photos live in `fixtures/private/`
/// (gitignored — shelf photos are personal data, like the corpus); the
/// scenario runs once per photo and skips politely when there are none.
/// The seeded pantry deliberately won't match anyone's real shelf: recon's
/// job is exactly that gap, and what the model saw is human judgment —
/// read the printed proposals against the photos.
async fn pantry_recon(report: &mut Report) -> Result<()> {
    use base64::Engine as _;

    struct NoFetch;
    impl mise_assistant::fetch::Fetch for NoFetch {
        async fn fetch(&mut self, url: &str) -> std::result::Result<String, String> {
            Err(format!("no network in this scenario (asked for {url})"))
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/private");
    let mut photos: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("jpg" | "jpeg" | "png" | "webp")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    photos.sort();
    if photos.is_empty() {
        println!("  (skipped: no photos in {} — drop shelf photos there to run this)", dir.display());
        return Ok(());
    }

    for path in photos {
        println!("  --- photo: {} ---", path.file_name().unwrap().to_string_lossy());
        let media_type = match path.extension().and_then(|e| e.to_str()) {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("png") => "image/png",
            _ => "image/webp",
        };
        let photo = mise_assistant::recon::Photo {
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(std::fs::read(&path)?),
        };

        let tmp = tempfile::tempdir()?;
        let mut store = seed(&tmp.path().join("corpus"))?;
        let pantry_id = DocId::Pantry(slug("home"));
        let before: mise_store::pages::PantryDoc = store.get(&pantry_id)?;
        let (tools, reply, proposals) = chat_full(
            &mut store,
            &ThreadId::Page(pantry_id.clone()),
            "Here's the pantry shelf — reconcile it against the page.",
            NoFetch,
            Some(&photo),
        )
        .await?;

        report.check(
            "proposed instead of editing (propose_pantry_diff, no pantry_set/remove)",
            tools.iter().any(|t| t == "propose_pantry_diff")
                && !tools.iter().any(|t| t == "pantry_set" || t == "pantry_remove"),
        );
        let after: mise_store::pages::PantryDoc = store.get(&pantry_id)?;
        report.check("the photo touched nothing", after == before);
        let lines: usize = proposals.iter().map(|p| p.lines.len()).sum();
        report.check("at least one proposal line", lines > 0);
        report.check("the reply summarizes the proposal", !reply.trim().is_empty());
        for p in &proposals {
            for l in &p.lines {
                println!("    ⇒ {}: {} ({})", l.item, l.presence, l.reason);
            }
        }
        println!("    ^ judge against the photo: misses, inventions, wrong presences.");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let _ = dotenvy::dotenv();
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let all = [
        "plan-week",
        "pantry-in-passing",
        "debrief",
        "draft-from-url",
        "calculator-page",
        "pantry-recon",
    ];
    let run_list: Vec<&str> = if requested.is_empty() {
        all.to_vec()
    } else {
        all.iter().copied().filter(|n| requested.iter().any(|r| r == n)).collect()
    };

    let mut report = Report { checks: vec![] };
    for name in run_list {
        println!("\n════ scenario: {name} ════");
        match name {
            "plan-week" => plan_week(&mut report).await?,
            "pantry-in-passing" => pantry_in_passing(&mut report).await?,
            "debrief" => debrief(&mut report).await?,
            "draft-from-url" => draft_from_url(&mut report).await?,
            "calculator-page" => calculator_page(&mut report).await?,
            "pantry-recon" => pantry_recon(&mut report).await?,
            _ => unreachable!(),
        }
    }

    let failed = report.checks.iter().filter(|(_, pass)| !pass).count();
    println!(
        "\n{} checks, {} failed. Mechanical checks only — read the transcripts.",
        report.checks.len(),
        failed,
    );
    Ok(ExitCode::from(u8::try_from(failed.min(255)).unwrap()))
}
