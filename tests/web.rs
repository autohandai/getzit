mod common;

use common::Fixture;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use zit::accept;
use zit::web::Server;

const LIB: &str = "pub fn price(x: u32) -> u32 {\n    x\n}\n\npub fn tax(x: u32) -> u32 {\n    x / 10\n}\n";

/// current <- callee <- compose(other); a stale caller; one open workspace.
fn scene() -> (Fixture, SocketAddr, zit::Oid, zit::Oid) {
    let fx = Fixture::new(&[("src/lib.rs", LIB), ("src/shop.rs", "pub fn buy() {}\n")]);
    let callee = fx.change("claude", &[("src/lib.rs", &LIB.replace("price(x: u32)", "price(x: u32, t: u32)"))]);
    let other = fx.change("autohand", &[("src/lib.rs", &LIB.replace("x / 10", "x / 5"))]);
    let caller = fx.change("codex", &[("src/shop.rs", "pub fn buy() { lib::price(3); }\n")]);
    accept::accept(&fx.repo, &callee.id).unwrap();
    accept::accept(&fx.repo, &other.id).unwrap();
    fx.workspace("autohand");

    let server = Server::bind(fx.repo.clone(), 0).unwrap();
    let addr = server.addr();
    std::thread::spawn(move || server.run());
    (fx, addr, callee.id, caller.id)
}

fn request(addr: SocketAddr, raw: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, head.to_string(), body.to_string())
}

fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
    request(addr, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", addr.port()))
}

#[test]
fn the_page_is_served_at_the_root() {
    let (_fx, addr, ..) = scene();
    let (code, head, body) = get(addr, "/");
    assert_eq!(code, 200);
    assert!(head.to_lowercase().contains("content-type: text/html"), "{head}");
    assert!(body.contains("<svg") && body.contains("/api/graph"), "the page draws a graph from the API");
}

#[test]
fn the_graph_has_the_accepted_line_the_speculative_changes_and_the_workspaces() {
    let (fx, addr, callee, caller) = scene();
    let (code, head, body) = get(addr, "/api/graph");
    assert_eq!(code, 200);
    assert!(head.to_lowercase().contains("content-type: application/json"), "{head}");
    let graph: Value = serde_json::from_str(&body).unwrap();

    assert_eq!(graph["current"], fx.repo.current().unwrap().as_str());
    let line: Vec<&Value> = graph["mainline"].as_array().unwrap().iter().collect();
    assert_eq!(line.len(), 3, "genesis, callee, compose: {line:?}");
    assert_eq!(line[0]["intent"], "genesis");
    assert_eq!(line[1]["id"], callee.as_str());
    assert_eq!(line[2]["parents"].as_array().unwrap().len(), 2, "the compose change keeps both parents");

    let changes = graph["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["id"], caller.as_str());
    assert_eq!(changes[0]["status"], "invalid");
    assert_eq!(changes[0]["reason"], "stale");
    assert_eq!(changes[0]["detail"][0]["by"], callee.as_str(), "the edge to draw: who invalidated it");
    assert_eq!(changes[0]["fork"], line[0]["id"], "where it left the accepted line");
    assert_eq!(changes[0]["wrote"], serde_json::json!(["src/shop.rs#buy"]));

    let workspaces = graph["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["agent"], "autohand");
    assert_eq!(workspaces[0]["fork"], graph["current"]);
    assert_eq!(workspaces[0]["writes"], serde_json::json!([]));
    assert_eq!(workspaces[0]["overlaps"], serde_json::json!([]));
}

#[test]
fn a_change_can_be_inspected_with_its_diff() {
    let (_fx, addr, _, caller) = scene();
    let (code, _, body) = get(addr, &format!("/api/change/{caller}"));
    assert_eq!(code, 200);
    let detail: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(detail["writes"], serde_json::json!(["src/shop.rs#buy"]));
    assert!(detail["diff"].as_str().unwrap().contains("+pub fn buy() { lib::price(3); }"), "{detail}");
}

#[test]
fn unknown_things_are_404_and_bad_ids_do_not_reach_git() {
    let (_fx, addr, ..) = scene();
    assert_eq!(get(addr, "/nope").0, 404);
    assert_eq!(get(addr, "/api/change/--help").0, 404);
    assert_eq!(get(addr, "/api/change/0123456789012345678901234567890123456789").0, 404);
}

#[test]
fn only_local_hosts_and_reads_are_served() {
    let (_fx, addr, ..) = scene();
    let evil = request(addr, "GET /api/graph HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n");
    assert_eq!(evil.0, 403, "a rebinding page cannot read the graph");
    let post = request(
        addr,
        &format!("POST /api/graph HTTP/1.1\r\nHost: localhost:{}\r\nContent-Length: 0\r\n\r\n", addr.port()),
    );
    assert_eq!(post.0, 405);
}

#[test]
fn the_page_ships_its_own_fonts() {
    let (_fx, addr, ..) = scene();
    let (_, _, page) = get(addr, "/");
    for font in ["/fonts/autohand-sans.woff2", "/fonts/autohand-mono.woff2"] {
        assert!(page.contains(font), "the page uses {font}");
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(stream, "GET {font} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        let head =
            String::from_utf8_lossy(&reply[..reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap()]).to_lowercase();
        assert!(head.starts_with("http/1.1 200") && head.contains("content-type: font/woff2"), "{head}");
        let body = &reply[head.len() + 4..];
        assert_eq!(&body[..4], b"wOF2", "a WOFF2 file");
    }
}

#[test]
fn the_page_has_a_light_and_a_dark_theme() {
    let (_fx, addr, ..) = scene();
    let (_, _, page) = get(addr, "/");
    assert!(page.contains("prefers-color-scheme: dark"));
    assert!(page.contains("#181818"), "the dark background");
    assert!(page.contains("data-theme"), "a manual override");
}
