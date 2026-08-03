//! Test-only support: the export parser and corpus strategies.
//!
//! The parser reverses the render layer exactly — export → parse →
//! structural compare against store state is how export completeness is
//! verified. It lives in test code only and must never grow into an input
//! path.

#![allow(dead_code)]

use std::collections::BTreeMap;

use mise_core::types::{CookKind, EffortClass, LogEntry, Presence, RecipeStatus, Slug};
use mise_store::pages::{
    CorpusState, DishRefDoc, EquipmentDoc, FactsDoc, FridgeDoc, IngredientDoc, LeadTimeDoc,
    LocationDocs, LocationMeta, PantryDoc, PantryItemDoc, PortionDoc, QueueDoc, QueueEntryDoc,
    RecipeDoc, ShoppingDoc, ShoppingItemDoc, ShopsDoc, StateDoc, SteeringDoc, TechniqueDoc,
    TierDoc,
};
use mise_store::threads::{Role, ThreadId, ThreadMessage};
use proptest::collection::vec;
use proptest::prelude::*;

// ------------------------------------------------------------- unescaping --

pub fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('|') => out.push('|'),
            Some(';') => out.push(';'),
            Some('=') => out.push('='),
            Some('[') => out.push('['),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split on an unescaped separator character, returning still-escaped parts.
fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            parts.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            parts.last_mut().unwrap().push(c);
            escaped = true;
        } else if c == sep {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

fn opt_cell(raw: &str) -> Option<String> {
    (!raw.is_empty()).then(|| unesc(raw))
}

fn parse_tags(raw: &str) -> BTreeMap<String, String> {
    if raw.is_empty() {
        return BTreeMap::new();
    }
    split_unescaped(raw, ';')
        .into_iter()
        .map(|token| {
            let kv = split_unescaped(&token, '=');
            assert_eq!(kv.len(), 2, "bad tag token {token:?}");
            (unesc(&kv[0]), unesc(&kv[1]))
        })
        .collect()
}

// ----------------------------------------------------------------- tables --

/// Raw (still-escaped, trimmed) cells of every data row in `chunk`.
fn table_rows(chunk: &str) -> Vec<Vec<String>> {
    let mut lines = chunk.lines().filter(|l| l.starts_with('|'));
    let _header = lines.next().expect("table header");
    let _separator = lines.next().expect("table separator");
    lines.map(parse_row).collect()
}

fn parse_row(line: &str) -> Vec<String> {
    let mut segments = vec![String::new()];
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            segments.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            segments.last_mut().unwrap().push(c);
            escaped = true;
        } else if c == '|' {
            segments.push(String::new());
        } else {
            segments.last_mut().unwrap().push(c);
        }
    }
    assert!(segments.len() >= 2, "not a table row: {line:?}");
    segments[1..segments.len() - 1]
        .iter()
        .map(|s| s.trim().to_string())
        .collect()
}

// ------------------------------------------------------------ frontmatter --

fn split_frontmatter(content: &str) -> (BTreeMap<String, String>, &str) {
    let rest = content.strip_prefix("---\n").expect("frontmatter start");
    let end = rest.find("\n---\n").expect("frontmatter end");
    let fm = rest[..end]
        .lines()
        .map(|l| {
            let (k, v) = l.split_once(": ").unwrap_or_else(|| {
                (l.strip_suffix(':').expect("frontmatter line"), "")
            });
            (k.to_string(), v.to_string())
        })
        .collect();
    (fm, &rest[end + 5..])
}

fn fm_u32(fm: &BTreeMap<String, String>, key: &str) -> u32 {
    fm[key].parse().expect("integer frontmatter value")
}

// ---------------------------------------------------------- page parsers --

fn parse_state(content: &str) -> StateDoc {
    let (fm, rest) = split_frontmatter(content);
    let locations = table_rows(rest)
        .into_iter()
        .map(|row| {
            (unesc(&row[0]), LocationMeta { headcount: row[1].parse().unwrap() })
        })
        .collect();
    StateDoc {
        schema_version: fm_u32(&fm, "schema-version"),
        active_location: unesc(&fm["active-location"]),
        locations,
    }
}

fn parse_queue(content: &str) -> QueueDoc {
    let (fm, rest) = split_frontmatter(content);
    let mut entries: BTreeMap<String, QueueEntryDoc> = BTreeMap::new();
    for row in table_rows(rest) {
        let id = unesc(&row[0]);
        let entry = entries.entry(id).or_insert_with(|| QueueEntryDoc {
            dishes: vec![],
            reason: opt_cell(&row[3]),
            added: unesc(&row[4]),
        });
        if !(row[1].is_empty() && row[2].is_empty()) {
            entry.dishes.push(DishRefDoc {
                recipe: opt_cell(&row[1]),
                title: unesc(&row[2]),
            });
        }
    }
    QueueDoc { schema_version: fm_u32(&fm, "schema-version"), entries }
}

fn parse_shopping(content: &str) -> ShoppingDoc {
    let (fm, rest) = split_frontmatter(content);
    let items = table_rows(rest)
        .into_iter()
        .map(|row| {
            (
                unesc(&row[0]),
                ShoppingItemDoc {
                    text: unesc(&row[1]),
                    tier: opt_cell(&row[2]),
                    done: row[3] == "yes",
                },
            )
        })
        .collect();
    ShoppingDoc { schema_version: fm_u32(&fm, "schema-version"), items }
}

fn parse_kv(content: &str) -> (u32, BTreeMap<String, String>) {
    let (fm, rest) = split_frontmatter(content);
    let entries = table_rows(rest)
        .into_iter()
        .map(|row| (unesc(&row[0]), unesc(&row[1])))
        .collect();
    (fm_u32(&fm, "schema-version"), entries)
}

fn parse_pantry(content: &str) -> PantryDoc {
    let (fm, rest) = split_frontmatter(content);
    let items = table_rows(rest)
        .into_iter()
        .map(|row| {
            (
                unesc(&row[0]),
                PantryItemDoc {
                    name: unesc(&row[1]),
                    // Parsing our own byte-identical export back to the typed
                    // fields — the round-trip the completeness property checks.
                    presence: unesc(&row[2]).parse::<Presence>().expect("presence from our export"),
                    bought: opt_cell(&row[3]).map(|s| s.parse().expect("date from our export")),
                    tier: opt_cell(&row[4]).map(|s| Slug::new(s).expect("slug from our export")),
                    note: opt_cell(&row[5]),
                },
            )
        })
        .collect();
    PantryDoc { schema_version: fm_u32(&fm, "schema-version"), items }
}

fn parse_equipment(content: &str) -> EquipmentDoc {
    let (fm, rest) = split_frontmatter(content);
    let items = table_rows(rest)
        .into_iter()
        .map(|row| (unesc(&row[0]), unesc(&row[1])))
        .collect();
    EquipmentDoc { schema_version: fm_u32(&fm, "schema-version"), items }
}

fn parse_shops(content: &str) -> ShopsDoc {
    let (fm, rest) = split_frontmatter(content);
    let tiers = table_rows(rest)
        .into_iter()
        .map(|row| TierDoc { id: unesc(&row[0]), name: unesc(&row[1]) })
        .collect();
    ShopsDoc { schema_version: fm_u32(&fm, "schema-version"), tiers }
}

fn parse_portions(chunk: &str) -> BTreeMap<String, PortionDoc> {
    table_rows(chunk)
        .into_iter()
        .map(|row| {
            (
                unesc(&row[0]),
                PortionDoc {
                    dish: unesc(&row[1]),
                    servings: row[2].parse().unwrap(),
                    date: unesc(&row[3]),
                },
            )
        })
        .collect()
}

fn parse_fridge(content: &str) -> FridgeDoc {
    let (fm, rest) = split_frontmatter(content);
    let mut chunks = rest.split("\n## Freezer: ");
    let fridge = parse_portions(chunks.next().expect("fridge section"));
    let freezers = chunks
        .map(|chunk| {
            let (name, rows) = chunk.split_once('\n').expect("freezer heading");
            (unesc(name), parse_portions(rows))
        })
        .collect();
    FridgeDoc { schema_version: fm_u32(&fm, "schema-version"), fridge, freezers }
}

fn parse_ingredient(line: &str) -> IngredientDoc {
    let rest = line.strip_prefix("- ").expect("ingredient line");
    if rest.starts_with('[') && !rest.starts_with("\\[") {
        let end = rest.find("] ").expect("ingredient link close");
        IngredientDoc {
            pantry: Some(Slug::new(unesc(&rest[1..end])).expect("pantry link is a slug")),
            text: unesc(&rest[end + 2..]),
        }
    } else {
        IngredientDoc { text: unesc(rest), pantry: None }
    }
}

fn parse_recipe(content: &str) -> RecipeDoc {
    let (fm, rest) = split_frontmatter(content);
    let lead = fm.get("lead-minutes").map(|m| LeadTimeDoc {
        minutes: m.parse().unwrap(),
        act_now_step: unesc(&fm["lead-step"]),
    });
    let tags = fm.get("tags").map(|t| parse_tags(t)).unwrap_or_default();
    let equipment = fm
        .get("equipment")
        .map(|e| {
            e.split(',')
                .map(|s| Slug::new(unesc(s)).expect("equipment link is a slug"))
                .collect()
        })
        .unwrap_or_default();

    let marker = "\n## Method\n";
    let method = rest.find(marker).expect("method section");
    let before = &rest[..method];
    let after = &rest[method + marker.len()..];
    let ingredients = before
        .lines()
        .filter(|l| l.starts_with("- "))
        .map(parse_ingredient)
        .collect();
    let body = after.strip_prefix('\n').unwrap_or(after);
    let body = body.strip_suffix('\n').unwrap_or(body);

    RecipeDoc {
        schema_version: fm_u32(&fm, "schema-version"),
        title: unesc(&fm["title"]),
        servings: fm_u32(&fm, "servings"),
        effort: unesc(&fm["effort"]).parse::<EffortClass>().expect("effort from our export"),
        lead,
        tags,
        equipment,
        ingredients,
        source: fm.get("source").map(|s| unesc(s)),
        status: fm["status"].parse().expect("status is in vocabulary"),
        body: body.into(),
    }
}

fn parse_technique(content: &str) -> TechniqueDoc {
    let (fm, rest) = split_frontmatter(content);
    // rest: "\n# Title\n" then optionally "\n" + body + "\n".
    let after_title = rest
        .strip_prefix('\n')
        .and_then(|r| r.split_once('\n'))
        .expect("technique title line")
        .1;
    let body = after_title.strip_prefix('\n').unwrap_or(after_title);
    let body = body.strip_suffix('\n').unwrap_or(body);
    TechniqueDoc {
        schema_version: fm_u32(&fm, "schema-version"),
        title: unesc(&fm["title"]),
        tags: fm.get("tags").map(|t| parse_tags(t)).unwrap_or_default(),
        body: body.into(),
    }
}

fn parse_log_page(content: &str) -> Vec<LogEntry> {
    table_rows(content)
        .into_iter()
        .map(|row| LogEntry {
            date: row[0].parse().unwrap(),
            kind: row[1].parse::<CookKind>().unwrap(),
            recipe: (!row[2].is_empty()).then(|| Slug::new(unesc(&row[2])).unwrap()),
            title: unesc(&row[3]),
            location: unesc(&row[4]),
            servings: row[5].parse().unwrap(),
            verdict: unesc(&row[6]),
            tags: parse_tags(&row[7]),
        })
        .collect()
}

fn parse_thread(path_key: &str, content: &str) -> Vec<ThreadMessage> {
    let (heading, rest) = content.split_once('\n').expect("thread heading line");
    let key = heading.strip_prefix("# Thread — ").expect("thread heading");
    assert_eq!(key, path_key, "thread heading matches its path");
    let thread = ThreadId::parse(key).expect("valid thread id");
    rest.split("\n## ")
        .skip(1)
        .map(|chunk| {
            let (head, body) = chunk.split_once('\n').expect("message heading");
            let (role, created) = head.split_once(" — ").expect("role — created");
            let lines: Vec<&str> = body
                .lines()
                .filter(|l| l.starts_with('>'))
                .map(|l| l.strip_prefix("> ").unwrap_or_else(|| &l[1..]))
                .collect();
            ThreadMessage {
                thread: thread.clone(),
                role: role.parse().expect("valid role"),
                content: lines.join("\n"),
                created: created.parse().expect("valid datetime"),
            }
        })
        .collect()
}

/// Reconstruct the full corpus from a rendered export. Panics on anything
/// malformed — this is a test oracle, not an input path.
pub fn parse_corpus(files: &BTreeMap<String, String>) -> CorpusState {
    let (steering_version, steering) = parse_kv(&files["steering.md"]);
    let (facts_version, facts) = parse_kv(&files["facts.md"]);

    let mut locations: BTreeMap<String, LocationDocs> = BTreeMap::new();
    for path in files.keys() {
        if let Some(rest) = path.strip_prefix("locations/") {
            let name = rest.split('/').next().unwrap().to_string();
            if !locations.contains_key(&name) {
                locations.insert(
                    name.clone(),
                    LocationDocs {
                        pantry: parse_pantry(&files[&format!("locations/{name}/pantry.md")]),
                        equipment: parse_equipment(
                            &files[&format!("locations/{name}/equipment.md")],
                        ),
                        shops: parse_shops(&files[&format!("locations/{name}/shops.md")]),
                        fridge: parse_fridge(&files[&format!("locations/{name}/fridge.md")]),
                    },
                );
            }
        }
    }

    let mut recipes = BTreeMap::new();
    let mut techniques = BTreeMap::new();
    let mut log = Vec::new();
    let mut threads = BTreeMap::new();
    for (path, content) in files {
        if let Some(slug) = path.strip_prefix("recipes/").and_then(|p| p.strip_suffix(".md")) {
            recipes.insert(slug.to_string(), parse_recipe(content));
        } else if let Some(slug) =
            path.strip_prefix("techniques/").and_then(|p| p.strip_suffix(".md"))
        {
            techniques.insert(slug.to_string(), parse_technique(content));
        } else if path.starts_with("log/") {
            // BTreeMap iteration order is path order: months ascend.
            log.extend(parse_log_page(content));
        } else if let Some(key) =
            path.strip_prefix("threads/").and_then(|p| p.strip_suffix(".md"))
        {
            threads.insert(key.to_string(), parse_thread(key, content));
        }
    }

    CorpusState {
        state: parse_state(&files["state.md"]),
        queue: parse_queue(&files["queue.md"]),
        someday: parse_queue(&files["someday.md"]),
        shopping: parse_shopping(&files["shopping.md"]),
        steering: SteeringDoc { schema_version: steering_version, entries: steering },
        facts: FactsDoc { schema_version: facts_version, facts },
        locations,
        recipes,
        techniques,
        log,
        threads,
    }
}

// ------------------------------------------------------------- strategies --

/// Printable text with escape-worthy characters well represented, trimmed
/// (store invariant), possibly empty.
pub fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        proptest::string::string_regex("[ -~]{0,20}").unwrap(),
        proptest::string::string_regex("[a-zæøå☃|;=\\\\\\[\\]#\\-]{1,12}").unwrap(),
    ]
    .prop_map(|s| s.trim().to_string())
}

/// Non-empty trimmed text.
pub fn text1() -> impl Strategy<Value = String> {
    text().prop_map(|s| if s.is_empty() { "x".to_string() } else { s })
}

/// `None` or non-empty text (store invariant: no `Some("")`).
pub fn opt_text() -> impl Strategy<Value = Option<String>> {
    text().prop_map(|s| (!s.is_empty()).then_some(s))
}

/// Multi-line prose body, trimmed (store invariant).
pub fn body() -> impl Strategy<Value = String> {
    vec(text(), 0..4).prop_map(|lines| lines.join("\n").trim().to_string())
}

pub fn slug_str(prefix: &'static str, n: usize) -> impl Strategy<Value = String> {
    (0..n).prop_map(move |i| format!("{prefix}{i}"))
}

pub fn date_str() -> impl Strategy<Value = String> {
    (2020i16..2030, 1i8..=12, 1i8..=28)
        .prop_map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
}

fn arb_pantry_doc() -> impl Strategy<Value = PantryDoc> {
    vec(
        (
            slug_str("item", 8),
            (
                text1(),
                prop_oneof![Just("have"), Just("low"), Just("out")].prop_map(str::to_string),
                proptest::option::of(date_str()),
                proptest::option::of(slug_str("tier", 4)),
                opt_text(),
            ),
        ),
        0..6,
    )
    .prop_map(|items| PantryDoc {
        schema_version: 1,
        items: items
            .into_iter()
            .map(|(slug, (name, presence, bought, tier, note))| {
                let item = PantryItemDoc {
                    name,
                    presence: presence.parse().unwrap(),
                    bought: bought.map(|s: String| s.parse().unwrap()),
                    tier: tier.map(|s: String| Slug::new(s).unwrap()),
                    note,
                };
                (slug, item)
            })
            .collect(),
    })
}

fn arb_equipment_doc() -> impl Strategy<Value = EquipmentDoc> {
    vec((slug_str("tool", 6), text()), 0..5).prop_map(|items| EquipmentDoc {
        schema_version: 1,
        items: items.into_iter().collect(),
    })
}

fn arb_shops_doc() -> impl Strategy<Value = ShopsDoc> {
    vec(text1(), 0..4).prop_map(|names| ShopsDoc {
        schema_version: 1,
        tiers: names
            .into_iter()
            .enumerate()
            .map(|(i, name)| TierDoc { id: format!("tier{i}"), name })
            .collect(),
    })
}

fn arb_portions() -> impl Strategy<Value = BTreeMap<String, PortionDoc>> {
    vec(
        (slug_str("p", 8), (text1(), 0u32..10, date_str())),
        0..4,
    )
    .prop_map(|portions| {
        portions
            .into_iter()
            .map(|(id, (dish, servings, date))| (id, PortionDoc { dish, servings, date }))
            .collect()
    })
}

fn arb_fridge_doc() -> impl Strategy<Value = FridgeDoc> {
    (
        arb_portions(),
        vec((text1(), arb_portions()), 0..2),
    )
        .prop_map(|(fridge, freezers)| FridgeDoc {
            schema_version: 1,
            fridge,
            freezers: freezers.into_iter().collect(),
        })
}

fn arb_location_docs() -> impl Strategy<Value = LocationDocs> {
    (arb_pantry_doc(), arb_equipment_doc(), arb_shops_doc(), arb_fridge_doc()).prop_map(
        |(pantry, equipment, shops, fridge)| LocationDocs { pantry, equipment, shops, fridge },
    )
}

fn arb_queue_doc() -> impl Strategy<Value = QueueDoc> {
    vec(
        (
            slug_str("q", 8),
            (
                vec(
                    (proptest::option::of(slug_str("r", 6)), text1()),
                    0..3,
                ),
                opt_text(),
                date_str(),
            ),
        ),
        0..4,
    )
    .prop_map(|entries| QueueDoc {
        schema_version: 1,
        entries: entries
            .into_iter()
            .map(|(id, (dishes, reason, added))| {
                (
                    id,
                    QueueEntryDoc {
                        dishes: dishes
                            .into_iter()
                            .map(|(recipe, title)| DishRefDoc { recipe, title })
                            .collect(),
                        reason,
                        added,
                    },
                )
            })
            .collect(),
    })
}

fn arb_shopping_doc() -> impl Strategy<Value = ShoppingDoc> {
    vec(
        (slug_str("s", 8), (text1(), proptest::option::of(slug_str("tier", 4)), any::<bool>())),
        0..5,
    )
    .prop_map(|items| ShoppingDoc {
        schema_version: 1,
        items: items
            .into_iter()
            .map(|(id, (text, tier, done))| (id, ShoppingItemDoc { text, tier, done }))
            .collect(),
    })
}

fn arb_kv(prefix: &'static str) -> impl Strategy<Value = BTreeMap<String, String>> {
    vec((slug_str(prefix, 6), text()), 0..4).prop_map(|kv| kv.into_iter().collect())
}

fn arb_recipe_doc() -> impl Strategy<Value = RecipeDoc> {
    (
        (
            text1(),
            1u32..12,
            prop_oneof![Just("weekday"), Just("project")].prop_map(str::to_string),
            proptest::option::of((1u32..20_000, text1())),
        ),
        arb_kv("axis"),
        vec(slug_str("tool", 6).prop_map(|s| Slug::new(s).unwrap()), 0..3),
        vec(
            (text1(), proptest::option::of(slug_str("item", 8).prop_map(|s| Slug::new(s).unwrap()))),
            0..5,
        ),
        proptest::option::of(text1()),
        prop_oneof![
            Just(RecipeStatus::Draft),
            Just(RecipeStatus::Active),
            Just(RecipeStatus::Retired)
        ],
        body(),
    )
        .prop_map(
            |((title, servings, effort, lead), tags, equipment, ingredients, source, status, body)| {
                RecipeDoc {
                    schema_version: 1,
                    title,
                    servings,
                    effort: effort.parse().unwrap(),
                    lead: lead.map(|(minutes, act_now_step)| LeadTimeDoc { minutes, act_now_step }),
                    tags,
                    equipment,
                    ingredients: ingredients
                        .into_iter()
                        .map(|(text, pantry)| IngredientDoc { text, pantry })
                        .collect(),
                    source,
                    status,
                    body: body.as_str().into(),
                }
            },
        )
}

fn arb_technique_doc() -> impl Strategy<Value = TechniqueDoc> {
    (text1(), arb_kv("axis"), body()).prop_map(|(title, tags, body)| TechniqueDoc {
        schema_version: 1,
        title,
        tags,
        body: body.as_str().into(),
    })
}

fn arb_log_entry(location_pool: Vec<String>) -> impl Strategy<Value = LogEntry> {
    (
        date_str(),
        0usize..3,
        proptest::option::of(slug_str("r", 6)),
        text1(),
        0..location_pool.len().max(1),
        0u32..12,
        text(),
        arb_kv("axis"),
    )
        .prop_map(move |(date, kind, recipe, title, loc_ix, servings, verdict, tags)| LogEntry {
            date: date.parse().unwrap(),
            kind: [CookKind::Meal, CookKind::Bake, CookKind::Staple][kind],
            recipe: recipe.map(|r| Slug::new(r).unwrap()),
            title,
            location: location_pool
                .get(loc_ix)
                .cloned()
                .unwrap_or_else(|| "home".to_string()),
            servings,
            verdict,
            tags,
        })
}

fn arb_thread_message() -> impl Strategy<Value = ThreadMessage> {
    let thread = prop_oneof![
        Just("planning".to_string()),
        slug_str("r", 6).prop_map(|s| format!("recipe/{s}")),
        slug_str("t", 4).prop_map(|s| format!("technique/{s}")),
        Just("queue".to_string()),
    ];
    (
        thread,
        prop_oneof![Just(Role::User), Just(Role::Assistant)],
        body().prop_map(|s| if s.is_empty() { "noted".to_string() } else { s }),
        // A small time pool so equal timestamps (uid tiebreak) actually occur.
        (date_str(), 0u8..3, 0u8..3),
    )
        .prop_map(|(thread, role, content, (date, h, m))| ThreadMessage {
            thread: ThreadId::parse(&thread).unwrap(),
            role,
            content,
            created: format!("{date}T{h:02}:{m:02}:00").parse().unwrap(),
        })
}

/// A well-formed corpus: trimmed strings, no empty options, valid slug keys.
pub fn arb_corpus() -> impl Strategy<Value = CorpusState> {
    let locations = prop_oneof![
        Just(vec!["home".to_string()]),
        Just(vec!["home".to_string(), "cottage".to_string()]),
    ];
    locations.prop_flat_map(|names| {
        let n = names.len();
        (
            (
                Just(names.clone()),
                vec(arb_location_docs(), n..=n),
                vec(1u32..6, n..=n),
            ),
            arb_queue_doc(),
            arb_queue_doc(),
            arb_shopping_doc(),
            arb_kv("goal"),
            arb_kv("fact"),
            vec((slug_str("r", 6), arb_recipe_doc()), 0..3),
            vec((slug_str("t", 4), arb_technique_doc()), 0..2),
            vec(arb_log_entry(names.clone()), 0..6),
            vec(arb_thread_message(), 0..5),
        )
            .prop_map(
                |(
                    (names, location_docs, headcounts),
                    queue,
                    someday,
                    shopping,
                    steering,
                    facts,
                    recipes,
                    techniques,
                    log,
                    thread_messages,
                )| {
                    let state = StateDoc {
                        schema_version: 1,
                        active_location: names[0].clone(),
                        locations: names
                            .iter()
                            .zip(&headcounts)
                            .map(|(n, h)| (n.clone(), LocationMeta { headcount: *h }))
                            .collect(),
                    };
                    let mut threads: BTreeMap<String, Vec<ThreadMessage>> = BTreeMap::new();
                    for msg in thread_messages {
                        threads.entry(msg.thread.to_string()).or_default().push(msg);
                    }
                    CorpusState {
                        state,
                        queue,
                        someday,
                        shopping,
                        steering: SteeringDoc { schema_version: 1, entries: steering },
                        facts: FactsDoc { schema_version: 1, facts },
                        locations: names.iter().cloned().zip(location_docs).collect(),
                        recipes: recipes.into_iter().collect(),
                        techniques: techniques.into_iter().collect(),
                        log,
                        threads,
                    }
                },
            )
    })
}
