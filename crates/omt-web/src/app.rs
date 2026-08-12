use crate::{
    auth::Authentication,
    diagnostics::{self, DiagnosticResult, Diagnostics},
    network,
    playback::{ActionResult, Playback},
    settings::{RateLimit, Settings},
    templates,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use minijinja::Environment;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Instant,
};

const SESSION_COOKIE: &str = "__Host-omt_session";
const LOGIN_COOKIE: &str = "__Host-omt_login";
const FLASH_COOKIE: &str = "__Host-omt_flash";

pub struct AppState {
    pub settings: Settings,
    pub auth: Authentication,
    pub playback: Arc<Playback>,
    pub diagnostics: Diagnostics,
    templates: Environment<'static>,
    hostname: String,
    rates: Mutex<HashMap<(String, IpAddr), VecDeque<Instant>>>,
    flashes: Mutex<BTreeMap<String, (String, String)>>,
}

impl AppState {
    pub fn build(settings: Settings) -> Result<Arc<Self>, String> {
        let playback = Arc::new(Playback::new(&settings));
        let hostname = std::fs::read_to_string("/etc/hostname")
            .unwrap_or_else(|_| "omt-client".to_owned())
            .trim()
            .to_owned();
        Ok(Arc::new(Self {
            auth: Authentication::load(&settings)?,
            diagnostics: Diagnostics::new(&settings, Arc::clone(&playback)),
            templates: templates::environment()?,
            hostname,
            playback,
            settings,
            rates: Mutex::new(HashMap::new()),
            flashes: Mutex::new(BTreeMap::new()),
        }))
    }

    fn allow(&self, scope: &str, address: IpAddr, limit: RateLimit) -> bool {
        let Ok(mut rates) = self.rates.lock() else {
            return false;
        };
        let now = Instant::now();
        for entries in rates.values_mut() {
            while entries.front().is_some_and(|expires| *expires <= now) {
                entries.pop_front();
            }
        }
        rates.retain(|_, entries| !entries.is_empty());
        let key = (scope.to_owned(), address);
        if !rates.contains_key(&key) && rates.len() >= 4_096 {
            return false;
        }
        let entries = rates.entry(key).or_default();
        if entries.len() >= limit.count {
            return false;
        }
        entries.push_back(now + limit.window);
        true
    }

    fn common(
        &self,
        authenticated: bool,
        csrf_token: &str,
        endpoint: &str,
        headers: &HeaderMap,
    ) -> Map<String, Value> {
        let flashes = cookie(headers, FLASH_COOKIE)
            .and_then(|key| self.flashes.lock().ok()?.remove(&key))
            .map_or_else(Vec::new, |(category, message)| {
                vec![json!([category, message])]
            });
        let mut context = Map::new();
        context.insert("hostname".to_owned(), json!(self.hostname));
        context.insert("authenticated".to_owned(), json!(authenticated));
        context.insert("csrf_token".to_owned(), json!(csrf_token));
        context.insert("endpoint".to_owned(), json!(endpoint));
        context.insert("flashes".to_owned(), json!(flashes));
        context
    }

    fn render(
        &self,
        template: &str,
        context: Map<String, Value>,
        status: StatusCode,
        clear_flash: bool,
    ) -> Response {
        match self
            .templates
            .get_template(template)
            .and_then(|value| value.render(Value::Object(context)))
        {
            Ok(body) => {
                let mut response = (
                    status,
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    body,
                )
                    .into_response();
                if clear_flash {
                    append_cookie(&mut response, clear_cookie(FLASH_COOKIE));
                }
                response
            }
            Err(error) => {
                eprintln!("template error: {error}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
        }
    }

    fn flash_redirect(&self, destination: &'static str, result: ActionResult) -> Response {
        let Ok(key) = crate::io::random_hex(16) else {
            return Redirect::to(destination).into_response();
        };
        if let Ok(mut flashes) = self.flashes.lock() {
            while flashes.len() >= 64 {
                if let Some(first) = flashes.keys().next().cloned() {
                    flashes.remove(&first);
                } else {
                    break;
                }
            }
            flashes.insert(
                key.clone(),
                if result.ok {
                    ("success".to_owned(), result.message)
                } else {
                    ("error".to_owned(), result.error)
                },
            );
        }
        let mut response = Redirect::to(destination).into_response();
        append_cookie(
            &mut response,
            format!("{FLASH_COOKIE}={key}; Path=/; Max-Age=60; Secure; HttpOnly; SameSite=Lax"),
        );
        response
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/login", get(login_get).post(login_post))
        .route("/logout", post(logout))
        .route("/", get(dashboard))
        .route("/sources/select", post(select_source))
        .route("/sources/refresh", post(refresh_sources))
        .route("/playback/restart", post(restart_playback))
        .route("/playback/clear", post(clear_playback))
        .route("/settings/network", get(network_get).post(network_post))
        .route("/settings/direct-source", post(direct_source))
        .route("/diagnostics", get(diagnostics_get))
        .route("/diagnostics/discovery", post(diagnostics_discovery))
        .route("/diagnostics/runtime", post(diagnostics_runtime))
        .route("/diagnostics/direct", post(diagnostics_direct))
        .route("/diagnostics/download", post(diagnostics_download))
        .route("/system", get(system_get))
        .route("/system/video-limit", post(video_limit))
        .route("/system/reboot", get(reboot_get).post(reboot_post))
        .route("/about", get(about))
        .route("/static/style.css", get(style))
        .route("/static/favicon.svg", get(favicon))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(state.settings.max_request_bytes))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_owned())
        })
}

fn cookie_value(name: &str, value: &str, max_age: u64) -> String {
    format!("{name}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Lax")
}
fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax")
}
fn append_cookie(response: &mut Response, cookie: String) {
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn session(state: &AppState, headers: &HeaderMap) -> Option<String> {
    cookie(headers, SESSION_COOKIE).filter(|session_id| state.auth.is_current(session_id))
}

fn form(body: &Bytes) -> HashMap<String, String> {
    serde_urlencoded::from_bytes(body).unwrap_or_default()
}

fn csrf_valid(state: &AppState, headers: &HeaderMap, body: &Bytes, session_id: &str) -> bool {
    let values = form(body);
    values
        .get("csrf_token")
        .is_some_and(|token| state.auth.verify_csrf("session", session_id, token))
        && session(state, headers).as_deref() == Some(session_id)
}

fn require_session(state: &AppState, headers: &HeaderMap) -> Result<String, Box<Response>> {
    session(state, headers).ok_or_else(|| Box::new(Redirect::to("/login").into_response()))
}

fn csrf_error(state: &AppState, headers: &HeaderMap, session_id: &str) -> Response {
    let token = state
        .auth
        .csrf_token("session", session_id)
        .unwrap_or_default();
    let mut context = state.common(true, &token, "error", headers);
    context.insert("title".to_owned(), json!("Session expired"));
    context.insert(
        "message".to_owned(),
        json!("Session expired. Please try again."),
    );
    state.render(
        "error.html",
        context,
        StatusCode::BAD_REQUEST,
        cookie(headers, FLASH_COOKIE).is_some(),
    )
}

async fn login_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if session(&state, &headers).is_some() {
        return Redirect::to("/").into_response();
    }
    login_page(&state, &headers, "", StatusCode::OK)
}

fn login_page(state: &AppState, headers: &HeaderMap, error: &str, status: StatusCode) -> Response {
    let nonce = crate::io::random_hex(24).unwrap_or_default();
    let token = state.auth.csrf_token("login", &nonce).unwrap_or_default();
    let mut context = state.common(false, &token, "auth.login", headers);
    context.insert("error".to_owned(), json!(error));
    let mut response = state.render(
        "login.html",
        context,
        status,
        cookie(headers, FLASH_COOKIE).is_some(),
    );
    append_cookie(&mut response, cookie_value(LOGIN_COOKIE, &nonce, 600));
    response
}

async fn login_post(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.allow("login", address.ip(), state.settings.login_limit) {
        return login_page(
            &state,
            &headers,
            "Too many login attempts. Please wait.",
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    let values = form(&body);
    let nonce = cookie(&headers, LOGIN_COOKIE).unwrap_or_default();
    if nonce.is_empty()
        || !values
            .get("csrf_token")
            .is_some_and(|token| state.auth.verify_csrf("login", &nonce, token))
    {
        return login_page(
            &state,
            &headers,
            "Session expired. Please try again.",
            StatusCode::BAD_REQUEST,
        );
    }
    match state.auth.authenticate(
        values.get("password").map_or("", String::as_str),
        cookie(&headers, SESSION_COOKIE).as_deref(),
    ) {
        Ok(Some(session_id)) => {
            let mut response = Redirect::to("/").into_response();
            append_cookie(
                &mut response,
                cookie_value(
                    SESSION_COOKIE,
                    &session_id,
                    state.settings.session_lifetime.as_secs(),
                ),
            );
            append_cookie(&mut response, clear_cookie(LOGIN_COOKIE));
            response
        }
        Ok(None) => login_page(&state, &headers, "Invalid password", StatusCode::OK),
        Err(error) => {
            eprintln!("unable to create persistent session: {error}");
            login_page(
                &state,
                &headers,
                "Unable to create a persistent session. Check configuration storage.",
                StatusCode::SERVICE_UNAVAILABLE,
            )
        }
    }
}

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(session_id) = session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    if !csrf_valid(&state, &headers, &body, &session_id) {
        return csrf_error(&state, &headers, &session_id);
    }
    if let Err(error) = state.auth.revoke(&session_id) {
        eprintln!("unable to revoke session: {error}");
    }
    let mut response = Redirect::to("/login").into_response();
    append_cookie(&mut response, clear_cookie(SESSION_COOKIE));
    response
}

async fn dashboard(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let session_id = match require_session(&state, &headers) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let token = state
        .auth
        .csrf_token("session", &session_id)
        .unwrap_or_default();
    let playback = state.playback.playback();
    let mut context = state.common(true, &token, "dashboard.dashboard", &headers);
    context.insert("sources".to_owned(), json!(state.playback.sources()));
    context.insert("current_source".to_owned(), json!(playback.source));
    context.insert(
        "current_direct_target".to_owned(),
        json!(playback.direct_address),
    );
    context.insert("playback".to_owned(), json!(playback));
    context.insert(
        "video_limit".to_owned(),
        json!(state.playback.video_limit()),
    );
    state.render(
        "dashboard.html",
        context,
        StatusCode::OK,
        cookie(&headers, FLASH_COOKIE).is_some(),
    )
}

macro_rules! authenticated_post {
    ($state:expr, $headers:expr, $body:expr) => {{
        let session_id = match require_session(&$state, &$headers) {
            Ok(value) => value,
            Err(response) => return *response,
        };
        if !csrf_valid(&$state, &$headers, &$body, &session_id) {
            return csrf_error(&$state, &$headers, &session_id);
        }
        (session_id, form(&$body))
    }};
}

async fn select_source(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (_, values) = authenticated_post!(state, headers, body);
    state.flash_redirect(
        "/",
        state
            .playback
            .select(values.get("source").map_or("", String::as_str)),
    )
}
async fn refresh_sources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let _ = authenticated_post!(state, headers, body);
    state.playback.refresh();
    Redirect::to("/").into_response()
}
async fn restart_playback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let _ = authenticated_post!(state, headers, body);
    state.flash_redirect("/", state.playback.restart())
}
async fn clear_playback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let _ = authenticated_post!(state, headers, body);
    state.flash_redirect("/", state.playback.clear())
}

fn network_page(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
    submitted: Option<&str>,
    error_override: Option<&str>,
) -> Response {
    let token = state
        .auth
        .csrf_token("session", session_id)
        .unwrap_or_default();
    let mut network = network::read_configuration(&state.settings.runtime_config_file);
    if let Some(value) = submitted {
        value.clone_into(&mut network.discovery_server);
    }
    if let Some(error) = error_override {
        error.clone_into(&mut network.error);
    }
    let configuration = state.playback.configuration();
    let mut context = state.common(true, &token, "network.network_settings", headers);
    context.insert("network".to_owned(), json!({"discovery_server": network.discovery_server, "discovery_server_text": network.discovery_server, "error": network.error}));
    context.insert("current_source".to_owned(), json!(configuration.source));
    context.insert(
        "current_direct_target".to_owned(),
        json!(configuration.direct_address),
    );
    context.insert("configuration_error".to_owned(), json!(configuration.error));
    state.render(
        "network.html",
        context,
        StatusCode::OK,
        cookie(headers, FLASH_COOKIE).is_some(),
    )
}

async fn network_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let session_id = match require_session(&state, &headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    network_page(&state, &headers, &session_id, None, None)
}
async fn network_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (session_id, values) = authenticated_post!(state, headers, body);
    let submitted = values.get("discovery_server").map_or("", String::as_str);
    match network::save_configuration(&state.settings.runtime_config_file, submitted) {
        Ok(false) => state.flash_redirect(
            "/settings/network",
            ActionResult {
                ok: true,
                message: "OMT discovery settings are already up to date.".to_owned(),
                error: String::new(),
            },
        ),
        Ok(true) => {
            state.playback.refresh();
            let configured = state.playback.configuration().configured();
            let result = if configured {
                state.playback.restart()
            } else {
                ActionResult {
                    ok: true,
                    message: "OMT discovery settings saved.".to_owned(),
                    error: String::new(),
                }
            };
            state.flash_redirect("/settings/network", result)
        }
        Err(error) => {
            let mut response =
                network_page(&state, &headers, &session_id, Some(submitted), Some(&error));
            if let Ok(value) = HeaderValue::from_str(&format!(
                "{FLASH_COOKIE}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax"
            )) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            eprintln!("network setting rejected: {error}");
            response
        }
    }
}
async fn direct_source(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (_, values) = authenticated_post!(state, headers, body);
    state.flash_redirect(
        "/settings/network",
        state
            .playback
            .save_direct(values.get("direct_address").map_or("", |v| v.trim())),
    )
}

fn diagnostics_page(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
    result: Option<DiagnosticResult>,
    observed_status: Option<String>,
) -> Response {
    let token = state
        .auth
        .csrf_token("session", session_id)
        .unwrap_or_default();
    let configuration = state.playback.configuration();
    let mut context = state.common(true, &token, "diagnostics.diagnostics", headers);
    context.insert(
        "app_version".to_owned(),
        json!(diagnostics::version(&state.settings)),
    );
    context.insert("current_source".to_owned(), json!(configuration.source));
    context.insert(
        "current_direct_target".to_owned(),
        json!(configuration.direct_address),
    );
    context.insert("configuration_error".to_owned(), json!(configuration.error));
    context.insert(
        "omt_status".to_owned(),
        json!(observed_status.unwrap_or_else(|| state.diagnostics.status())),
    );
    context.insert("result".to_owned(), json!(result));
    state.render(
        "diagnostics.html",
        context,
        StatusCode::OK,
        cookie(headers, FLASH_COOKIE).is_some(),
    )
}
async fn diagnostics_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let id = match require_session(&state, &headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    diagnostics_page(&state, &headers, &id, None, None)
}
async fn diagnostics_discovery(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, _) = authenticated_post!(state, headers, body);
    if !state.allow(
        "diagnostics",
        address.ip(),
        state.settings.diagnostic_action_limit,
    ) {
        return too_many(&state, &headers, &id);
    }
    state.playback.refresh();
    diagnostics_page(
        &state,
        &headers,
        &id,
        Some(state.diagnostics.discovery()),
        None,
    )
}
async fn diagnostics_runtime(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, _) = authenticated_post!(state, headers, body);
    if !state.allow(
        "diagnostics",
        address.ip(),
        state.settings.diagnostic_action_limit,
    ) {
        return too_many(&state, &headers, &id);
    }
    let (result, status) = state.diagnostics.runtime();
    diagnostics_page(&state, &headers, &id, Some(result), Some(status))
}
async fn diagnostics_direct(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, values) = authenticated_post!(state, headers, body);
    if !state.allow(
        "diagnostics",
        address.ip(),
        state.settings.diagnostic_action_limit,
    ) {
        return too_many(&state, &headers, &id);
    }
    diagnostics_page(
        &state,
        &headers,
        &id,
        Some(
            state
                .diagnostics
                .direct(values.get("direct_address").map_or("", |v| v.trim())),
        ),
        None,
    )
}
async fn diagnostics_download(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, values) = authenticated_post!(state, headers, body);
    if !state.allow(
        "diagnostic-download",
        address.ip(),
        state.settings.diagnostic_download_limit,
    ) {
        return too_many(&state, &headers, &id);
    }
    match state.diagnostics.bundle(
        values.get("include_packet_capture").map(String::as_str) == Some("1"),
        &diagnostics::version(&state.settings),
    ) {
        Ok(bundle) => {
            let mut response = (StatusCode::OK, bundle.bytes).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            );
            if let Ok(value) =
                HeaderValue::from_str(&format!("attachment; filename=\"{}\"", bundle.filename))
            {
                response
                    .headers_mut()
                    .insert(header::CONTENT_DISPOSITION, value);
            }
            response
        }
        Err(error) => {
            eprintln!("support bundle failed: {error}");
            error_page(
                &state,
                &headers,
                &id,
                "Something went wrong",
                "The appliance could not complete that request. Check the container logs.",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    }
}

async fn system_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let id = match require_session(&state, &headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let token = state.auth.csrf_token("session", &id).unwrap_or_default();
    let mut context = state.common(true, &token, "system.system", &headers);
    context.insert(
        "video_limit".to_owned(),
        json!(state.playback.video_limit()),
    );
    state.render(
        "system.html",
        context,
        StatusCode::OK,
        cookie(&headers, FLASH_COOKIE).is_some(),
    )
}
async fn video_limit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (_, values) = authenticated_post!(state, headers, body);
    state.flash_redirect(
        "/system",
        state
            .playback
            .save_video_limit(values.get("video_limit").map_or("", String::as_str)),
    )
}
async fn reboot_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let id = match require_session(&state, &headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let token = state.auth.csrf_token("session", &id).unwrap_or_default();
    let context = state.common(true, &token, "system.confirm_reboot", &headers);
    state.render(
        "reboot_confirm.html",
        context,
        StatusCode::OK,
        cookie(&headers, FLASH_COOKIE).is_some(),
    )
}
async fn reboot_post(
    State(state): State<Arc<AppState>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (id, _) = authenticated_post!(state, headers, body);
    if !state.allow("reboot", address.ip(), state.settings.reboot_limit) {
        return too_many(&state, &headers, &id);
    }
    let result = state.diagnostics.request_reboot();
    if result.ok {
        let token = state.auth.csrf_token("session", &id).unwrap_or_default();
        let mut context = state.common(true, &token, "system.reboot", &headers);
        context.insert("message".to_owned(), json!(result.message));
        state.render(
            "reboot_scheduled.html",
            context,
            StatusCode::ACCEPTED,
            cookie(&headers, FLASH_COOKIE).is_some(),
        )
    } else {
        state.flash_redirect("/system", result)
    }
}

async fn about(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let id = match require_session(&state, &headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let token = state.auth.csrf_token("session", &id).unwrap_or_default();
    let (license, notices) = diagnostics::legal_texts(&state.settings);
    let mut context = state.common(true, &token, "about.about", &headers);
    context.insert(
        "app_version".to_owned(),
        json!(diagnostics::version(&state.settings)),
    );
    context.insert("project_license".to_owned(), json!(license));
    context.insert("third_party_notices".to_owned(), json!(notices));
    state.render(
        "about.html",
        context,
        StatusCode::OK,
        cookie(&headers, FLASH_COOKIE).is_some(),
    )
}

fn error_page(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
    title: &str,
    message: &str,
    status: StatusCode,
) -> Response {
    let token = state
        .auth
        .csrf_token("session", session_id)
        .unwrap_or_default();
    let mut context = state.common(true, &token, "error", headers);
    context.insert("title".to_owned(), json!(title));
    context.insert("message".to_owned(), json!(message));
    state.render(
        "error.html",
        context,
        status,
        cookie(headers, FLASH_COOKIE).is_some(),
    )
}
fn too_many(state: &AppState, headers: &HeaderMap, session_id: &str) -> Response {
    error_page(
        state,
        headers,
        session_id,
        "Too many requests",
        "Too many requests. Please wait and try again.",
        StatusCode::TOO_MANY_REQUESTS,
    )
}

async fn not_found(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(id) = session(&state, &headers) {
        error_page(
            &state,
            &headers,
            &id,
            "Page not found",
            "That page does not exist.",
            StatusCode::NOT_FOUND,
        )
    } else {
        login_page(
            &state,
            &headers,
            "That page does not exist.",
            StatusCode::NOT_FOUND,
        )
    }
}
async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}
async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../static/favicon.svg"),
    )
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let is_static = request.uri().path().starts_with("/static/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'",
        ),
    );
    if is_static {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
    } else {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Method};
    use std::{
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tower::ServiceExt;

    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "omt-web-test-{}",
            crate::io::random_hex(8).unwrap_or_default()
        ));
        fs::create_dir_all(root.join("omt")).unwrap_or_else(|error| panic!("{error}"));
        root
    }

    fn test_settings(root: &Path) -> Settings {
        let rate = RateLimit {
            count: 1,
            window: Duration::from_mins(1),
        };
        Settings {
            config_dir: root.to_path_buf(),
            runtime_dir: root.join("run"),
            password_file: root.join("web_password"),
            session_lifetime: Duration::from_hours(1),
            max_request_bytes: 16_384,
            login_limit: rate,
            diagnostic_action_limit: rate,
            diagnostic_download_limit: rate,
            reboot_limit: rate,
            control_command: PathBuf::from("/bin/false"),
            receiver_command: PathBuf::from("/bin/false"),
            control_timeout: Duration::from_millis(100),
            source_cache_ttl: Duration::ZERO,
            source_target_file: root.join("source_target.json"),
            video_ceiling_file: root.join("video_ceiling.json"),
            board_label: "Test Pi".to_owned(),
            board_video_ceiling: "1920x1080@60".to_owned(),
            playback_status_file: root.join("run/playback-status.json"),
            sdk_config_dir: root.join("omt"),
            runtime_config_file: root.join("omt/settings.xml"),
            playback_status_stale: Duration::from_secs(5),
            diagnostics_host_report_file: root.join("host-report"),
            diagnostics_host_request_file: root.join("host-request"),
            diagnostics_host_pcap_file: root.join("host.pcap"),
            diagnostics_host_pcap_metadata_file: root.join("host-pcap.txt"),
            diagnostics_host_timeout: Duration::from_millis(10),
            diagnostics_host_budget: 1,
            diagnostics_bundle_budget: Duration::from_secs(1),
            diagnostics_receive_probe: false,
            version_file: root.join("version"),
            runtime_integrity_manifest: root.join("manifest"),
            project_license_file: root.join("LICENSE"),
            third_party_notices_file: root.join("NOTICES"),
            reboot_request_file: root.join("reboot.request"),
            reboot_result_file: root.join("reboot.result"),
            reboot_ack_timeout: Duration::from_millis(10),
            web_port: 5000,
            tls_cert_file: root.join("cert.pem"),
            tls_key_file: root.join("key.pem"),
        }
    }

    fn test_state() -> (Arc<AppState>, PathBuf) {
        let root = test_root();
        fs::write(root.join("web_secret"), "test-secret\n")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("web_password"), "correct\n").unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("omt/settings.xml"), "<Settings />\n")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("version"), "0.9.test\n").unwrap_or_else(|error| panic!("{error}"));
        fs::write(
            root.join("LICENSE"),
            "Copyright (c) 2026 Matthew David Miller\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("NOTICES"), "notices\n").unwrap_or_else(|error| panic!("{error}"));
        (
            AppState::build(test_settings(&root)).unwrap_or_else(|error| panic!("{error}")),
            root,
        )
    }

    fn request(method: Method, uri: &str, cookie_value: Option<&str>, body: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(value) = cookie_value {
            builder = builder.header(header::COOKIE, value);
        }
        let mut request = builder
            .body(Body::from(body.to_owned()))
            .unwrap_or_else(|error| panic!("{error}"));
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        request
    }

    fn set_cookie(response: &Response, name: &str) -> String {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|value| {
                value
                    .split(';')
                    .next()?
                    .strip_prefix(&format!("{name}="))
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| panic!("missing {name} cookie"))
    }

    fn hidden_token(body: &str) -> String {
        let marker = "name=\"csrf_token\" value=\"";
        let rest = body
            .split_once(marker)
            .map_or_else(|| panic!("missing CSRF token"), |(_, value)| value);
        rest.split_once('"')
            .map(|(value, _)| value.to_owned())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn login_sessions_csrf_rate_limits_and_headers() {
        let (state, root) = test_state();
        let service = router(Arc::clone(&state));
        let login = service
            .clone()
            .oneshot(request(Method::GET, "/login", None, ""))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(login.status(), StatusCode::OK);
        assert_eq!(
            login
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some("default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'")
        );
        assert_eq!(
            login
                .headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .and_then(|value| value.to_str().ok()),
            Some("max-age=31536000; includeSubDomains")
        );
        let nonce = set_cookie(&login, LOGIN_COOKIE);
        let login_body = to_bytes(login.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        let token = hidden_token(&String::from_utf8_lossy(&login_body));
        let submitted = format!("csrf_token={token}&password=correct");
        let authenticated = service
            .clone()
            .oneshot(request(
                Method::POST,
                "/login",
                Some(&format!("{LOGIN_COOKIE}={nonce}")),
                &submitted,
            ))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(authenticated.status(), StatusCode::SEE_OTHER);
        let session_id = set_cookie(&authenticated, SESSION_COOKIE);
        let dashboard = service
            .clone()
            .oneshot(request(
                Method::GET,
                "/",
                Some(&format!("{SESSION_COOKIE}={session_id}")),
                "",
            ))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(dashboard.status(), StatusCode::OK);
        let dashboard_body = to_bytes(dashboard.into_body(), 256 * 1024)
            .await
            .unwrap_or_default();
        assert!(String::from_utf8_lossy(&dashboard_body).contains("Dashboard"));
        let rejected = service
            .clone()
            .oneshot(request(
                Method::POST,
                "/sources/refresh",
                Some(&format!("{SESSION_COOKIE}={session_id}")),
                "csrf_token=wrong",
            ))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let throttled = service
            .oneshot(request(
                Method::POST,
                "/login",
                Some(&format!("{LOGIN_COOKIE}={nonce}")),
                &submitted,
            ))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
        fs::remove_dir_all(root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[tokio::test]
    async fn every_authenticated_page_renders() {
        let (state, root) = test_state();
        let session_id = state
            .auth
            .authenticate("correct", None)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_default();
        let service = router(state);
        for path in [
            "/",
            "/settings/network",
            "/diagnostics",
            "/system",
            "/system/reboot",
            "/about",
        ] {
            let response = service
                .clone()
                .oneshot(request(
                    Method::GET,
                    path,
                    Some(&format!("{SESSION_COOKIE}={session_id}")),
                    "",
                ))
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
        fs::remove_dir_all(root).unwrap_or_else(|error| panic!("{error}"));
    }
}
