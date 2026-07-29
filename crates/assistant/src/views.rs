//! Read-only views over the store — the assistant's eyes and the API's
//! payloads. Pure functions of store state and the clock: one structured
//! view, two renderings (tool-output string here, JSON at the server).

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
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct QueueView {
    pub location: String,
    pub headcount: u32,
    pub entries: Vec<QueueEntryView>,
    pub coverage: CoverageView,
    pub someday: Vec<SomedayView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueueEntryView {
    pub id: String,
    pub dishes: Vec<DishView>,
    pub reason: Option<String>,
    pub added: String,
    /// Days on the queue, when positive and `added` parses.
    pub age_days: Option<i32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DishView {
    pub title: String,
    pub recipe: Option<String>,
    /// Absent for recipe-less ideas.
    pub effort: Option<String>,
    pub unlinked: usize,
    pub verdict: VerdictView,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum VerdictView {
    /// No recipe yet — nothing to assess.
    Idea,
    Ready,
    Lead {
        step: String,
        ready_date: String,
        /// `HH:MM`
        ready_time: String,
    },
    Shop {
        tier: Option<String>,
        tier_name: String,
        items: Vec<String>,
    },
    MissingEquipment { items: Vec<String> },
}

#[derive(Clone, Debug, Serialize)]
pub struct CoverageView {
    pub dinners: u32,
    pub runs_out: String,
    pub freezer_dinners: u32,
    pub runs_out_with_freezer: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SomedayView {
    pub id: String,
    pub titles: Vec<String>,
}

/// The queue with readiness annotations, coverage, and the someday shelf
/// for the active location, as structured data.
pub fn queue_view(store: &Store, now: DateTime) -> Result<QueueView> {
    let today = now.date();
    let (loc, view) = store.active_view()?;
    let queue: QueueDoc = store.get(&DocId::Queue)?;
    let someday: QueueDoc = store.get(&DocId::Someday)?;

    let mut sorted: Vec<_> = queue.entries.iter().collect();
    sorted.sort_by_key(|(id, e)| (e.added.clone(), (*id).clone()));
    let entries = sorted
        .into_iter()
        .map(|(id, entry)| {
            let dishes = entry
                .dishes
                .iter()
                .map(|dish| dish_view(store, &view, dish, now))
                .collect::<Result<Vec<_>>>()?;
            Ok(QueueEntryView {
                id: id.clone(),
                dishes,
                reason: entry.reason.clone(),
                added: entry.added.clone(),
                age_days: entry
                    .added
                    .parse::<Date>()
                    .ok()
                    .map(|d| (today - d).get_days())
                    .filter(|days| *days > 0),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let cov = coverage(&view.fridge, &view.freezer, view.headcount, today);
    Ok(QueueView {
        location: loc.to_string(),
        headcount: view.headcount.get(),
        entries,
        coverage: CoverageView {
            dinners: cov.dinners,
            runs_out: cov.runs_out.to_string(),
            freezer_dinners: cov.freezer_dinners,
            runs_out_with_freezer: cov.runs_out_with_freezer.to_string(),
        },
        someday: someday
            .entries
            .iter()
            .map(|(id, entry)| SomedayView {
                id: id.clone(),
                titles: entry.dishes.iter().map(|d| d.title.clone()).collect(),
            })
            .collect(),
    })
}

fn dish_view(
    store: &Store,
    view: &LocationView,
    dish: &DishRefDoc,
    now: DateTime,
) -> Result<DishView> {
    let Some(recipe_slug) = &dish.recipe else {
        return Ok(DishView {
            title: dish.title.clone(),
            recipe: None,
            effort: None,
            unlinked: 0,
            verdict: VerdictView::Idea,
        });
    };
    let s = Slug::new(recipe_slug.as_str())
        .map_err(|e| mise_store::StoreError::Corrupt(e.to_string()))?;
    let doc: RecipeDoc = store.get(&DocId::Recipe(s.clone()))?;
    let meta = doc.to_core(&s)?;
    let assessment = readiness::assess(&meta, view);
    let verdict = match assessment.verdict(&view.tiers) {
        Verdict::Ready => VerdictView::Ready,
        Verdict::AfterLead(lead) => {
            let ready = readiness::ready_at(now, &lead);
            VerdictView::Lead {
                step: lead.act_now_step.clone(),
                ready_date: ready.date().to_string(),
                ready_time: format!("{:02}:{:02}", ready.hour(), ready.minute()),
            }
        }
        Verdict::NeedsShopping { tier } => VerdictView::Shop {
            tier_name: tier
                .as_ref()
                .and_then(|t| view.tiers.iter().find(|x| &x.id == t).map(|x| x.name.clone()))
                .unwrap_or_else(|| "source unknown".to_string()),
            tier: tier.map(|t| t.to_string()),
            items: assessment.shop.iter().map(|n| n.item.to_string()).collect(),
        },
        Verdict::MissingEquipment => VerdictView::MissingEquipment {
            items: assessment.missing_equipment.iter().map(|e| e.to_string()).collect(),
        },
    };
    Ok(DishView {
        title: dish.title.clone(),
        recipe: Some(recipe_slug.clone()),
        effort: Some(meta.effort.to_string()),
        unlinked: assessment.unlinked.len(),
        verdict,
    })
}

/// One line for one dish: `title [effort] — verdict · n unlinked`.
fn dish_line(d: &DishView) -> String {
    if d.recipe.is_none() {
        return format!("{} (idea — no recipe yet)", d.title);
    }
    let verdict = match &d.verdict {
        VerdictView::Idea => unreachable!("ideas have no recipe"),
        VerdictView::Ready => "ready now".to_string(),
        VerdictView::Lead { step, ready_date, ready_time } => {
            format!("start now: {step} → ready {ready_date} {ready_time}")
        }
        VerdictView::Shop { tier_name, items, .. } => {
            format!("shop — {tier_name}: {}", items.join(", "))
        }
        VerdictView::MissingEquipment { items } => {
            format!("missing equipment here: {}", items.join(", "))
        }
    };
    let unlinked = match d.unlinked {
        0 => String::new(),
        1 => " · 1 unlinked ingredient".to_string(),
        n => format!(" · {n} unlinked ingredients"),
    };
    format!(
        "{} [{}] — {verdict}{unlinked}",
        d.title,
        d.effort.as_deref().unwrap_or("?"),
    )
}

/// The tool-output rendering of [`queue_view`].
pub fn render_queue_status(v: &QueueView) -> String {
    let mut out = format!("Queue — {} (cooking for {})\n", v.location, v.headcount);
    if v.entries.is_empty() {
        out.push_str("  (empty)\n");
    }
    for entry in &v.entries {
        let age = entry
            .age_days
            .map(|days| format!(", {days}d on the queue"))
            .unwrap_or_default();
        if let [dish] = entry.dishes.as_slice() {
            out.push_str(&format!("  • {}: {}\n", entry.id, dish_line(dish)));
        } else {
            out.push_str(&format!("  • {} (menu):\n", entry.id));
            for dish in &entry.dishes {
                out.push_str(&format!("      - {}\n", dish_line(dish)));
            }
        }
        match &entry.reason {
            Some(reason) => {
                out.push_str(&format!("      why: {reason} (added {}{age})\n", entry.added));
            }
            None => out.push_str(&format!("      added {}{age}\n", entry.added)),
        }
    }

    let cov = &v.coverage;
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

    if !v.someday.is_empty() {
        out.push_str("\nSomeday shelf:\n");
        for s in &v.someday {
            out.push_str(&format!("  · {}: {}\n", s.id, s.titles.join(" + ")));
        }
    }
    out
}

/// The queue picture as tool output — [`queue_view`] rendered.
pub fn queue_status(store: &Store, now: DateTime) -> Result<String> {
    Ok(render_queue_status(&queue_view(store, now)?))
}
