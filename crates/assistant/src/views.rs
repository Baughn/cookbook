//! Read-only textual views over the store — the assistant's eyes. Pure
//! functions of store state and the clock, so tool output is deterministic.

// These return the store's own Result so the tool layer can apply its
// one error policy; the Err size is the store's business.
#![allow(clippy::result_large_err)]

use jiff::civil::{Date, DateTime};
use mise_core::coverage::coverage;
use mise_core::readiness::{self, Verdict};
use mise_core::types::{LocationView, Slug};
use mise_store::error::Result;
use mise_store::pages::{DishRefDoc, QueueDoc, RecipeDoc};
use mise_store::{DocId, Store};

/// The queue with readiness annotations, coverage, and the someday shelf —
/// the same picture the CLI shows, as a string for tool output.
pub fn queue_status(store: &Store, now: DateTime) -> Result<String> {
    let today = now.date();
    let (loc, view) = store.active_view()?;
    let queue: QueueDoc = store.get(&DocId::Queue)?;
    let someday: QueueDoc = store.get(&DocId::Someday)?;

    let mut out = format!("Queue — {loc} (cooking for {})\n", view.headcount);
    if queue.entries.is_empty() {
        out.push_str("  (empty)\n");
    }
    let mut entries: Vec<_> = queue.entries.iter().collect();
    entries.sort_by_key(|(id, e)| (e.added.clone(), (*id).clone()));
    for (id, entry) in entries {
        let age = entry
            .added
            .parse::<Date>()
            .ok()
            .map(|d| (today - d).get_days())
            .filter(|days| *days > 0)
            .map(|days| format!(", {days}d on the queue"))
            .unwrap_or_default();
        if let [dish] = entry.dishes.as_slice() {
            out.push_str(&format!("  • {id}: {}\n", dish_line(store, &view, dish, now)?));
        } else {
            out.push_str(&format!("  • {id} (menu):\n"));
            for dish in &entry.dishes {
                out.push_str(&format!("      - {}\n", dish_line(store, &view, dish, now)?));
            }
        }
        match &entry.reason {
            Some(reason) => {
                out.push_str(&format!("      why: {reason} (added {}{age})\n", entry.added));
            }
            None => out.push_str(&format!("      added {}{age}\n", entry.added)),
        }
    }

    let cov = coverage(&view.fridge, &view.freezer, view.headcount, today);
    let freezer_note = if cov.freezer_dinners > 0 {
        format!(
            " — unless you defrost: +{} dinner{} to {}",
            cov.freezer_dinners,
            if cov.freezer_dinners == 1 { "" } else { "s" },
            cov.runs_out_with_freezer,
        )
    } else {
        String::new()
    };
    out.push('\n');
    match cov.dinners {
        0 => out.push_str(&format!(
            "Fridge: nothing cooked — you run out of food today{freezer_note}\n"
        )),
        n => out.push_str(&format!(
            "Fridge: {n} dinner{} covered — you run out {}{freezer_note}\n",
            if n == 1 { "" } else { "s" },
            cov.runs_out,
        )),
    }

    if !someday.entries.is_empty() {
        out.push_str("\nSomeday shelf:\n");
        for (id, entry) in &someday.entries {
            let titles: Vec<&str> = entry.dishes.iter().map(|d| d.title.as_str()).collect();
            out.push_str(&format!("  · {id}: {}\n", titles.join(" + ")));
        }
    }
    Ok(out)
}

fn dish_line(
    store: &Store,
    view: &LocationView,
    dish: &DishRefDoc,
    now: DateTime,
) -> Result<String> {
    let Some(recipe_slug) = &dish.recipe else {
        return Ok(format!("{} (idea — no recipe yet)", dish.title));
    };
    let s = Slug::new(recipe_slug.as_str())
        .map_err(|e| mise_store::StoreError::Corrupt(e.to_string()))?;
    let doc: RecipeDoc = store.get(&DocId::Recipe(s.clone()))?;
    let meta = doc.to_core(&s)?;
    let assessment = readiness::assess(&meta, view);
    let verdict = match assessment.verdict(&view.tiers) {
        Verdict::Ready => "ready now".to_string(),
        Verdict::AfterLead(lead) => {
            let ready = readiness::ready_at(now, &lead);
            format!(
                "start now: {} → ready {} {:02}:{:02}",
                lead.act_now_step,
                ready.date(),
                ready.hour(),
                ready.minute(),
            )
        }
        Verdict::NeedsShopping { tier } => {
            let items: Vec<&str> = assessment.shop.iter().map(|n| n.item.as_str()).collect();
            let tier_name = tier
                .and_then(|t| view.tiers.iter().find(|x| x.id == t).map(|x| x.name.clone()))
                .unwrap_or_else(|| "source unknown".to_string());
            format!("shop — {tier_name}: {}", items.join(", "))
        }
        Verdict::MissingEquipment => {
            let items: Vec<&str> =
                assessment.missing_equipment.iter().map(|e| e.as_str()).collect();
            format!("missing equipment here: {}", items.join(", "))
        }
    };
    let unlinked = match assessment.unlinked.len() {
        0 => String::new(),
        1 => " · 1 unlinked ingredient".to_string(),
        n => format!(" · {n} unlinked ingredients"),
    };
    Ok(format!("{} [{}] — {verdict}{unlinked}", dish.title, meta.effort))
}
