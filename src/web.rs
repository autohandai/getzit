//! Web view: a read-only local HTTP server that draws the live graph.
//! One page, two JSON endpoints, no dependencies.

use crate::api::{self, ChangeRow, Detail, WorkspaceRow};
use crate::change::{self, Change};
use crate::footprint;
use crate::git::{Oid, Repo};
use crate::resource::Resource;
use crate::{Error, Result};
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

const PAGE: &str = include_str!("web/index.html");
/// Autohand Sans and Autohand Mono, SIL Open Font License 1.1 (web/fonts/OFL.txt).
const SANS: &[u8] = include_bytes!("web/fonts/autohand-sans.woff2");
const MONO: &[u8] = include_bytes!("web/fonts/autohand-mono.woff2");
/// How much accepted history the page draws.
const MAINLINE: usize = 60;
const MAX_DIFF_BYTES: usize = 200_000;

/// A row plus the accepted change it forked from.
#[derive(Serialize)]
struct Forked<T: Serialize> {
    #[serde(flatten)]
    row: T,
    fork: Option<Oid>,
    /// What it wrote (changes only; workspaces report theirs live).
    #[serde(skip_serializing_if = "Option::is_none")]
    wrote: Option<Vec<Resource>>,
}

#[derive(Serialize)]
struct Graph {
    repo: String,
    current: Oid,
    /// Accepted history, oldest first.
    mainline: Vec<Change>,
    changes: Vec<Forked<ChangeRow>>,
    workspaces: Vec<Forked<WorkspaceRow>>,
}

#[derive(Serialize)]
struct Inspect {
    #[serde(flatten)]
    detail: Detail,
    diff: String,
}

fn graph(repo: &Repo) -> Result<Graph> {
    let overview = api::overview(repo)?;
    let current = overview.current.id.clone();
    let mut mainline = change::accepted(repo, MAINLINE)?;
    mainline.reverse();
    let fork = |id: &Oid| repo.merge_base(id, &current).ok().flatten();
    Ok(Graph {
        repo: repo
            .git_dir()
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        changes: overview
            .changes
            .into_iter()
            .map(|row| {
                let c = &row.change;
                let wrote = c.parents.first().and_then(|p| footprint::of_change(repo, p, &c.id, &c.reads).ok());
                let wrote = wrote.map(|fp| fp.writes.into_iter().collect()).unwrap_or_default();
                Forked { fork: fork(&c.id), wrote: Some(wrote), row }
            })
            .collect(),
        workspaces: overview
            .workspaces
            .into_iter()
            .map(|row| Forked { fork: fork(&row.workspace.base), wrote: None, row })
            .collect(),
        mainline,
        current,
    })
}

fn inspect(repo: &Repo, id: &Oid) -> Result<Inspect> {
    let detail = api::detail(repo, id)?;
    let mut diff = match detail.change.parents.first() {
        Some(parent) => repo.git(&["diff", "--no-color", parent.as_str(), id.as_str()])?,
        None => repo.git(&["show", "--no-color", "--format=", id.as_str()])?,
    };
    if diff.len() > MAX_DIFF_BYTES {
        let cut = (0..=MAX_DIFF_BYTES).rev().find(|&i| diff.is_char_boundary(i)).unwrap_or(0);
        diff.truncate(cut);
        diff.push_str("\n… diff truncated");
    }
    Ok(Inspect { detail, diff })
}

pub struct Server {
    listener: TcpListener,
    repo: Repo,
}

impl Server {
    /// Listen on localhost. If `port` is taken, any free port is used instead.
    pub fn bind(repo: Repo, port: u16) -> Result<Server> {
        repo.current()?;
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))?;
        Ok(Server { listener, repo })
    }

    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener has an address")
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr())
    }

    /// Serve until the process ends.
    pub fn run(self) {
        for stream in self.listener.incoming().flatten() {
            let repo = self.repo.clone();
            std::thread::spawn(move || {
                let _ = handle(&repo, stream);
            });
        }
    }
}

struct Reply {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

fn text(status: &'static str, body: &str) -> Reply {
    Reply { status, content_type: "text/plain; charset=utf-8", body: body.as_bytes().to_vec() }
}

fn font(body: &[u8]) -> Reply {
    Reply { status: "200 OK", content_type: "font/woff2", body: body.to_vec() }
}

fn json<T: Serialize>(value: Result<T>) -> Reply {
    match value.and_then(|v| Ok(serde_json::to_vec(&v)?)) {
        Ok(body) => Reply { status: "200 OK", content_type: "application/json", body },
        Err(Error::UnknownRevision(_)) => text("404 Not Found", "no such change"),
        Err(e) => text("500 Internal Server Error", &e.to_string()),
    }
}

/// The page is for this machine only: a foreign `Host` means another site
/// is trying to read the graph through the browser.
fn is_local(host: &str) -> bool {
    let name = host
        .rsplit_once(':')
        .map_or(host, |(name, port)| if port.bytes().all(|b| b.is_ascii_digit()) { name } else { host });
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

fn route(repo: &Repo, method: &str, path: &str, host: &str) -> Reply {
    if !is_local(host) {
        return text("403 Forbidden", "zit web only answers to localhost");
    }
    if method != "GET" {
        return text("405 Method Not Allowed", "the web view is read-only");
    }
    match path.split('?').next().unwrap_or(path) {
        "/" => Reply { status: "200 OK", content_type: "text/html; charset=utf-8", body: PAGE.as_bytes().to_vec() },
        "/api/graph" => json(graph(repo)),
        "/fonts/autohand-sans.woff2" => font(SANS),
        "/fonts/autohand-mono.woff2" => font(MONO),
        other => match other.strip_prefix("/api/change/") {
            Some(id) if (7..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                json(repo.resolve(id).and_then(|id| inspect(repo, &id)))
            }
            _ => text("404 Not Found", "not found"),
        },
    }
}

fn handle(repo: &Repo, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut host = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("host") {
                host = value.trim().to_string();
            }
        }
    }
    let mut parts = request.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let reply = route(repo, method, path, &host);
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    )?;
    stream.write_all(&reply.body)
}

/// Open `url` in the default browser. Best effort.
pub fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
