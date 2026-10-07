mod common;

use common::Fixture;
use ratatui::crossterm::event::KeyCode;
use ratatui::{backend::TestBackend, Terminal};
use zit::accept;
use zit::tui::{Action, App};
use zit::{api, view};

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n";

/// current <- callee (accepted); a stale caller; one open workspace.
fn scene() -> (Fixture, zit::Oid, String) {
    let fx = Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")]);
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let caller = fx.change("codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    accept::accept(&fx.repo, &callee.id).unwrap();
    let ws = fx.workspace("autohand");
    (fx, caller.id, ws.id)
}

#[test]
fn status_text_shows_current_changes_and_workspaces() {
    let (fx, caller, ws) = scene();
    let text = view::overview_text(&api::overview(&fx.repo).unwrap());
    assert!(text.contains("current"), "{text}");
    assert!(text.contains(caller.short()) && text.contains("invalid: stale") && text.contains("codex"), "{text}");
    assert!(text.contains("src/lib.rs#price"), "the reason is shown: {text}");
    assert!(text.contains(&ws) && text.contains("autohand") && text.contains("clean"), "{text}");
}

#[test]
fn detail_text_shows_writes_and_reason() {
    let (fx, caller, _) = scene();
    let text = view::detail_text(&api::detail(&fx.repo, &caller).unwrap());
    assert!(text.contains("src/shop.rs#buy"), "{text}");
    assert!(text.contains("read here, written by"), "{text}");
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    buffer.content().chunks(100).map(|row| row.iter().map(|c| c.symbol()).collect::<String>() + "\n").collect()
}

#[test]
fn the_ui_lists_changes_and_workspaces() {
    let (fx, caller, ws) = scene();
    let app = App::new(api::overview(&fx.repo).unwrap());
    let screen = screen(&app);
    assert!(screen.contains(caller.short()) && screen.contains("invalid: stale"), "{screen}");
    assert!(screen.contains(&ws) && screen.contains("autohand"), "{screen}");
    assert!(screen.contains("accept"), "key help is visible: {screen}");
}

#[test]
fn keys_act_on_the_selected_row_of_the_focused_pane() {
    let (fx, caller, ws) = scene();
    let mut app = App::new(api::overview(&fx.repo).unwrap());
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::Accept(caller.clone()));
    assert_eq!(app.on_key(KeyCode::Char('c')), Action::Check(caller.clone()));
    assert_eq!(app.on_key(KeyCode::Char('t')), Action::Retry(caller.clone()));
    assert_eq!(app.on_key(KeyCode::Char('d')), Action::Discard(caller.clone()));
    assert_eq!(app.on_key(KeyCode::Tab), Action::None);
    assert_eq!(app.on_key(KeyCode::Char('d')), Action::Dispose(ws));
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::None, "accept means nothing for a workspace");
    assert_eq!(app.on_key(KeyCode::Char('r')), Action::Refresh);
    assert_eq!(app.on_key(KeyCode::Char('q')), Action::Quit);
}

#[test]
fn selection_moves_and_stays_in_bounds() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let first = fx.change("a", &[("a.txt", "1\n")]);
    let second = fx.change("b", &[("b.txt", "2\n")]);
    let mut app = App::new(api::overview(&fx.repo).unwrap());
    let ids = [first.id, second.id];
    let listed: Vec<_> = api::overview(&fx.repo).unwrap().changes.into_iter().map(|r| r.change.id).collect();
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::Accept(listed[0].clone()));
    app.on_key(KeyCode::Down);
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::Accept(listed[1].clone()));
    app.on_key(KeyCode::Down);
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::Accept(listed[1].clone()));
    app.on_key(KeyCode::Up);
    app.on_key(KeyCode::Up);
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::Accept(listed[0].clone()));
    assert!(ids.iter().all(|id| listed.contains(id)));
}

/// In raw mode Ctrl-C arrives as a key, not a signal: it must quit, not run `c` (check).
#[test]
fn ctrl_c_quits_instead_of_running_a_check() {
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    let (fx, ..) = scene();
    let mut app = App::new(api::overview(&fx.repo).unwrap());
    assert_eq!(app.on_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Quit);
    assert!(matches!(app.on_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)), Action::Check(_)));
}

/// A multi-line message (git errors are) must not push the key help off the two-line footer.
#[test]
fn a_multi_line_message_keeps_the_key_help_visible() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let mut app = App::new(api::overview(&fx.repo).unwrap());
    app.message = "error: git merge: fatal: first line\nsecond line\nthird line".into();
    let screen = screen(&app);
    assert!(screen.contains("first line"), "{screen}");
    assert!(screen.contains("a accept"), "key help is visible: {screen}");
}

#[test]
fn an_empty_graph_renders_and_ignores_actions() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let mut app = App::new(api::overview(&fx.repo).unwrap());
    assert!(screen(&app).contains("no speculative changes"));
    assert_eq!(app.on_key(KeyCode::Char('a')), Action::None);
}
