//! Turning a fetched page into something worth a model's attention. Pure
//! text → text, no IO: the [`crate::fetch::Fetch`] seam does the network,
//! this module does the reading, and tests feed it fixture HTML.
//!
//! Recipe sites are messy for humans but SEO keeps them clean for
//! machines: most embed a schema.org `Recipe` as JSON-LD with the exact
//! ingredients and steps and none of the life story. That's the first
//! pass. Anything else falls back to Readability-style extraction of the
//! main content, rendered as markdown.

use serde_json::Value;

/// Keep tool results a fraction of the context, not a flood.
const MAX_MARKDOWN: usize = 20_000;

/// Extract the readable substance of a page as markdown, or say why not.
/// The error is model-facing (it lands in an error tool result).
pub fn extract(html: &str, url: &str) -> Result<String, String> {
    let md = match json_ld_recipe(html) {
        Some(recipe) => recipe,
        None => readable_article(html, url)?,
    };
    let mut md = md.trim().to_string();
    if md.is_empty() {
        return Err("the page had no readable content".into());
    }
    if md.len() > MAX_MARKDOWN {
        let mut cut = MAX_MARKDOWN;
        while !md.is_char_boundary(cut) {
            cut -= 1;
        }
        md.truncate(cut);
        md.push_str("\n\n[truncated]");
    }
    md.push_str(&format!("\n\nSource: {url}\n"));
    Ok(md)
}

// ------------------------------------------------------------- JSON-LD --

/// The first schema.org `Recipe` object on the page, as markdown.
fn json_ld_recipe(html: &str) -> Option<String> {
    let doc = dom_query::Document::from(html);
    for script in doc.select(r#"script[type="application/ld+json"]"#).iter() {
        let Ok(value) = serde_json::from_str::<Value>(&script.text()) else {
            continue; // malformed blobs are common; skip, don't fail
        };
        if let Some(recipe) = find_recipe(&value) {
            return Some(render_recipe(recipe));
        }
    }
    None
}

/// Recipes hide at the top level, in arrays, and in `@graph` bundles.
fn find_recipe(value: &Value) -> Option<&Value> {
    match value {
        Value::Array(items) => items.iter().find_map(find_recipe),
        Value::Object(map) => {
            let is_recipe = match map.get("@type") {
                Some(Value::String(t)) => t == "Recipe",
                Some(Value::Array(ts)) => ts.iter().any(|t| t == "Recipe"),
                _ => false,
            };
            if is_recipe {
                return Some(value);
            }
            map.get("@graph").and_then(find_recipe)
        }
        _ => None,
    }
}

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        // recipeYield and friends are sometimes numbers or ["4", "4 servings"].
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => items.iter().find_map(str_of),
        _ => None,
    }
}

/// "PT1H30M" → "1 h 30 min"; anything unparseable passes through raw.
fn duration(v: &Value) -> Option<String> {
    let raw = str_of(v)?;
    let Ok(span) = raw.parse::<jiff::Span>() else {
        return Some(raw);
    };
    let (h, m) = (span.get_hours(), span.get_minutes());
    Some(match (h, m) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    })
}

/// Instruction lists come as strings, `HowToStep`s, or `HowToSection`s of
/// steps; flatten them all into one numbered method.
fn instruction_lines(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        Value::Array(items) => items.iter().for_each(|i| instruction_lines(i, out)),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(str_of) {
                out.push(text);
            } else if let Some(items) = map.get("itemListElement") {
                instruction_lines(items, out);
            }
        }
        _ => {}
    }
}

fn render_recipe(recipe: &Value) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let title = recipe.get("name").and_then(str_of).unwrap_or_else(|| "Recipe".into());
    let _ = writeln!(out, "# {title}\n");
    if let Some(d) = recipe.get("description").and_then(str_of) {
        let _ = writeln!(out, "{d}\n");
    }
    let mut facts: Vec<String> = Vec::new();
    if let Some(y) = recipe.get("recipeYield").and_then(str_of) {
        facts.push(format!("Yield: {y}"));
    }
    for (label, key) in [("Prep", "prepTime"), ("Cook", "cookTime"), ("Total", "totalTime")] {
        if let Some(d) = recipe.get(key).and_then(duration) {
            facts.push(format!("{label}: {d}"));
        }
    }
    for fact in &facts {
        let _ = writeln!(out, "- {fact}");
    }
    if !facts.is_empty() {
        out.push('\n');
    }

    if let Some(Value::Array(lines)) = recipe.get("recipeIngredient") {
        out.push_str("## Ingredients\n\n");
        for line in lines.iter().filter_map(str_of) {
            let _ = writeln!(out, "- {line}");
        }
        out.push('\n');
    }
    let mut steps = Vec::new();
    if let Some(instructions) = recipe.get("recipeInstructions") {
        instruction_lines(instructions, &mut steps);
    }
    if !steps.is_empty() {
        out.push_str("## Method\n\n");
        for (i, step) in steps.iter().enumerate() {
            let _ = writeln!(out, "{}. {step}", i + 1);
        }
    }
    out
}

// --------------------------------------------------------- readability --

/// Main-content extraction for pages without recipe data: strip nav, ads,
/// and comments, keep the article, hand it over as markdown.
fn readable_article(html: &str, url: &str) -> Result<String, String> {
    let mut readability = dom_smoothie::Readability::new(html, Some(url), None)
        .map_err(|e| format!("unreadable page: {e}"))?;
    let article = readability.parse().map_err(|e| format!("unreadable page: {e}"))?;
    let mut md = format!("# {}\n\n", article.title.trim());
    let body = htmd::convert(article.content.as_ref())
        .map_err(|e| format!("unreadable page: {e}"))?;
    md.push_str(body.trim());
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical SEO blob: the recipe nested in @graph, typed as an array,
    /// sections in the instructions, ISO durations — surrounded by a page
    /// full of life story the extraction must ignore.
    const JSON_LD_PAGE: &str = r#"<html><head>
      <script type="application/ld+json">{"broken": </script>
      <script type="application/ld+json">
      {"@context":"https://schema.org","@graph":[
        {"@type":"Organization","name":"Grandma's Blog"},
        {"@type":["Recipe","NewsArticle"],
         "name":"Mapo tofu",
         "description":"The one that works.",
         "recipeYield":["4","4 servings"],
         "prepTime":"PT20M","cookTime":"PT1H10M",
         "recipeIngredient":["400 g silken tofu","2 tbsp doubanjiang"],
         "recipeInstructions":[
           {"@type":"HowToSection","name":"Prep",
            "itemListElement":[{"@type":"HowToStep","text":"Cube the tofu."}]},
           {"@type":"HowToStep","text":"Fry the doubanjiang."},
           "Simmer and serve."
         ]}]}
      </script></head>
      <body><p>It was a rainy Tuesday in 1987 when my grandmother…</p></body></html>"#;

    #[test]
    fn json_ld_recipe_wins_and_renders_clean() {
        let md = extract(JSON_LD_PAGE, "https://example.com/mapo").unwrap();
        assert!(md.starts_with("# Mapo tofu\n"), "{md}");
        assert!(md.contains("- Yield: 4\n"), "{md}");
        assert!(md.contains("- Prep: 20 min\n"), "{md}");
        assert!(md.contains("- Cook: 1 h 10 min\n"), "{md}");
        assert!(md.contains("- 400 g silken tofu\n"), "{md}");
        assert!(md.contains("1. Cube the tofu.\n"), "{md}");
        assert!(md.contains("2. Fry the doubanjiang.\n"), "{md}");
        assert!(md.contains("3. Simmer and serve.\n"), "{md}");
        assert!(md.ends_with("Source: https://example.com/mapo\n"), "{md}");
        assert!(!md.contains("rainy Tuesday"), "life story must not leak: {md}");
    }

    #[test]
    fn pages_without_recipe_data_fall_back_to_readable_markdown() {
        let paragraph = "Salt your eggplant a full hour ahead; nothing else matters as much. "
            .repeat(20);
        let html = format!(
            r#"<html><head><title>On eggplant</title></head><body>
            <nav><a href="/">Home</a><a href="/about">About</a></nav>
            <article><h1>On eggplant</h1><p>{paragraph}</p><p>{paragraph}</p></article>
            <footer>© nobody</footer></body></html>"#
        );
        let md = extract(&html, "https://example.com/eggplant").unwrap();
        assert!(md.contains("Salt your eggplant"), "{md}");
        assert!(!md.contains("About"), "nav must be stripped: {md}");
    }

    #[test]
    fn oversized_pages_truncate_on_a_char_boundary() {
        let long = "Stir gently — æøå. ".repeat(3000);
        let html = format!(
            r#"<html><body><article><h1>Endless</h1><p>{long}</p></article></body></html>"#
        );
        let md = extract(&html, "https://example.com/long").unwrap();
        assert!(md.len() < MAX_MARKDOWN + 100);
        assert!(md.contains("[truncated]"), "{md}");
        assert!(md.ends_with("Source: https://example.com/long\n"));
    }

    #[test]
    fn empty_pages_are_an_error() {
        assert!(extract("<html><body></body></html>", "https://example.com/x").is_err());
    }
}
