//! End-to-end smoke test of the M1 deliverable: init a corpus from scratch,
//! populate it, show the queue with readiness annotations, and find the
//! export committed to a browsable git repo.

use std::path::Path;
use std::process::Command;

fn mise(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mise {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Removing an id that never existed must be an error naming the miss —
/// not a success message that contradicts the git history (`modify`
/// no-ops and commits nothing when the closure changed nothing).
#[test]
fn removing_what_was_never_there_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let root = root.as_path();
    mise(root, &["init", "--location", "home", "--headcount", "2"]);

    for args in [
        &["queue", "remove", "not-there"][..],
        &["pantry", "remove", "nope"],
        &["equipment", "remove", "nothing"],
        &["fridge", "remove", "p9"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_mise"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "mise {args:?} claimed success");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("no such"), "mise {args:?}: {stderr}");
    }
}

#[test]
fn init_populate_queue_export() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let root = root.as_path();

    mise(root, &["init", "--location", "home", "--headcount", "2"]);

    // A recipe with equipment, tags, lead time, and linked ingredients.
    mise(root, &[
        "recipe", "add", "mapo-tofu",
        "--title", "Mapo tofu",
        "--servings", "4",
        "--effort", "weekday",
        "--tag", "cuisine=sichuan",
        "--tag", "protein=pork",
        "--equipment", "wok",
        "--lead-minutes", "720",
        "--lead-step", "defrost the pork",
    ]);
    mise(root, &["recipe", "ingredient", "mapo-tofu", "600 g silken tofu", "--link", "tofu"]);
    mise(root, &["recipe", "ingredient", "mapo-tofu", "doubanjiang", "--link", "doubanjiang"]);
    mise(root, &["recipe", "ingredient", "mapo-tofu", "a splash of shaoxing wine"]);

    // Pantry: tofu on hand, doubanjiang out (town tier). No wok yet.
    mise(root, &["pantry", "set", "tofu", "--presence", "have", "--tier", "shop"]);
    mise(root, &["pantry", "set", "doubanjiang", "--presence", "out", "--tier", "town"]);

    mise(root, &["queue", "add", "Mapo tofu", "--recipe", "mapo-tofu", "--reason", "craving"]);
    mise(root, &["queue", "add", "Something with duck", "--someday"]);
    mise(root, &["fridge", "add", "Sunday mapo", "--servings", "3"]);

    // Missing wok dominates; buying the wok drops it to town-tier shopping;
    // stocking doubanjiang leaves only the lead-time gate.
    let out = mise(root, &["queue"]);
    assert!(out.contains("missing equipment here: wok"), "{out}");
    assert!(out.contains("1 unlinked ingredient"), "{out}");
    assert!(out.contains("why: craving"), "{out}");
    assert!(out.contains("Fridge: 1 dinner covered"), "{out}");
    assert!(out.contains("Someday shelf"), "{out}");

    mise(root, &["equipment", "add", "wok", "--note", "carbon steel"]);
    let out = mise(root, &["queue"]);
    assert!(out.contains("shop — Town: doubanjiang"), "{out}");

    mise(root, &["pantry", "set", "doubanjiang", "--presence", "have", "--bought", "today"]);
    let out = mise(root, &["queue"]);
    assert!(out.contains("start now: defrost the pork"), "{out}");

    let export = root.join("export");
    assert!(export.join("locations/home/pantry.md").exists());

    mise(root, &["location", "add", "cottage", "--headcount", "4"]);
    mise(root, &["location", "use", "cottage"]);
    let out = mise(root, &["queue"]);
    assert!(out.contains("Queue — cottage"), "{out}");
    // The cottage has no tofu: the same queue reads differently there.
    assert!(out.contains("shop") || out.contains("missing equipment"), "{out}");
    mise(root, &["location", "use", "home"]);

    mise(root, &["log", "add", "Mapo tofu", "--recipe", "mapo-tofu", "--verdict", "great, more numbing"]);
    let rotation = mise(root, &["log", "rotation"]);
    assert!(rotation.contains("cuisine=sichuan"), "{rotation}");

    // The export is a browsable git repo with provenance messages.
    let git_log = Command::new("git")
        .arg("-C")
        .arg(export.as_path())
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&git_log.stdout).into_owned();
    assert!(log.contains("cli: queue add mapo-tofu"), "{log}");
    assert!(log.contains("init: empty corpus"), "{log}");
    let recipe_md = std::fs::read_to_string(export.join("recipes/mapo-tofu.md")).unwrap();
    assert!(recipe_md.contains("lead-minutes: 720"), "{recipe_md}");
    assert!(recipe_md.contains("- [tofu] 600 g silken tofu"), "{recipe_md}");
    let log_dir = std::fs::read_dir(export.join("log")).unwrap().count();
    assert_eq!(log_dir, 1, "one month shard expected");
}

/// `mise chat` fails helpfully before any network is involved: an unknown
/// page thread, then a missing API key. (The model itself is never part of
/// the test suite.)
#[test]
fn chat_fails_helpfully_without_page_or_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    mise(&root, &["init"]);

    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .current_dir(dir.path()) // no .env here
        .arg("--root")
        .arg(&root)
        .args(["chat", "hello?", "--page", "recipe/nope"])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no page recipe/nope"), "{stderr}");

    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .current_dir(dir.path())
        .arg("--root")
        .arg(&root)
        .args(["chat", "hello?"])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ANTHROPIC_API_KEY"), "{stderr}");
}
