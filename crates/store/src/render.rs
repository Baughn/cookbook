//! The deterministic render layer: hydrated corpus state in, the complete
//! markdown export out, as a pure map of relative path → file contents.
//!
//! Same state → byte-identical files, always. All iteration is over ordered
//! maps or explicitly sorted rows; no floats, no wall clock, no randomness.
//! Values that could break table or line structure (newlines, pipes,
//! backslashes) are backslash-escaped; the export stays the readable truth
//! and the test-only parser can reverse it exactly.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mise_core::types::LogEntry;

use crate::pages::{
    CorpusState, EquipmentDoc, FridgeDoc, LocationDocs, PantryDoc, QueueDoc, RecipeDoc,
    ShoppingDoc, ShopsDoc, StateDoc, TechniqueDoc,
};

/// Escape a value for single-line contexts (table cells, frontmatter values).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '|' => out.push_str("\\|"),
            c => out.push(c),
        }
    }
    out
}

/// Escape a tag axis or value: `esc` plus the token separators.
pub fn tag_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '|' => out.push_str("\\|"),
            ';' => out.push_str("\\;"),
            '=' => out.push_str("\\="),
            c => out.push(c),
        }
    }
    out
}

fn opt(o: &Option<String>) -> String {
    o.as_deref().map(esc).unwrap_or_default()
}

fn tags_cell(tags: &BTreeMap<String, String>) -> String {
    tags.iter()
        .map(|(k, v)| format!("{}={}", tag_esc(k), tag_esc(v)))
        .collect::<Vec<_>>()
        .join(";")
}

fn frontmatter(out: &mut String, pairs: &[(&str, String)]) {
    out.push_str("---\n");
    for (k, v) in pairs {
        let _ = writeln!(out, "{k}: {v}");
    }
    out.push_str("---\n");
}

fn table(out: &mut String, headers: &[&str], rows: &[Vec<String>]) {
    let _ = writeln!(out, "| {} |", headers.join(" | "));
    let _ = writeln!(
        out,
        "|{}|",
        headers.iter().map(|_| " --- ").collect::<Vec<_>>().join("|")
    );
    for row in rows {
        let _ = writeln!(out, "| {} |", row.join(" | "));
    }
}

/// Render the whole corpus. The one public entry point.
pub fn render(c: &CorpusState) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    files.insert("state.md".to_string(), state_page(&c.state));
    files.insert("queue.md".to_string(), queue_page("Queue", &c.queue));
    files.insert("someday.md".to_string(), queue_page("Someday", &c.someday));
    files.insert("shopping.md".to_string(), shopping_page(&c.shopping));
    files.insert("steering.md".to_string(), kv_page("Steering", "note", &c.steering.schema_version, &c.steering.entries));
    files.insert("facts.md".to_string(), kv_page("Facts", "fact", &c.facts.schema_version, &c.facts.facts));
    for (name, docs) in &c.locations {
        location_pages(&mut files, name, docs);
    }
    for (slug, recipe) in &c.recipes {
        files.insert(format!("recipes/{slug}.md"), recipe_page(recipe));
    }
    for (slug, technique) in &c.techniques {
        files.insert(format!("techniques/{slug}.md"), technique_page(technique));
    }
    for (month, entries) in log_by_month(&c.log) {
        files.insert(format!("log/{month}.md"), log_page(&month, &entries));
    }
    for (key, messages) in &c.threads {
        files.insert(format!("threads/{key}.md"), thread_page(key, messages));
    }
    files
}

pub(crate) fn state_page(s: &StateDoc) -> String {
    let mut out = String::new();
    frontmatter(
        &mut out,
        &[
            ("schema-version", s.schema_version.to_string()),
            ("active-location", esc(&s.active_location)),
        ],
    );
    out.push_str("\n# State\n\n");
    let rows: Vec<Vec<String>> = s
        .locations
        .iter()
        .map(|(name, meta)| vec![esc(name), meta.headcount.to_string()])
        .collect();
    table(&mut out, &["location", "headcount"], &rows);
    out
}

pub(crate) fn queue_page(title: &str, doc: &QueueDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    let _ = write!(out, "\n# {title}\n\n");
    let mut entries: Vec<_> = doc.entries.iter().collect();
    entries.sort_by_key(|(id, e)| (e.added.clone(), (*id).clone()));
    let mut rows = Vec::new();
    for (id, e) in entries {
        let reason = opt(&e.reason);
        let added = esc(&e.added);
        if e.dishes.is_empty() {
            rows.push(vec![esc(id), String::new(), String::new(), reason, added]);
        } else {
            for dish in &e.dishes {
                rows.push(vec![
                    esc(id),
                    opt(&dish.recipe),
                    esc(&dish.title),
                    reason.clone(),
                    added.clone(),
                ]);
            }
        }
    }
    table(&mut out, &["entry", "recipe", "dish", "reason", "added"], &rows);
    out
}

pub(crate) fn shopping_page(doc: &ShoppingDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    out.push_str("\n# Shopping list\n\n");
    let rows: Vec<Vec<String>> = doc
        .items
        .iter()
        .map(|(id, item)| {
            vec![
                esc(id),
                esc(&item.text),
                opt(&item.tier),
                (if item.done { "yes" } else { "no" }).to_string(),
            ]
        })
        .collect();
    table(&mut out, &["id", "item", "tier", "done"], &rows);
    out
}

pub(crate) fn kv_page(
    title: &str,
    value_header: &str,
    schema_version: &u32,
    entries: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", schema_version.to_string())]);
    let _ = write!(out, "\n# {title}\n\n");
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|(k, v)| vec![esc(k), esc(v)])
        .collect();
    table(&mut out, &["key", value_header], &rows);
    out
}

fn location_pages(files: &mut BTreeMap<String, String>, name: &str, docs: &LocationDocs) {
    files.insert(
        format!("locations/{name}/pantry.md"),
        pantry_page(name, &docs.pantry),
    );
    files.insert(
        format!("locations/{name}/equipment.md"),
        equipment_page(name, &docs.equipment),
    );
    files.insert(
        format!("locations/{name}/shops.md"),
        shops_page(name, &docs.shops),
    );
    files.insert(
        format!("locations/{name}/fridge.md"),
        fridge_page(name, &docs.fridge),
    );
}

pub(crate) fn pantry_page(name: &str, doc: &PantryDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    let _ = write!(out, "\n# Pantry — {name}\n\n");
    let rows: Vec<Vec<String>> = doc
        .items
        .iter()
        .map(|(slug, i)| {
            vec![
                esc(slug),
                esc(&i.name),
                esc(&i.presence.to_string()),
                opt(&i.bought.map(|d| d.to_string())),
                opt(&i.tier.as_ref().map(|s| s.to_string())),
                opt(&i.note),
            ]
        })
        .collect();
    table(
        &mut out,
        &["item", "name", "presence", "bought", "tier", "note"],
        &rows,
    );
    out
}

pub(crate) fn equipment_page(name: &str, doc: &EquipmentDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    let _ = write!(out, "\n# Equipment — {name}\n\n");
    let rows: Vec<Vec<String>> = doc
        .items
        .iter()
        .map(|(slug, note)| vec![esc(slug), esc(note)])
        .collect();
    table(&mut out, &["item", "note"], &rows);
    out
}

pub(crate) fn shops_page(name: &str, doc: &ShopsDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    let _ = write!(out, "\n# Shops — {name}\n\n");
    let rows: Vec<Vec<String>> = doc
        .tiers
        .iter()
        .map(|t| vec![esc(&t.id), esc(&t.name)])
        .collect();
    table(&mut out, &["id", "name"], &rows);
    out
}

pub(crate) fn fridge_page(name: &str, doc: &FridgeDoc) -> String {
    let mut out = String::new();
    frontmatter(&mut out, &[("schema-version", doc.schema_version.to_string())]);
    let _ = write!(out, "\n# Fridge — {name}\n\n");
    let portion_rows = |portions: &BTreeMap<String, crate::pages::PortionDoc>| {
        portions
            .iter()
            .map(|(id, p)| {
                vec![esc(id), esc(&p.dish), p.servings.to_string(), esc(&p.date)]
            })
            .collect::<Vec<_>>()
    };
    table(
        &mut out,
        &["id", "dish", "servings", "date"],
        &portion_rows(&doc.fridge),
    );
    for (freezer, portions) in &doc.freezers {
        let _ = write!(out, "\n## Freezer: {}\n\n", esc(freezer));
        table(
            &mut out,
            &["id", "dish", "servings", "date"],
            &portion_rows(portions),
        );
    }
    out
}

fn ingredient_line(i: &crate::pages::IngredientDoc) -> String {
    let mut text = esc(&i.text);
    if text.starts_with('[') {
        text.insert(0, '\\');
    }
    match &i.pantry {
        Some(link) => format!("- [{}] {}", esc(link.as_str()), text),
        None => format!("- {text}"),
    }
}

pub(crate) fn recipe_page(r: &RecipeDoc) -> String {
    let mut pairs = vec![
        ("schema-version", r.schema_version.to_string()),
        ("title", esc(&r.title)),
        ("servings", r.servings.to_string()),
        ("effort", esc(&r.effort.to_string())),
    ];
    if let Some(lead) = &r.lead {
        pairs.push(("lead-minutes", lead.minutes.to_string()));
        pairs.push(("lead-step", esc(&lead.act_now_step)));
    }
    if !r.tags.is_empty() {
        pairs.push(("tags", tags_cell(&r.tags)));
    }
    if !r.equipment.is_empty() {
        pairs.push(("equipment", r.equipment.iter().map(|e| esc(e.as_str())).collect::<Vec<_>>().join(",")));
    }
    if let Some(source) = &r.source {
        pairs.push(("source", esc(source)));
    }
    pairs.push(("status", r.status.to_string()));

    let mut out = String::new();
    frontmatter(&mut out, &pairs);
    let _ = write!(out, "\n# {}\n\n", esc(&r.title));
    out.push_str("## Ingredients\n");
    if !r.ingredients.is_empty() {
        out.push('\n');
        for i in &r.ingredients {
            let _ = writeln!(out, "{}", ingredient_line(i));
        }
    }
    out.push_str("\n## Method\n");
    let body = r.body.as_str();
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    out
}

pub(crate) fn technique_page(t: &TechniqueDoc) -> String {
    let mut pairs = vec![
        ("schema-version", t.schema_version.to_string()),
        ("title", esc(&t.title)),
    ];
    if !t.tags.is_empty() {
        pairs.push(("tags", tags_cell(&t.tags)));
    }
    let mut out = String::new();
    frontmatter(&mut out, &pairs);
    let _ = write!(out, "\n# {}\n", esc(&t.title));
    let body = t.body.as_str();
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    out
}

fn log_by_month(log: &[LogEntry]) -> BTreeMap<String, Vec<LogEntry>> {
    let mut months: BTreeMap<String, Vec<LogEntry>> = BTreeMap::new();
    for entry in log {
        let month = format!("{:04}-{:02}", entry.date.year(), entry.date.month());
        months.entry(month).or_default().push(entry.clone());
    }
    months
}

/// A thread transcript. Message content is blockquoted line by line — a
/// content line can never be mistaken for a `##` message heading, so the
/// transcript round-trips exactly. Content is normalized on append (LF,
/// trimmed, non-empty); thread keys and roles need no escaping by
/// construction.
fn thread_page(key: &str, messages: &[crate::threads::ThreadMessage]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Thread — {key}");
    for m in messages {
        let _ = write!(out, "\n## {} — {}\n\n", m.role, m.created);
        for line in m.content.lines() {
            if line.is_empty() {
                out.push_str(">\n");
            } else {
                let _ = writeln!(out, "> {line}");
            }
        }
    }
    out
}

fn log_page(month: &str, entries: &[LogEntry]) -> String {
    let mut out = String::new();
    let _ = write!(out, "# Log — {month}\n\n");
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.date.to_string(),
                e.kind.to_string(),
                e.recipe.as_ref().map(|s| esc(s.as_str())).unwrap_or_default(),
                esc(&e.title),
                esc(&e.location),
                e.servings.to_string(),
                esc(&e.verdict),
                tags_cell(&e.tags),
            ]
        })
        .collect();
    table(
        &mut out,
        &["date", "kind", "recipe", "title", "location", "servings", "verdict", "tags"],
        &rows,
    );
    out
}
