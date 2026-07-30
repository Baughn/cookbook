//! CRDT convergence, concretely: random operation sequences applied to
//! forked replicas under seeded interleavings, merged both ways, asserting
//! identical final state. Idempotence and commutativity are standalone
//! properties. The motivating scenario — offline shopping-list checkoffs in
//! a signal-dead store while a desktop thread edits the pantry — gets its
//! explicit named test, and the composed property (convergence ∘ export
//! determinism) gets its own: two devices, same files.

mod support;

use std::collections::BTreeMap;

use automerge::{ActorId, AutoCommit};
use autosurgeon::{hydrate, reconcile};
use mise_store::pages::{
    CorpusState, DishRefDoc, FactsDoc, FridgeDoc, IngredientDoc, LocationDocs, PantryDoc,
    PantryItemDoc, PortionDoc, QueueDoc, QueueEntryDoc, RecipeDoc, ShoppingDoc, ShoppingItemDoc,
    ShopsDoc, StateDoc, SteeringDoc, EquipmentDoc,
};
use mise_store::render::render;
use mise_store::store::DEFAULT_TIERS;
use proptest::collection::vec;
use proptest::prelude::*;

// ---------------------------------------------------------------- replica --

struct Replica {
    pantry: AutoCommit,
    shopping: AutoCommit,
    queue: AutoCommit,
    fridge: AutoCommit,
    recipe: AutoCommit,
}

type Snapshot = (PantryDoc, ShoppingDoc, QueueDoc, FridgeDoc, RecipeDoc);

fn seeded_doc<T: autosurgeon::Reconcile>(value: &T) -> AutoCommit {
    let mut doc = AutoCommit::new().with_actor(ActorId::from(b"base".as_slice()));
    reconcile(&mut doc, value).unwrap();
    doc
}

fn base_replica() -> Replica {
    let mut pantry = PantryDoc::empty();
    for (slug, presence) in [("miso", "have"), ("eggs", "out"), ("rice", "have")] {
        pantry.items.insert(
            slug.to_string(),
            PantryItemDoc {
                name: slug.to_string(),
                presence: presence.to_string(),
                bought: None,
                tier: Some("shop".to_string()),
                note: None,
            },
        );
    }
    let mut shopping = ShoppingDoc::empty();
    for (id, text) in [("eggs", "a dozen eggs"), ("spring-onion", "spring onions")] {
        shopping.items.insert(
            id.to_string(),
            ShoppingItemDoc { text: text.to_string(), tier: Some("shop".to_string()), done: false },
        );
    }
    let queue = QueueDoc::empty();
    let fridge = FridgeDoc::empty();
    let recipe = RecipeDoc {
        schema_version: 1,
        title: "Wok thing".to_string(),
        servings: 4,
        effort: "weekday".to_string(),
        lead: None,
        tags: BTreeMap::from([("format".to_string(), "stir-fry".to_string())]),
        equipment: vec!["wok".to_string()],
        ingredients: vec![IngredientDoc {
            text: "400 g something".to_string(),
            pantry: Some("rice".to_string()),
        }],
        status: "active".into(),
        body: "Heat the wok. Add oil. Fry the aromatics. Serve.".into(),
    };
    Replica {
        pantry: seeded_doc(&pantry),
        shopping: seeded_doc(&shopping),
        queue: seeded_doc(&queue),
        fridge: seeded_doc(&fridge),
        recipe: seeded_doc(&recipe),
    }
}

impl Replica {
    fn fork(&mut self, actor: &[u8]) -> Replica {
        let f = |doc: &mut AutoCommit| doc.fork().with_actor(ActorId::from(actor));
        Replica {
            pantry: f(&mut self.pantry),
            shopping: f(&mut self.shopping),
            queue: f(&mut self.queue),
            fridge: f(&mut self.fridge),
            recipe: f(&mut self.recipe),
        }
    }

    fn merge_from(&mut self, other: &mut Replica) {
        self.pantry.merge(&mut other.pantry).unwrap();
        self.shopping.merge(&mut other.shopping).unwrap();
        self.queue.merge(&mut other.queue).unwrap();
        self.fridge.merge(&mut other.fridge).unwrap();
        self.recipe.merge(&mut other.recipe).unwrap();
    }

    fn snapshot(&self) -> Snapshot {
        (
            hydrate(&self.pantry).unwrap(),
            hydrate(&self.shopping).unwrap(),
            hydrate(&self.queue).unwrap(),
            hydrate(&self.fridge).unwrap(),
            hydrate(&self.recipe).unwrap(),
        )
    }

    fn corpus(&self) -> CorpusState {
        let (pantry, shopping, queue, fridge, recipe) = self.snapshot();
        CorpusState {
            state: StateDoc::new("home", 2),
            queue,
            someday: QueueDoc::empty(),
            shopping,
            steering: SteeringDoc::empty(),
            facts: FactsDoc::empty(),
            locations: BTreeMap::from([(
                "home".to_string(),
                LocationDocs {
                    pantry,
                    equipment: EquipmentDoc::empty(),
                    shops: ShopsDoc::new(DEFAULT_TIERS),
                    fridge,
                },
            )]),
            recipes: BTreeMap::from([("wok-thing".to_string(), recipe)]),
            techniques: BTreeMap::new(),
            log: vec![],
            threads: BTreeMap::new(),
        }
    }
}

// -------------------------------------------------------------------- ops --

#[derive(Clone, Debug)]
enum Op {
    PantryUpsert { k: u8, presence: u8, tier: Option<u8>, day: Option<u8> },
    PantryRemove { k: u8 },
    ShoppingAdd { k: u8, text: String },
    ShoppingToggle { k: u8 },
    ShoppingRemove { k: u8 },
    QueueAdd { k: u8, title: String, day: u8 },
    QueueRemove { k: u8 },
    FridgeAdd { k: u8, servings: u8, day: u8 },
    FridgeRemove { k: u8 },
    BodySplice { pos: u16, del: u8, insert: String },
    TitleSet { title: String },
}

fn item_key(k: u8) -> String {
    // Small key space, so concurrent edits collide often.
    ["miso", "eggs", "rice", "duck-legs", "wakame"][(k % 5) as usize].to_string()
}

fn apply(r: &mut Replica, op: &Op) {
    match op {
        Op::PantryUpsert { k, presence, tier, day } => {
            let mut doc: PantryDoc = hydrate(&r.pantry).unwrap();
            let key = item_key(*k);
            doc.items.insert(
                key.clone(),
                PantryItemDoc {
                    name: key,
                    presence: ["have", "low", "out"][(*presence % 3) as usize].to_string(),
                    bought: day.map(|d| format!("2026-07-{:02}", 1 + d % 28)),
                    tier: tier.map(|t| ["staples", "shop", "butcher", "town"][(t % 4) as usize].to_string()),
                    note: None,
                },
            );
            reconcile(&mut r.pantry, &doc).unwrap();
        }
        Op::PantryRemove { k } => {
            let mut doc: PantryDoc = hydrate(&r.pantry).unwrap();
            doc.items.remove(&item_key(*k));
            reconcile(&mut r.pantry, &doc).unwrap();
        }
        Op::ShoppingAdd { k, text } => {
            let mut doc: ShoppingDoc = hydrate(&r.shopping).unwrap();
            doc.items.insert(
                format!("s{}", k % 6),
                ShoppingItemDoc { text: text.clone(), tier: None, done: false },
            );
            reconcile(&mut r.shopping, &doc).unwrap();
        }
        Op::ShoppingToggle { k } => {
            let mut doc: ShoppingDoc = hydrate(&r.shopping).unwrap();
            let key = format!("s{}", k % 6);
            let keys: Vec<String> = doc.items.keys().cloned().collect();
            let target = if doc.items.contains_key(&key) {
                Some(key)
            } else {
                keys.first().cloned()
            };
            if let Some(t) = target {
                let item = doc.items.get_mut(&t).unwrap();
                item.done = !item.done;
                reconcile(&mut r.shopping, &doc).unwrap();
            }
        }
        Op::ShoppingRemove { k } => {
            let mut doc: ShoppingDoc = hydrate(&r.shopping).unwrap();
            doc.items.remove(&format!("s{}", k % 6));
            reconcile(&mut r.shopping, &doc).unwrap();
        }
        Op::QueueAdd { k, title, day } => {
            let mut doc: QueueDoc = hydrate(&r.queue).unwrap();
            doc.entries.insert(
                format!("q{}", k % 6),
                QueueEntryDoc {
                    dishes: vec![DishRefDoc { recipe: None, title: title.clone() }],
                    reason: None,
                    added: format!("2026-07-{:02}", 1 + day % 28),
                },
            );
            reconcile(&mut r.queue, &doc).unwrap();
        }
        Op::QueueRemove { k } => {
            let mut doc: QueueDoc = hydrate(&r.queue).unwrap();
            doc.entries.remove(&format!("q{}", k % 6));
            reconcile(&mut r.queue, &doc).unwrap();
        }
        Op::FridgeAdd { k, servings, day } => {
            let mut doc: FridgeDoc = hydrate(&r.fridge).unwrap();
            doc.fridge.insert(
                format!("f{}", k % 6),
                PortionDoc {
                    dish: "leftovers".to_string(),
                    servings: u32::from(*servings % 8),
                    date: format!("2026-07-{:02}", 1 + day % 28),
                },
            );
            reconcile(&mut r.fridge, &doc).unwrap();
        }
        Op::FridgeRemove { k } => {
            let mut doc: FridgeDoc = hydrate(&r.fridge).unwrap();
            doc.fridge.remove(&format!("f{}", k % 6));
            reconcile(&mut r.fridge, &doc).unwrap();
        }
        Op::BodySplice { pos, del, insert } => {
            let mut doc: RecipeDoc = hydrate(&r.recipe).unwrap();
            // The seed body and all inserts are ASCII, so bytes == chars.
            let len = doc.body.as_str().len();
            let pos = (*pos as usize) % (len + 1);
            let del = (*del as usize).min(len - pos);
            doc.body.splice(pos, del as isize, insert);
            reconcile(&mut r.recipe, &doc).unwrap();
        }
        Op::TitleSet { title } => {
            let mut doc: RecipeDoc = hydrate(&r.recipe).unwrap();
            doc.title = title.clone();
            reconcile(&mut r.recipe, &doc).unwrap();
        }
    }
}

fn arb_op() -> impl Strategy<Value = Op> {
    let ascii = || proptest::string::string_regex("[a-z ]{1,8}").unwrap();
    prop_oneof![
        (any::<u8>(), any::<u8>(), proptest::option::of(any::<u8>()), proptest::option::of(any::<u8>()))
            .prop_map(|(k, presence, tier, day)| Op::PantryUpsert { k, presence, tier, day }),
        any::<u8>().prop_map(|k| Op::PantryRemove { k }),
        (any::<u8>(), ascii()).prop_map(|(k, text)| Op::ShoppingAdd { k, text }),
        any::<u8>().prop_map(|k| Op::ShoppingToggle { k }),
        any::<u8>().prop_map(|k| Op::ShoppingRemove { k }),
        (any::<u8>(), ascii(), any::<u8>()).prop_map(|(k, title, day)| Op::QueueAdd { k, title, day }),
        any::<u8>().prop_map(|k| Op::QueueRemove { k }),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(k, servings, day)| Op::FridgeAdd { k, servings, day }),
        any::<u8>().prop_map(|k| Op::FridgeRemove { k }),
        (any::<u16>(), any::<u8>(), ascii()).prop_map(|(pos, del, insert)| Op::BodySplice { pos, del, insert }),
        ascii().prop_map(|title| Op::TitleSet { title }),
    ]
}

// ------------------------------------------------------------- properties --

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Two replicas diverge under independent op sequences (a partition),
    /// then sync: both directions converge to the identical state, and
    /// merging is idempotent and commutative.
    #[test]
    fn replicas_converge(
        base_ops in vec(arb_op(), 0..6),
        ops_a in vec(arb_op(), 0..12),
        ops_b in vec(arb_op(), 0..12),
    ) {
        let mut base = base_replica();
        for op in &base_ops {
            apply(&mut base, op);
        }
        let mut a = base.fork(b"replica-a");
        let mut b = base.fork(b"replica-b");
        for op in &ops_a {
            apply(&mut a, op);
        }
        for op in &ops_b {
            apply(&mut b, op);
        }

        // Commutativity: merge in both orders on independent copies.
        let mut a1 = a.fork(b"copy-a1");
        let mut b1 = b.fork(b"copy-b1");
        a1.merge_from(&mut b1);
        let mut a2 = a.fork(b"copy-a2");
        let mut b2 = b.fork(b"copy-b2");
        b2.merge_from(&mut a2);
        prop_assert_eq!(a1.snapshot(), b2.snapshot());

        // Full sync: both replicas end identical.
        a.merge_from(&mut b);
        b.merge_from(&mut a);
        prop_assert_eq!(a.snapshot(), b.snapshot());

        // Idempotence: merging again changes nothing.
        let before = a.snapshot();
        a.merge_from(&mut b);
        prop_assert_eq!(a.snapshot(), before);
    }

    /// The composed property, and the user-facing promise: converged
    /// replicas produce byte-identical exports. Two devices, same files.
    #[test]
    fn converged_replicas_export_identically(
        ops_a in vec(arb_op(), 0..12),
        ops_b in vec(arb_op(), 0..12),
    ) {
        let mut base = base_replica();
        let mut a = base.fork(b"replica-a");
        let mut b = base.fork(b"replica-b");
        for op in &ops_a {
            apply(&mut a, op);
        }
        for op in &ops_b {
            apply(&mut b, op);
        }
        a.merge_from(&mut b);
        b.merge_from(&mut a);
        prop_assert_eq!(render(&a.corpus()), render(&b.corpus()));
    }
}

/// The motivating scenario, by name: checking off shopping items in a
/// signal-dead basement while a desktop thread edits the pantry. Item-level
/// merges compose; neither side's edits are lost.
#[test]
fn basement_checkoff_merges_with_desktop_pantry_edit() {
    let mut base = base_replica();
    let mut phone = base.fork(b"phone");
    let mut desktop = base.fork(b"desktop");

    // In the basement, offline: eggs bought, plus an impulse add.
    let mut shopping: ShoppingDoc = hydrate(&phone.shopping).unwrap();
    shopping.items.get_mut("eggs").unwrap().done = true;
    shopping.items.insert(
        "dashi".to_string(),
        ShoppingItemDoc { text: "instant dashi".to_string(), tier: None, done: false },
    );
    reconcile(&mut phone.shopping, &shopping).unwrap();

    // Meanwhile a desktop planning thread edits the pantry.
    let mut pantry: PantryDoc = hydrate(&desktop.pantry).unwrap();
    pantry.items.get_mut("miso").unwrap().presence = "out".to_string();
    pantry.items.insert(
        "duck-legs".to_string(),
        PantryItemDoc {
            name: "duck legs".to_string(),
            presence: "have".to_string(),
            bought: Some("2026-07-28".to_string()),
            tier: Some("butcher".to_string()),
            note: None,
        },
    );
    reconcile(&mut desktop.pantry, &pantry).unwrap();

    // Signal returns; sync both ways.
    phone.merge_from(&mut desktop);
    desktop.merge_from(&mut phone);
    assert_eq!(phone.snapshot(), desktop.snapshot());

    // Nothing was lost in either direction.
    let (pantry, shopping, ..) = phone.snapshot();
    assert!(shopping.items["eggs"].done);
    assert_eq!(shopping.items["dashi"].text, "instant dashi");
    assert_eq!(pantry.items["miso"].presence, "out");
    assert_eq!(pantry.items["duck-legs"].presence, "have");
}
