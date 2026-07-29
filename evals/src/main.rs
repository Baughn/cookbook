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
    let mut store = Store::create(root, &slug("home"), 2)?;
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
                retired: false,
                body: "Cook it well.".into(),
            },
            p,
        )?;
    }
    store.modify::<mise_store::pages::PantryDoc>(&DocId::Pantry(slug("home")), p, |d| {
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
    store.modify::<mise_store::pages::SteeringDoc>(&DocId::Steering, p, |d| {
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
        })?;
    }
    Ok(store)
}

async fn chat(store: &mut Store, thread: &ThreadId, message: &str) -> Result<Vec<String>> {
    println!("\n>>> {message}\n");
    let mut client = AnthropicClient::new(
        std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?,
    );
    let mut clock = || Zoned::now().datetime();
    let exchange = run_exchange(&mut client, store, thread, message, &mut clock, &mut |e| {
        match e {
            ExchangeEvent::TextDelta(d) => print!("{d}"),
            ExchangeEvent::ToolCall { name } => println!("  ⚙ {name}"),
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("\n");
    Ok(exchange.tools_used)
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

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let _ = dotenvy::dotenv();
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let all = ["plan-week", "pantry-in-passing", "debrief"];
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
