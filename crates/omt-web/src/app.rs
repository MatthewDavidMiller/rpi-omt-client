//! The Web frontend's routes. Every page but the login and the static assets
//! needs a current session; every POST also needs the session-scoped CSRF
//! token. Security headers are applied to every response.

use omt_common::timefmt::now_ms;
use omt_common::{Result, fsio, rand};

use crate::auth::{Auth, SESSION_ID_HEX};
use crate::diagnostics::{self, DiagnosticResult};
use crate::http::{Handler, Request, Response};
use crate::network;
use crate::playback::{ActionResult, Playback, VideoLimit, clip};
use crate::settings::{RateLimit, Settings};
use crate::templates_gen;
use crate::tv::Value;

pub const RATE_KEYS: usize = 4096;
pub const MAX_FLASHES: usize = 64;
pub const FORM_MAX: usize = 32;

const SESSION_COOKIE: &str = "__Host-omt_session";
const LOGIN_COOKIE: &str = "__Host-omt_login";
const FLASH_COOKIE: &str = "__Host-omt_flash";
const COOKIE_ATTRIBUTES: &str = "Path=/; Secure; HttpOnly; SameSite=Lax";
const CSP: &str = "default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'";
/// The longest Cookie header read, and the longest value kept for each
/// cookie: the sizes the C frontend's buffers held.
const COOKIE_HEADER_MAX: usize = 8191;
const SESSION_COOKIE_MAX: usize = 127;
const FLASH_COOKIE_MAX: usize = 63;

// ------------------------------------------------------------------ form

/// application/x-www-form-urlencoded, decoded. Duplicate keys keep the last
/// value; any pair that decodes to invalid UTF-8 empties the whole form, as
/// serde_urlencoded's error did.
#[derive(Debug, Default)]
pub struct Form(Vec<(String, String)>);

/// Percent-decoding as the WHATWG urlencoded parser does it: '+' is a
/// space, a well-formed %XX is a byte, and a malformed escape is kept
/// literally.
fn decode(s: &[u8]) -> Option<String> {
    let hex = |c: u8| char::from(c).to_digit(16);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < s.len() => match (hex(s[i + 1]), hex(s[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(u8::try_from(hi << 4 | lo).unwrap_or(0));
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

impl Form {
    pub fn parse(body: &[u8]) -> Self {
        let mut out: Vec<(String, String)> = Vec::new();
        for pair in body.split(|&c| c == b'&').filter(|p| !p.is_empty()) {
            let (key, value) = match pair.iter().position(|&c| c == b'=') {
                Some(eq) => (&pair[..eq], &pair[eq + 1..]),
                None => (pair, &b""[..]),
            };
            let (Some(key), Some(value)) = (decode(key), decode(value)) else {
                return Self::default();
            };
            if let Some(entry) = out.iter_mut().find(|(k, _)| *k == key) {
                entry.1 = value;
            } else if out.len() < FORM_MAX {
                out.push((key, value));
            }
        }
        Self(out)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn field(&self, key: &str) -> &str {
        self.get(key).unwrap_or("")
    }
}

// --------------------------------------------------------------- cookies

/// The first non-empty value of cookie `name`; `None` as well when that
/// value is longer than `max` bytes.
pub fn cookie_value(header: &str, name: &str, max: usize) -> Option<String> {
    for part in header.split(';') {
        let Some((key, value)) = part.trim().split_once('=') else { continue };
        if key == name && !value.is_empty() {
            return (value.len() <= max).then(|| value.to_owned());
        }
    }
    None
}

fn request_cookie(req: &Request<'_>, name: &str, max: usize) -> Option<String> {
    let header = req.header("cookie").filter(|h| h.len() <= COOKIE_HEADER_MAX)?;
    cookie_value(&String::from_utf8_lossy(header), name, max)
}

fn set_cookie(res: &mut Response, name: &str, value: &str, max_age: u64) {
    res.add_header(
        "set-cookie",
        &format!("{name}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Lax"),
    );
}

fn clear_cookie(res: &mut Response, name: &str) {
    res.add_header("set-cookie", &format!("{name}=; Path=/; Max-Age=0; {COOKIE_ATTRIBUTES}"));
}

// ------------------------------------------------------------- app state

#[derive(Debug)]
struct RateEntry {
    scope: String,
    peer: String,
    /// Expiry times, oldest first.
    expiries: Vec<u64>,
}

#[derive(Debug)]
struct Flash {
    key: String,
    category: &'static str,
    message: String,
}

#[derive(Debug)]
pub struct App {
    settings: Settings,
    auth: Auth,
    playback: Playback,
    hostname: String,
    rates: Vec<RateEntry>,
    flashes: Vec<Flash>,
}

/// A sliding-window limiter over (scope, peer) keys.
#[derive(Debug, Default)]
pub struct RateLimiter(Vec<RateEntry>);

fn allow(rates: &mut Vec<RateEntry>, scope: &str, peer: &str, limit: RateLimit) -> bool {
    let now = now_ms();
    // Expire across every key, and forget keys with nothing left.
    rates.retain_mut(|e| {
        e.expiries.retain(|&t| t > now);
        !e.expiries.is_empty()
    });
    let index = match rates.iter().position(|e| e.scope == scope && e.peer == peer) {
        Some(i) => i,
        // Fail closed when the table is full: an attacker spraying
        // addresses cannot buy an unthrottled attempt by evicting someone.
        None if rates.len() >= RATE_KEYS => return false,
        None => {
            rates.push(RateEntry {
                scope: scope.to_owned(),
                peer: peer.to_owned(),
                expiries: Vec::new(),
            });
            rates.len() - 1
        }
    };
    let entry = &mut rates[index];
    if entry.expiries.len() >= limit.count {
        return false;
    }
    entry.expiries.push(now + limit.window_ms);
    true
}

impl RateLimiter {
    pub fn allow(&mut self, scope: &str, peer: &str, limit: RateLimit) -> bool {
        allow(&mut self.0, scope, peer, limit)
    }

    pub fn keys(&self) -> usize {
        self.0.len()
    }
}

impl App {
    pub fn new(settings: &Settings) -> Result<Self> {
        let hostname = match fsio::read_text("/etc/hostname", 255) {
            Ok(Some(h)) => h.trim().to_owned(),
            _ => "omt-client".to_owned(),
        };
        Ok(Self {
            auth: Auth::load(settings)?,
            playback: Playback::new(settings),
            settings: settings.clone(),
            hostname,
            rates: Vec::new(),
            flashes: Vec::new(),
        })
    }
}

impl Handler for App {
    fn handle(&mut self, req: &Request<'_>, res: &mut Response) {
        let has_flash_cookie = request_cookie(req, FLASH_COOKIE, FLASH_COOKIE_MAX).is_some();
        let get = req.method == "GET" || req.method == "HEAD";
        let post = req.method == "POST";
        let is_static = req.path.starts_with("/static/");
        if is_static
            && get
            && (req.path == "/static/style.css" || req.path == "/static/favicon.svg")
        {
            if let Some((data, kind)) = templates_gen::static_asset(&req.path[8..]) {
                res.status = 200;
                res.add_header("content-type", kind);
                res.body.extend_from_slice(data.as_bytes());
            }
        } else {
            let mut r = Ctx { app: self, req, res, session: String::new(), has_flash_cookie };
            match ROUTES.iter().find(|route| route.path == req.path) {
                None => not_found(&mut r),
                Some(route) => match (get, post, route.get, route.post) {
                    (true, _, Some(f), _) | (_, true, _, Some(f)) => f(&mut r),
                    _ => {
                        // A known path with the wrong method, as axum
                        // answered it.
                        res.status = 405;
                        let allow = match (route.get.is_some(), route.post.is_some()) {
                            (true, true) => "GET,HEAD,POST",
                            (true, false) => "GET,HEAD",
                            _ => "POST",
                        };
                        res.add_header("allow", allow);
                    }
                },
            }
        }
        security_headers(res, is_static);
    }
}

fn security_headers(res: &mut Response, is_static: bool) {
    res.add_header("strict-transport-security", "max-age=31536000; includeSubDomains");
    res.add_header("x-frame-options", "DENY");
    res.add_header("x-content-type-options", "nosniff");
    res.add_header("referrer-policy", "strict-origin-when-cross-origin");
    res.add_header("content-security-policy", CSP);
    if is_static {
        res.add_header("cache-control", "public, max-age=86400");
    } else {
        res.add_header("cache-control", "no-store");
        res.add_header("pragma", "no-cache");
    }
}

// ------------------------------------------------------------- responses

struct Ctx<'a, 'r> {
    app: &'a mut App,
    req: &'a Request<'r>,
    res: &'a mut Response,
    session: String,
    has_flash_cookie: bool,
}

fn redirect(res: &mut Response, to: &str) {
    res.status = 303;
    res.add_header("location", to);
}

impl Ctx<'_, '_> {
    /// The context every page shares; consumes the request's flash.
    fn common(&mut self, authenticated: bool, csrf: &str, endpoint: &str) -> Value {
        let mut flashes = Value::List(Vec::new());
        if let Some(key) = request_cookie(self.req, FLASH_COOKIE, FLASH_COOKIE_MAX) {
            let list = &mut self.app.flashes;
            if let Some(i) = list.iter().position(|f| f.key == key) {
                let f = list.swap_remove(i);
                flashes.push(Value::List(vec![Value::str(f.category), Value::str(f.message)]));
            }
        }
        let mut ctx = Value::map();
        ctx.set_str("hostname", self.app.hostname.as_str());
        ctx.set_bool("authenticated", authenticated);
        ctx.set_str("csrf_token", csrf);
        ctx.set_str("endpoint", endpoint);
        ctx.set("flashes", flashes);
        ctx
    }

    fn render(&mut self, template: &str, context: &Value, status: u16) {
        let mut body = String::new();
        if !templates_gen::render(template, context, &mut body) {
            eprintln!("template error: unable to render {template}");
            self.res.status = 500;
            self.res.add_header("content-type", "text/plain; charset=utf-8");
            self.res.body = b"Internal Server Error".to_vec();
            return;
        }
        self.res.body = body.into_bytes();
        self.res.status = status;
        self.res.add_header("content-type", "text/html; charset=utf-8");
        if self.has_flash_cookie {
            clear_cookie(self.res, FLASH_COOKIE);
        }
    }

    fn session_token(&self) -> String {
        self.app.auth.csrf_token("session", &self.session).unwrap_or_default()
    }

    fn flash_redirect(&mut self, to: &str, result: &ActionResult) {
        let Ok(key) = rand::random_hex(16) else {
            redirect(self.res, to);
            return;
        };
        // Bounded: past the cap the lexically first key gives way, as the
        // Rust BTreeMap did.
        let list = &mut self.app.flashes;
        while list.len() >= MAX_FLASHES {
            let first = (0..list.len()).min_by(|&a, &b| list[a].key.cmp(&list[b].key)).unwrap_or(0);
            list.swap_remove(first);
        }
        let (category, message) =
            if result.ok { ("success", &result.message) } else { ("error", &result.error) };
        list.push(Flash { key: key.clone(), category, message: message.clone() });
        redirect(self.res, to);
        set_cookie(self.res, FLASH_COOKIE, &key, 60);
    }

    fn login_page(&mut self, error: &str, status: u16) {
        let nonce = rand::random_hex(24).unwrap_or_default();
        let token = self.app.auth.csrf_token("login", &nonce).unwrap_or_default();
        let mut ctx = self.common(false, &token, "auth.login");
        ctx.set_str("error", error);
        self.render("login.html", &ctx, status);
        set_cookie(self.res, LOGIN_COOKIE, &nonce, 600);
    }

    fn error_page(&mut self, title: &str, message: &str, status: u16) {
        let token = self.session_token();
        let mut ctx = self.common(true, &token, "error");
        ctx.set_str("title", title);
        ctx.set_str("message", message);
        self.render("error.html", &ctx, status);
    }

    fn too_many(&mut self) {
        self.error_page("Too many requests", "Too many requests. Please wait and try again.", 429);
    }

    fn current_session(&mut self) -> bool {
        let Some(id) = request_cookie(self.req, SESSION_COOKIE, SESSION_COOKIE_MAX) else {
            return false;
        };
        if id.len() != SESSION_ID_HEX || !self.app.auth.is_current(&id) {
            return false;
        }
        self.session = id;
        true
    }

    fn require_session(&mut self) -> bool {
        if self.current_session() {
            return true;
        }
        redirect(self.res, "/login");
        false
    }

    /// The authenticated-POST preamble: a current session and its CSRF
    /// token.
    fn authenticated_post(&mut self) -> Option<Form> {
        let form = Form::parse(self.req.body);
        if !self.require_session() {
            return None;
        }
        let valid = form
            .get("csrf_token")
            .is_some_and(|t| self.app.auth.verify_csrf("session", &self.session, t));
        if !valid {
            let csrf = self.session_token();
            let mut ctx = self.common(true, &csrf, "error");
            ctx.set_str("title", "Session expired");
            ctx.set_str("message", "Session expired. Please try again.");
            self.render("error.html", &ctx, 400);
            return None;
        }
        Some(form)
    }

    fn allow(&mut self, scope: &str, limit: RateLimit) -> bool {
        allow(&mut self.app.rates, scope, self.req.peer, limit)
    }
}

// ------------------------------------------------------------- contexts

fn video_limit_value(v: VideoLimit) -> Value {
    let mut m = Value::map();
    m.set_str("board_label", v.board_label);
    m.set_str("effective", v.effective);
    m.set_str("board_default", v.board_default);
    m.set_str("error", v.error);
    m.set_str("effective_description", v.effective_description);
    m.set_str("board_default_description", v.board_default_description);
    m.set_bool("overridden", v.overridden);
    m.set_bool("above_board_default", v.above_board_default);
    m
}

fn command_value(d: &DiagnosticResult) -> Value {
    let c = &d.command;
    let mut m = Value::map();
    m.set_str("command", c.command.as_str());
    m.set("returncode", c.returncode.map_or(Value::None, |r| Value::Int(i64::from(r))));
    m.set_str("stdout", c.stdout.as_str());
    m.set_str("stderr", c.stderr.as_str());
    m.set("duration_seconds", Value::Float(c.duration_seconds));
    m.set_bool("timed_out", c.timed_out);
    m.set_str("error", c.error.as_str());
    m.set_bool("skipped", d.skipped);
    m.set_bool("stdout_truncated", c.stdout_truncated);
    m.set_bool("stderr_truncated", c.stderr_truncated);
    m.set("sources", Value::List(d.sources.iter().map(|s| Value::str(s.as_str())).collect()));
    m
}

// -------------------------------------------------------------- handlers

fn dashboard(r: &mut Ctx<'_, '_>) {
    if !r.require_session() {
        return;
    }
    let token = r.session_token();
    let summary = r.app.playback.summary();
    let mut ctx = r.common(true, &token, "dashboard.dashboard");
    let sources = r
        .app
        .playback
        .sources()
        .into_iter()
        .map(|name| {
            let mut s = Value::map();
            s.set_str("selection_value", format!("discovered|{name}"));
            s.set_str("display_label", format!("{name} \u{2014} OMT discovery"));
            s.set_str("backend", "OMT discovery");
            s.set_str("name", name);
            s
        })
        .collect();
    ctx.set("sources", Value::List(sources));
    ctx.set_str("current_source", summary.source.as_str());
    ctx.set_str("current_direct_target", summary.direct_address.as_str());
    let mut pb = Value::map();
    pb.set_str("state", summary.state);
    pb.set_str("label", summary.label);
    pb.set_str("detail", summary.detail);
    pb.set_str("tone", summary.tone);
    pb.set_str("source", summary.source);
    pb.set_str("direct_address", summary.direct_address);
    ctx.set("playback", pb);
    ctx.set("video_limit", video_limit_value(r.app.playback.video_limit()));
    r.render("dashboard.html", &ctx, 200);
}

fn login_get(r: &mut Ctx<'_, '_>) {
    if r.current_session() {
        redirect(r.res, "/");
        return;
    }
    r.login_page("", 200);
}

fn login_post(r: &mut Ctx<'_, '_>) {
    let limit = r.app.settings.login_limit;
    if !r.allow("login", limit) {
        r.login_page("Too many login attempts. Please wait.", 429);
        return;
    }
    let form = Form::parse(r.req.body);
    let nonce = request_cookie(r.req, LOGIN_COOKIE, SESSION_COOKIE_MAX);
    let valid = match (&nonce, form.get("csrf_token")) {
        (Some(nonce), Some(token)) => r.app.auth.verify_csrf("login", nonce, token),
        _ => false,
    };
    if !valid {
        r.login_page("Session expired. Please try again.", 400);
        return;
    }
    let password = form.field("password");
    let previous = request_cookie(r.req, SESSION_COOKIE, SESSION_COOKIE_MAX);
    match r.app.auth.authenticate(password.as_bytes(), previous.as_deref()) {
        Ok(Some(session)) => {
            redirect(r.res, "/");
            set_cookie(r.res, SESSION_COOKIE, &session, r.app.settings.session_lifetime_s);
            clear_cookie(r.res, LOGIN_COOKIE);
        }
        Ok(None) => r.login_page("Invalid password", 200),
        Err(e) => {
            eprintln!("unable to create persistent session: {e}");
            r.login_page(
                "Unable to create a persistent session. Check configuration storage.",
                503,
            );
        }
    }
}

fn logout(r: &mut Ctx<'_, '_>) {
    if r.authenticated_post().is_none() {
        return;
    }
    if let Err(e) = r.app.auth.revoke(&r.session) {
        eprintln!("unable to revoke session: {e}");
    }
    redirect(r.res, "/login");
    clear_cookie(r.res, SESSION_COOKIE);
}

fn post_action(r: &mut Ctx<'_, '_>, to: &str, action: fn(&mut App, &Form) -> ActionResult) {
    if let Some(form) = r.authenticated_post() {
        let result = action(r.app, &form);
        r.flash_redirect(to, &result);
    }
}

/// A trimmed form field, as long as the C frontend's buffer kept.
fn trimmed_address(form: &Form, key: &str) -> String {
    clip(form.field(key).trim().to_owned(), omt_protocol::TARGET_MAX_BYTES * 2 - 1)
}

fn select_source(r: &mut Ctx<'_, '_>) {
    post_action(r, "/", |app, f| app.playback.select(f.field("source")));
}

fn restart_playback(r: &mut Ctx<'_, '_>) {
    post_action(r, "/", |app, _| app.playback.restart());
}

fn clear_playback(r: &mut Ctx<'_, '_>) {
    post_action(r, "/", |app, _| app.playback.clear());
}

fn direct_source(r: &mut Ctx<'_, '_>) {
    post_action(r, "/settings/network", |app, f| {
        app.playback.save_direct(&trimmed_address(f, "direct_address"))
    });
}

fn video_limit_post(r: &mut Ctx<'_, '_>) {
    post_action(r, "/system", |app, f| app.playback.save_video_limit(f.field("video_limit")));
}

fn refresh_sources(r: &mut Ctx<'_, '_>) {
    if r.authenticated_post().is_some() {
        r.app.playback.refresh();
        redirect(r.res, "/");
    }
}

fn network_page(r: &mut Ctx<'_, '_>, submitted: Option<&str>, error_override: Option<&str>) {
    let token = r.session_token();
    let mut network = network::read_network_configuration(&r.app.settings.runtime_config_file);
    if let Some(s) = submitted {
        network.discovery_server = clip(s.to_owned(), 599);
    }
    if let Some(e) = error_override {
        e.clone_into(&mut network.error);
    }
    let network_error = clip(network.error, 255);
    let configuration = r.app.playback.configuration();
    let mut ctx = r.common(true, &token, "network.network_settings");
    let mut n = Value::map();
    n.set_str("discovery_server", network.discovery_server.as_str());
    n.set_str("discovery_server_text", network.discovery_server);
    n.set_str("error", network_error);
    ctx.set("network", n);
    ctx.set_str("current_source", configuration.source);
    ctx.set_str("current_direct_target", configuration.direct_address);
    ctx.set_str("configuration_error", configuration.error);
    r.render("network.html", &ctx, 200);
}

fn network_get(r: &mut Ctx<'_, '_>) {
    if r.require_session() {
        network_page(r, None, None);
    }
}

fn network_post(r: &mut Ctx<'_, '_>) {
    let Some(form) = r.authenticated_post() else { return };
    let submitted = form.field("discovery_server");
    match network::save_network_configuration(&r.app.settings.runtime_config_file, submitted) {
        Ok(false) => {
            let result = ActionResult::success("OMT discovery settings are already up to date.");
            r.flash_redirect("/settings/network", &result);
        }
        Ok(true) => {
            r.app.playback.refresh();
            let result = if r.app.playback.configuration().configured() {
                r.app.playback.restart()
            } else {
                ActionResult::success("OMT discovery settings saved.")
            };
            r.flash_redirect("/settings/network", &result);
        }
        Err(e) => {
            // A refused value re-renders the form with what was typed,
            // rather than redirecting the operator's input away.
            let message = e.to_string();
            network_page(r, Some(submitted), Some(&message));
            if !r.has_flash_cookie {
                clear_cookie(r.res, FLASH_COOKIE);
            }
            eprintln!("network setting rejected: {message}");
        }
    }
}

fn diagnostics_page(
    r: &mut Ctx<'_, '_>,
    result: Option<&DiagnosticResult>,
    observed_status: Option<&str>,
) {
    let token = r.session_token();
    let configuration = r.app.playback.configuration();
    let mut ctx = r.common(true, &token, "diagnostics.diagnostics");
    let status = match observed_status {
        Some(s) => s.to_owned(),
        None => diagnostics::status(&r.app.settings),
    };
    ctx.set_str("app_version", diagnostics::app_version(&r.app.settings));
    ctx.set_str("current_source", configuration.source);
    ctx.set_str("current_direct_target", configuration.direct_address);
    ctx.set_str("configuration_error", configuration.error);
    ctx.set_str("omt_status", status);
    match result {
        Some(result) => {
            let mut m = Value::map();
            m.set_str("title", result.title);
            m.set("command", command_value(result));
            ctx.set("result", m);
        }
        None => ctx.set("result", Value::None),
    }
    r.render("diagnostics.html", &ctx, 200);
}

fn diagnostics_get(r: &mut Ctx<'_, '_>) {
    if r.require_session() {
        diagnostics_page(r, None, None);
    }
}

#[derive(Clone, Copy)]
enum DiagKind {
    Discovery,
    Runtime,
    Direct,
}

fn diagnostics_action(r: &mut Ctx<'_, '_>, kind: DiagKind) {
    let Some(form) = r.authenticated_post() else { return };
    let limit = r.app.settings.diagnostic_action_limit;
    if !r.allow("diagnostics", limit) {
        r.too_many();
        return;
    }
    match kind {
        DiagKind::Discovery => {
            r.app.playback.refresh();
            let result = diagnostics::discovery(&r.app.settings);
            diagnostics_page(r, Some(&result), None);
        }
        DiagKind::Runtime => {
            let (result, status) = diagnostics::runtime(&r.app.settings);
            diagnostics_page(r, Some(&result), Some(&status));
        }
        DiagKind::Direct => {
            let address = trimmed_address(&form, "direct_address");
            let result = diagnostics::direct(&r.app.settings, &address);
            diagnostics_page(r, Some(&result), None);
        }
    }
}

fn diagnostics_discovery(r: &mut Ctx<'_, '_>) {
    diagnostics_action(r, DiagKind::Discovery);
}

fn diagnostics_runtime(r: &mut Ctx<'_, '_>) {
    diagnostics_action(r, DiagKind::Runtime);
}

fn diagnostics_direct(r: &mut Ctx<'_, '_>) {
    diagnostics_action(r, DiagKind::Direct);
}

fn diagnostics_download(r: &mut Ctx<'_, '_>) {
    let Some(form) = r.authenticated_post() else { return };
    let limit = r.app.settings.diagnostic_download_limit;
    if !r.allow("diagnostic-download", limit) {
        r.too_many();
        return;
    }
    let include = form.get("include_packet_capture") == Some("1");
    let version = diagnostics::app_version(&r.app.settings);
    match diagnostics::bundle(&r.app.settings, &r.app.playback, include, &version) {
        Ok((archive, filename)) => {
            r.res.status = 200;
            r.res.add_header("content-type", "application/zip");
            r.res
                .add_header("content-disposition", &format!("attachment; filename=\"{filename}\""));
            r.res.body = archive;
        }
        Err(e) => {
            eprintln!("support bundle failed: {e}");
            r.error_page(
                "Something went wrong",
                "The appliance could not complete that request. Check the container logs.",
                500,
            );
        }
    }
}

fn system_get(r: &mut Ctx<'_, '_>) {
    if !r.require_session() {
        return;
    }
    let token = r.session_token();
    let mut ctx = r.common(true, &token, "system.system");
    ctx.set("video_limit", video_limit_value(r.app.playback.video_limit()));
    r.render("system.html", &ctx, 200);
}

fn reboot_get(r: &mut Ctx<'_, '_>) {
    if !r.require_session() {
        return;
    }
    let token = r.session_token();
    let ctx = r.common(true, &token, "system.confirm_reboot");
    r.render("reboot_confirm.html", &ctx, 200);
}

fn reboot_post(r: &mut Ctx<'_, '_>) {
    if r.authenticated_post().is_none() {
        return;
    }
    let limit = r.app.settings.reboot_limit;
    if !r.allow("reboot", limit) {
        r.too_many();
        return;
    }
    let result = diagnostics::request_reboot(&r.app.settings);
    if result.ok {
        let token = r.session_token();
        let mut ctx = r.common(true, &token, "system.reboot");
        ctx.set_str("message", result.message);
        r.render("reboot_scheduled.html", &ctx, 202);
    } else {
        r.flash_redirect("/system", &result);
    }
}

fn about(r: &mut Ctx<'_, '_>) {
    if !r.require_session() {
        return;
    }
    let token = r.session_token();
    let version = diagnostics::app_version(&r.app.settings);
    let (license, notices) = diagnostics::legal_texts(&r.app.settings);
    let mut ctx = r.common(true, &token, "about.about");
    ctx.set_str("app_version", version);
    ctx.set_str("project_license", license);
    ctx.set_str("third_party_notices", notices);
    r.render("about.html", &ctx, 200);
}

fn not_found(r: &mut Ctx<'_, '_>) {
    if r.current_session() {
        r.error_page("Page not found", "That page does not exist.", 404);
    } else {
        r.login_page("That page does not exist.", 404);
    }
}

type HandlerFn = fn(&mut Ctx<'_, '_>);

struct Route {
    path: &'static str,
    get: Option<HandlerFn>,
    post: Option<HandlerFn>,
}

/// The route table: one row per path, with its GET and POST handlers. Every
/// path here is documented in docs/CODEBASE_REFERENCE.md or OPERATIONS.md.
static ROUTES: [Route; 18] = [
    Route { path: "/login", get: Some(login_get), post: Some(login_post) },
    Route { path: "/logout", get: None, post: Some(logout) },
    Route { path: "/", get: Some(dashboard), post: None },
    Route { path: "/sources/select", get: None, post: Some(select_source) },
    Route { path: "/sources/refresh", get: None, post: Some(refresh_sources) },
    Route { path: "/playback/restart", get: None, post: Some(restart_playback) },
    Route { path: "/playback/clear", get: None, post: Some(clear_playback) },
    Route { path: "/settings/network", get: Some(network_get), post: Some(network_post) },
    Route { path: "/settings/direct-source", get: None, post: Some(direct_source) },
    Route { path: "/diagnostics", get: Some(diagnostics_get), post: None },
    Route { path: "/diagnostics/discovery", get: None, post: Some(diagnostics_discovery) },
    Route { path: "/diagnostics/runtime", get: None, post: Some(diagnostics_runtime) },
    Route { path: "/diagnostics/direct", get: None, post: Some(diagnostics_direct) },
    Route { path: "/diagnostics/download", get: None, post: Some(diagnostics_download) },
    Route { path: "/system", get: Some(system_get), post: None },
    Route { path: "/system/video-limit", get: None, post: Some(video_limit_post) },
    Route { path: "/system/reboot", get: Some(reboot_get), post: Some(reboot_post) },
    Route { path: "/about", get: Some(about), post: None },
];
