mod common;

use common::{git, write, Fixture};
use zit::change::{self, Record};
use zit::claim::{self, Claimed, Holder};
use zit::resource::Resource;
use zit::{api, workspace};

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

fn fixture() -> Fixture {
    Fixture::new(&[("src/lib.rs", LIB), ("Cargo.toml", "[package]\n"), ("docs/a.md", "# A\n")])
}

fn res(items: &[&str]) -> Vec<Resource> {
    items.iter().map(|s| Resource::parse(s)).collect()
}

fn refused_by(outcome: Claimed) -> Vec<(String, String)> {
    match outcome {
        Claimed::Granted => panic!("expected a refusal"),
        Claimed::Refused(held) => held
            .into_iter()
            .map(|h| {
                let who = match h.by {
                    Holder::Workspace { agent, .. } => format!("workspace of {agent}"),
                    Holder::Change { agent, .. } => format!("change by {agent}"),
                };
                (h.resource.to_string(), who)
            })
            .collect(),
    }
}

#[test]
fn a_claim_excludes_overlapping_claims_by_other_live_workspaces() {
    let fx = fixture();
    let (a, b) = (fx.workspace("claude"), fx.workspace("codex"));
    assert_eq!(claim::claim(&fx.repo, &a.id, &res(&["Cargo.toml", "src/lib.rs"])).unwrap(), Claimed::Granted);

    let refused = refused_by(claim::claim(&fx.repo, &b.id, &res(&["Cargo.toml"])).unwrap());
    assert_eq!(refused, [("Cargo.toml".to_string(), "workspace of claude".to_string())]);
    // A whole-file claim covers every symbol in the file.
    assert!(matches!(claim::claim(&fx.repo, &b.id, &res(&["src/lib.rs#price"])).unwrap(), Claimed::Refused(_)));
    assert_eq!(claim::claim(&fx.repo, &b.id, &res(&["docs/a.md"])).unwrap(), Claimed::Granted);
    // Claiming again what you already hold is fine.
    assert_eq!(claim::claim(&fx.repo, &a.id, &res(&["Cargo.toml"])).unwrap(), Claimed::Granted);
}

#[test]
fn a_refused_claim_takes_nothing() {
    let fx = fixture();
    let (a, b, c) = (fx.workspace("a"), fx.workspace("b"), fx.workspace("c"));
    claim::claim(&fx.repo, &a.id, &res(&["Cargo.toml"])).unwrap();
    assert!(matches!(claim::claim(&fx.repo, &b.id, &res(&["docs/a.md", "Cargo.toml"])).unwrap(), Claimed::Refused(_)));
    assert_eq!(claim::claim(&fx.repo, &c.id, &res(&["docs/a.md"])).unwrap(), Claimed::Granted);
}

#[test]
fn claims_lapse_with_their_workspace() {
    let fx = fixture();
    let (a, b) = (fx.workspace("a"), fx.workspace("b"));
    claim::claim(&fx.repo, &a.id, &res(&["Cargo.toml"])).unwrap();
    workspace::dispose(&fx.repo, &a.id).unwrap();
    assert_eq!(claim::claim(&fx.repo, &b.id, &res(&["Cargo.toml"])).unwrap(), Claimed::Granted);
}

#[test]
fn a_recorded_change_holds_what_it_wrote_until_it_is_accepted_or_discarded() {
    let fx = fixture();
    let done = fx.change("claude", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let b = fx.workspace("codex");

    let refused = refused_by(claim::claim(&fx.repo, &b.id, &res(&["src/lib.rs#tax"])).unwrap());
    assert_eq!(refused, [("src/lib.rs#tax".to_string(), "change by claude".to_string())]);
    assert_eq!(claim::claim(&fx.repo, &b.id, &res(&["src/lib.rs#price"])).unwrap(), Claimed::Granted);

    change::discard(&fx.repo, &done.id).unwrap();
    assert_eq!(claim::claim(&fx.repo, &b.id, &res(&["src/lib.rs#tax"])).unwrap(), Claimed::Granted);
}

#[test]
fn of_racing_claims_exactly_one_wins() {
    let fx = fixture();
    let spaces: Vec<_> = (0..8).map(|i| fx.workspace(&format!("agent{i}"))).collect();
    let granted: usize = std::thread::scope(|s| {
        let handles: Vec<_> = spaces
            .iter()
            .map(|ws| {
                let repo = fx.repo.clone();
                s.spawn(move || claim::claim(&repo, &ws.id, &res(&["Cargo.toml"])).unwrap() == Claimed::Granted)
            })
            .collect();
        handles.into_iter().filter(|_| true).map(|h| h.join().unwrap() as usize).sum()
    });
    assert_eq!(granted, 1);
}

#[test]
fn status_shows_what_each_workspace_is_writing_and_who_else_is() {
    let fx = fixture();
    let (a, b, c) = (fx.workspace("claude"), fx.workspace("codex"), fx.workspace("autohand"));
    write(a.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    write(b.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 4"))]);
    write(c.path(), &[("docs/b.md", "# B\n")]);
    claim::claim(&fx.repo, &c.id, &res(&["docs/b.md"])).unwrap();

    let overview = api::overview(&fx.repo).unwrap();
    let row = |id: &str| overview.workspaces.iter().find(|r| r.workspace.id == id).unwrap();
    assert_eq!(row(&a.id).writes, res(&["src/lib.rs#tax"]), "in-flight writes are derived live, at symbol granularity");
    assert!(row(&a.id).dirty);
    let overlaps: Vec<(String, &str)> =
        row(&a.id).overlaps.iter().map(|o| (o.resource.to_string(), o.with.as_str())).collect();
    assert_eq!(overlaps, [("src/lib.rs#tax".to_string(), "codex")]);
    assert!(row(&c.id).overlaps.is_empty());
    assert_eq!(row(&c.id).claims, res(&["docs/b.md"]));

    // Looking does not disturb the agent's own git state.
    assert_eq!(git(a.path(), &["status", "--porcelain"]), "M src/lib.rs");
    assert!(change::record(&fx.repo, &a.id, &Record::default()).unwrap().is_some());
}

/// Claims are voluntary. An agent that writes without claiming still
/// holds what it is writing, so a careful agent is not led into duplicating it.
#[test]
fn unclaimed_in_flight_writes_are_held_too() {
    let fx = fixture();
    let (careless, careful) = (fx.workspace("claude"), fx.workspace("codex"));
    write(careless.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let refused = refused_by(claim::claim(&fx.repo, &careful.id, &res(&["src/lib.rs#tax"])).unwrap());
    assert_eq!(refused, [("src/lib.rs#tax".to_string(), "workspace of claude".to_string())]);
    assert_eq!(claim::claim(&fx.repo, &careful.id, &res(&["src/lib.rs#price"])).unwrap(), Claimed::Granted);
}

#[test]
fn generated_files_are_never_held() {
    let derive = "[[derive]]\npath = \"registry.txt\"\nrun = \"ls items > registry.txt\"\n";
    let fx = Fixture::new(&[("zit.toml", derive), ("items/a", "a\n"), ("registry.txt", "a\n")]);
    let (a, b) = (fx.workspace("claude"), fx.workspace("codex"));
    write(a.path(), &[("registry.txt", "a\nb\n")]);
    assert_eq!(claim::claim(&fx.repo, &b.id, &res(&["registry.txt"])).unwrap(), Claimed::Granted);
    assert_eq!(claim::claim(&fx.repo, &a.id, &res(&["registry.txt"])).unwrap(), Claimed::Granted);
}

/// What open workspaces are writing is advisory, so a recent answer is reused
/// instead of snapshotting every workspace on every call.
#[test]
fn live_write_sets_are_reused_briefly_and_recomputed_after() {
    let fx = fixture();
    let ws = fx.workspace("claude");
    write(ws.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let fresh = workspace::in_flight_within(&fx.repo, &ws, std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(fresh, res(&["src/lib.rs#tax"]));

    write(ws.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 5").replace("    x\n", "    x + 1\n"))]);
    let cached = workspace::in_flight_within(&fx.repo, &ws, std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(cached, fresh, "within the window, the earlier answer");
    let recomputed = workspace::in_flight_within(&fx.repo, &ws, std::time::Duration::ZERO).unwrap();
    assert_eq!(recomputed, res(&["src/lib.rs#price", "src/lib.rs#tax"]));
}

#[test]
fn a_recorded_workspace_is_not_still_reported_as_writing() {
    let fx = fixture();
    let ws = fx.workspace("claude");
    write(ws.path(), &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    workspace::in_flight_within(&fx.repo, &ws, std::time::Duration::from_secs(60)).unwrap();
    change::record(&fx.repo, &ws.id, &Record::default()).unwrap();
    let ws = workspace::get(&fx.repo, &ws.id).unwrap();
    assert!(workspace::in_flight_within(&fx.repo, &ws, std::time::Duration::from_secs(60)).unwrap().is_empty());
}

/// Looking at work in progress must not write it into the repository's object store.
#[test]
fn watching_work_in_progress_writes_nothing_into_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let cli = common::Cli::new(dir.path(), &[("a.txt", "a\n")]);
    cli.run(&["init"]).ok();
    let path = cli.run(&["materialise", "--agent", "bot", "--intent", "x"]).ok().stdout.trim().to_string();
    std::fs::write(std::path::Path::new(&path).join("draft.txt"), "an unrecorded draft 7f3a\n").unwrap();
    cli.run(&["status"]).ok();
    let blob = common::git(&cli.root, &["hash-object", &format!("{path}/draft.txt")]);
    let stored = std::process::Command::new("git")
        .current_dir(&cli.root)
        .args(["cat-file", "-e", blob.trim()])
        .status()
        .unwrap();
    assert!(!stored.success(), "status wrote the draft into .git/objects");
}

/// Claiming a type holds its methods too, and a method's claim blocks claiming its type.
#[test]
fn a_claim_on_a_type_covers_its_methods() {
    let src = "pub struct Shape;\n\nimpl Shape {\n    pub fn area(&self) -> u32 {\n        1\n    }\n}\n";
    let fx = Fixture::new(&[("src/shape.rs", src)]);
    let (a, b) = (fx.workspace("a"), fx.workspace("b"));
    let res = |s: &str| vec![zit::resource::Resource::parse(s)];
    assert!(matches!(claim::claim(&fx.repo, &a.id, &res("src/shape.rs#Shape")).unwrap(), Claimed::Granted));
    assert!(matches!(claim::claim(&fx.repo, &b.id, &res("src/shape.rs#Shape::area")).unwrap(), Claimed::Refused(_)));
}

/// `zit claim --edit` claims exactly what an edit would change, by zit's own index.
#[test]
fn claim_for_an_edit_takes_only_the_symbols_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let cli = common::Cli::new(dir.path(), &[("src/lib.rs", LIB)]);
    cli.run(&["init"]).ok();
    let theirs = cli.run(&["materialise", "--agent", "them", "--intent", "x"]).ok().stdout.trim().to_string();
    let theirs_id = std::path::Path::new(&theirs).parent().unwrap().file_name().unwrap().to_str().unwrap().to_string();
    cli.run(&["claim", "--workspace", &theirs_id, "src/lib.rs#price"]).ok();
    let ours = cli.run(&["materialise", "--agent", "us", "--intent", "y"]).ok().stdout.trim().to_string();
    let ours_id = std::path::Path::new(&ours).parent().unwrap().file_name().unwrap().to_str().unwrap().to_string();
    let content = dir.path().join("new.rs");

    // Only `tax` changes: granted, and that is all that is claimed.
    std::fs::write(&content, LIB.replace("x / 10", "x / 5")).unwrap();
    let dry = cli
        .run(&[
            "claim",
            "--json",
            "--workspace",
            &ours_id,
            "--edit",
            "src/lib.rs",
            "--content",
            content.to_str().unwrap(),
            "--dry-run",
        ])
        .ok()
        .json();
    assert_eq!(dry["resources"], serde_json::json!(["src/lib.rs#tax"]));
    let status = cli.run(&["status", "--json"]).ok().json();
    let mine = status["workspaces"].as_array().unwrap().iter().find(|w| w["id"] == ours_id.as_str()).unwrap().clone();
    assert_eq!(mine["claims"], serde_json::json!([]), "a dry run claims nothing");
    let granted = cli
        .run(&[
            "claim",
            "--json",
            "--workspace",
            &ours_id,
            "--edit",
            "src/lib.rs",
            "--content",
            content.to_str().unwrap(),
        ])
        .ok()
        .json();
    assert_eq!(granted["claim"], "granted");
    assert_eq!(granted["resources"], serde_json::json!(["src/lib.rs#tax"]));

    // Changing `price`, which the other workspace holds: refused, naming it.
    std::fs::write(&content, LIB.replace("    x\n}", "    x + 1\n}")).unwrap();
    let refused = cli.run(&[
        "claim",
        "--json",
        "--workspace",
        &ours_id,
        "--edit",
        "src/lib.rs",
        "--content",
        content.to_str().unwrap(),
    ]);
    assert_eq!(refused.code, 1);
    let refused = refused.json();
    assert_eq!(refused["claim"], "refused");
    assert_eq!(refused["held"][0]["by"]["agent"], "them");

    // A new file is claimed whole.
    let new_file = cli
        .run(&[
            "claim",
            "--json",
            "--workspace",
            &ours_id,
            "--edit",
            "src/new.rs",
            "--content",
            content.to_str().unwrap(),
            "--dry-run",
        ])
        .ok()
        .json();
    assert_eq!(new_file["resources"], serde_json::json!(["src/new.rs"]));
}
