//! The browser frontend's server: the embedded page and a JSON API over the
//! deployer's state, on the loopback interface only.
//!
//! A deployer holds SSH and appliance passwords and can erase a Raspberry
//! Pi, so being on loopback is not taken as being trusted. Every API call
//! must carry the session token the launch URL handed the page; the Host
//! header must name this server, so a DNS-rebound hostname is refused; a
//! cross-origin request is refused; actions accept only JSON bodies; and no
//! secret is ever sent back.

use std::fmt::Write as _;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use omt_common::json::{self, Value, write_string};
use omt_deploy::capsule::Capsule;

use crate::about;
use crate::app::{App, FIELDS, Kind, Slot, VIEWS};
use crate::fs;
use crate::http::{self, Request, Response};

/// Concurrent connections served; more are closed unanswered. A browser
/// opens a handful, and each lives for one request.
pub const MAX_CONNECTIONS: usize = 16;
/// How long a connection may take to send its request or read the answer.
pub const IO_TIMEOUT_MS: u64 = 10_000;
/// How often the accept loop drains the worker and checks for quit.
const TICK_MS: u64 = 50;
const CONNECTION_STACK: usize = 256 * 1024;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const DEPLOYER_CSS: &str = include_str!("../web/deployer.css");
/// The appliance Web GUI's own design system, so the two look like one
/// product and cannot drift apart.
const STYLE_CSS: &str = include_str!("../../omt-web/static/style.css");
const FAVICON_SVG: &str = include_str!("../../omt-web/static/favicon.svg");

pub struct Shared {
    app: Mutex<App>,
    token: String,
    port: u16,
    quit: AtomicBool,
    connections: AtomicUsize,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a credential.
        f.debug_struct("Shared").field("port", &self.port).finish_non_exhaustive()
    }
}

impl Shared {
    pub fn new(capsule: &'static Capsule, token: String, port: u16) -> Self {
        Self {
            app: Mutex::new(App::new(capsule)),
            token,
            port,
            quit: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
        }
    }

    pub fn app(&self) -> MutexGuard<'_, App> {
        self.app.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The URL that opens the page: the token rides in the fragment, which
    /// a browser never sends to a server or puts in a Referer.
    pub fn launch_url(&self) -> String {
        format!("http://127.0.0.1:{}/#t={}", self.port, self.token)
    }

    pub fn quitting(&self) -> bool {
        self.quit.load(Ordering::Relaxed) || self.app().should_quit
    }

    /// Answers one request.
    pub fn handle(&self, r: &Request) -> Response {
        if !self.host_allowed(r) {
            return Response::error(403, "This deployer only answers on 127.0.0.1 or localhost.");
        }
        if let Some(origin) = r.header("origin")
            && !self.origin_allowed(origin)
        {
            return Response::error(403, "Cross-origin requests are refused.");
        }
        let api = r.path.starts_with("/api/");
        if !api {
            return Self::asset(r);
        }
        if !self.token_matches(r.header("x-deploy-token").unwrap_or("")) {
            return Response::error(403, "Missing or wrong session token; reopen the launch URL.");
        }
        match r.method.as_str() {
            "GET" => self.get(r),
            "POST" => {
                let json = r
                    .header("content-type")
                    .and_then(|t| t.split(';').next())
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"));
                if !json {
                    return Response::error(415, "Actions take application/json.");
                }
                let mut body = match json::parse(&r.body) {
                    Ok(v) => v,
                    Err(e) => return Response::error(400, e.message()),
                };
                let response = self.post(&r.path, &body);
                // The body may have carried passwords.
                body.wipe();
                response
            }
            _ => Response::error(405, "Method not allowed."),
        }
    }

    fn host_allowed(&self, r: &Request) -> bool {
        let Some(host) = r.header("host") else { return false };
        let port = self.port.to_string();
        ["127.0.0.1", "localhost"].iter().any(|h| {
            host.strip_prefix(h).and_then(|rest| rest.strip_prefix(':')) == Some(port.as_str())
        })
    }

    fn origin_allowed(&self, origin: &str) -> bool {
        let port = self.port;
        origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
    }

    /// Compares in time independent of where the first difference is.
    fn token_matches(&self, given: &str) -> bool {
        let (a, b) = (self.token.as_bytes(), given.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    fn asset(r: &Request) -> Response {
        if r.method != "GET" {
            return Response::error(405, "Method not allowed.");
        }
        match r.path.as_str() {
            "/" => Response::new(200, "text/html; charset=utf-8", INDEX_HTML),
            "/app.js" => Response::new(200, "text/javascript; charset=utf-8", APP_JS),
            "/deployer.css" => Response::new(200, "text/css; charset=utf-8", DEPLOYER_CSS),
            "/style.css" => Response::new(200, "text/css; charset=utf-8", STYLE_CSS),
            "/favicon.svg" => Response::new(200, "image/svg+xml", FAVICON_SVG),
            _ => Response::error(404, "Not found."),
        }
    }

    fn get(&self, r: &Request) -> Response {
        match r.path.as_str() {
            "/api/schema" => Response::json(schema_json()),
            "/api/state" => {
                let since = r.param("since").and_then(|s| s.parse().ok()).unwrap_or(0);
                let mut app = self.app();
                app.poll_worker();
                Response::json(state_json(&app, Some(since)))
            }
            "/api/about" => {
                let capsule = self.app().capsule;
                let mut out = String::from("{\"summary\":");
                write_string(&mut out, &about::summary(capsule));
                out.push_str(",\"license\":");
                write_string(&mut out, about::license(capsule));
                out.push_str(",\"notices\":");
                write_string(&mut out, about::notices(capsule));
                out.push('}');
                Response::json(out)
            }
            "/api/fs/roots" => Response::json(fs::roots_json(&fs::roots())),
            "/api/fs/list" => {
                let path = r.param("path").unwrap_or_default();
                let files = r.param("files").as_deref() == Some("1");
                let hidden = r.param("hidden").as_deref() == Some("1");
                match fs::list(&path, files, hidden) {
                    Ok(l) => Response::json(fs::listing_json(&l)),
                    Err(e) => Response::error(404, &e),
                }
            }
            _ => Response::error(404, "Not found."),
        }
    }

    fn post(&self, path: &str, body: &Value) -> Response {
        let mut app = self.app();
        let outcome = match path {
            "/api/fields" => set_fields(&mut app, body),
            "/api/press" => {
                if !body.only_keys(&["slot"]) {
                    Err("Unknown key.")
                } else if app.confirm.is_some() {
                    Err("Answer the open confirmation first.")
                } else {
                    match body.get("slot").and_then(Value::as_str).and_then(Slot::from_name) {
                        Some(s) if s.job().is_some() => {
                            app.press(s);
                            Ok(())
                        }
                        _ => Err("Unknown action."),
                    }
                }
            }
            "/api/confirm" => match body.get("accept").and_then(Value::as_bool) {
                Some(accept) if body.only_keys(&["accept"]) => {
                    app.confirm_pending(accept);
                    Ok(())
                }
                _ => Err("Expected {\"accept\": true|false}."),
            },
            "/api/cancel" => {
                app.cancel_job();
                Ok(())
            }
            "/api/quit" => {
                app.request_quit();
                Ok(())
            }
            _ => return Response::error(404, "Not found."),
        };
        app.poll_worker();
        match outcome {
            // The log is the poll's to deliver: an action's answer carrying it
            // would skip the lines the page has not fetched yet.
            Ok(()) => Response::json(state_json(&app, None)),
            Err(problem) => Response::error(400, problem),
        }
    }
}

/// Applies `{"fields": {name: text}, "toggles": {slot: bool}}`. Every name
/// is checked before anything changes, so a refused request changes
/// nothing.
fn set_fields(app: &mut App, body: &Value) -> Result<(), &'static str> {
    if !body.only_keys(&["fields", "toggles"]) {
        return Err("Unknown key.");
    }
    if app.busy() {
        return Err("A job is running; its settings cannot change now.");
    }
    let fields = body.get("fields").map_or(Some(&[][..]), Value::as_object).ok_or("Bad fields.")?;
    let toggles =
        body.get("toggles").map_or(Some(&[][..]), Value::as_object).ok_or("Bad toggles.")?;
    for (name, value) in fields {
        let text = value.as_str().ok_or("A field value must be a string.")?;
        crate::app::Field::from_name(name).ok_or("Unknown field.")?;
        if text.len() > crate::app::FIELD_LIMIT || text.chars().any(char::is_control) {
            return Err("A value is too long or holds a control character.");
        }
    }
    for (name, value) in toggles {
        value.as_bool().ok_or("A toggle must be true or false.")?;
        Slot::from_name(name).filter(|s| s.kind() == Kind::Toggle).ok_or("Unknown toggle.")?;
    }
    for (name, value) in fields {
        if let (Some(f), Some(text)) = (crate::app::Field::from_name(name), value.as_str()) {
            app.set_field(f, text);
        }
    }
    for (name, value) in toggles {
        if let (Some(s), Some(on)) = (Slot::from_name(name), value.as_bool()) {
            app.set_toggle(s, on);
        }
    }
    Ok(())
}

/// The views and their rows, from the same tables the state model uses.
pub fn schema_json() -> String {
    let mut out = String::from("{\"views\":[");
    for (i, v) in VIEWS.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"title\":");
        write_string(&mut out, v.title());
        out.push_str(",\"slots\":[");
        for (j, s) in v.slots().iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            let kind = match s.kind() {
                Kind::Text => "text",
                Kind::Secret => "secret",
                Kind::Toggle => "toggle",
                Kind::Button => "button",
            };
            out.push_str("{\"name\":");
            write_string(&mut out, s.name());
            out.push_str(",\"label\":");
            write_string(&mut out, s.label());
            let _ = write!(out, ",\"kind\":\"{kind}\",\"field\":");
            match s.field() {
                Some(f) => write_string(&mut out, f.name()),
                None => out.push_str("null"),
            }
            let confirm = s.job().is_some_and(|(_, _, prompt)| prompt.is_some());
            let _ = write!(out, ",\"confirm\":{confirm}}}");
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

/// Everything the page shows, with the log from `since` on when given.
/// Secret values never leave: a secret is reported only as set or empty.
pub fn state_json(app: &App, since: Option<u64>) -> String {
    let mut out = String::with_capacity(4096);
    let _ = write!(out, "{{\"busy\":{},\"status\":", app.busy());
    write_string(&mut out, &app.status);
    out.push_str(",\"cancelling\":");
    out.push_str(if app.job.as_ref().is_some_and(crate::app::Worker::cancel_requested) {
        "true"
    } else {
        "false"
    });
    out.push_str(",\"quit\":");
    out.push_str(if app.should_quit { "true" } else { "false" });
    out.push_str(",\"confirm\":");
    match &app.confirm {
        Some(p) => write_string(&mut out, &p.prompt),
        None => out.push_str("null"),
    }
    out.push_str(",\"fields\":{");
    let mut first = true;
    for f in FIELDS {
        if !first {
            out.push(',');
        }
        first = false;
        write_string(&mut out, f.name());
        out.push(':');
        if f.is_secret() {
            let _ = write!(out, "{{\"set\":{}}}", !app.field(f).is_empty());
        } else {
            write_string(&mut out, app.field(f));
        }
    }
    out.push_str("},\"toggles\":{");
    let mut first = true;
    let mut blocked = String::from("{");
    for s in VIEWS.iter().flat_map(|v| v.slots()) {
        match s.kind() {
            Kind::Toggle => {
                if !first {
                    out.push(',');
                }
                first = false;
                write_string(&mut out, s.name());
                let _ = write!(out, ":{}", app.toggle(*s));
            }
            Kind::Button => {
                if blocked.len() > 1 {
                    blocked.push(',');
                }
                write_string(&mut blocked, s.name());
                blocked.push(':');
                // Nothing is worth re-checking while a job runs.
                match (!app.busy()).then(|| app.blocked(*s)).flatten() {
                    Some(problem) => write_string(&mut blocked, problem),
                    None => blocked.push_str("null"),
                }
            }
            _ => {}
        }
    }
    blocked.push('}');
    out.push_str("},\"blocked\":");
    out.push_str(&blocked);
    if let Some(since) = since {
        let (first_seq, lines) = app.log_since(since);
        let _ = write!(out, ",\"log_start\":{first_seq},\"log_end\":{},\"log\":[", app.log_end);
        for (i, line) in lines.enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_string(&mut out, line);
        }
        out.push(']');
    }
    out.push('}');
    out
}

fn serve_connection(shared: &Shared, mut stream: TcpStream) {
    let timeout = Some(Duration::from_millis(IO_TIMEOUT_MS));
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let response = match http::read_request(&mut stream) {
        Ok(mut r) => {
            let response = shared.handle(&r);
            omt_common::buf::wipe_vec(&mut r.body);
            response
        }
        Err(status) => Response::error(status, http::reason(status)),
    };
    let _ = http::write_response(&mut stream, &response);
    let _ = stream.shutdown(Shutdown::Write);
}

/// Serves until the operator quits. The listener must already be bound to
/// a loopback address.
pub fn serve(listener: &TcpListener, shared: &Arc<Shared>) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    while !shared.quitting() {
        // Progress keeps draining even while no page is watching, so the
        // worker's buffer never grows past one tick's output.
        shared.app().poll_worker();
        match listener.accept() {
            Ok((stream, _)) => {
                if shared.connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    shared.connections.fetch_sub(1, Ordering::AcqRel);
                    drop(stream);
                    continue;
                }
                let worker = Arc::clone(shared);
                let spawned = thread::Builder::new()
                    .name("deploy-http".into())
                    .stack_size(CONNECTION_STACK)
                    .spawn(move || {
                        serve_connection(&worker, stream);
                        worker.connections.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    shared.connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(TICK_MS));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    // Let the page's last request -- the quit itself -- be answered.
    for _ in 0..40 {
        if shared.connections.load(Ordering::Acquire) == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(TICK_MS));
    }
    Ok(())
}
