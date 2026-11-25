mod auth;
mod callback;
mod config;
mod even;
mod first_use;
mod mcp;
mod plugin;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

const VERSION: &str = env!("CARGO_PKG_VERSION");
#[derive(Clone)]
struct App {
    config: Arc<config::Config>,
    connection: Arc<Mutex<Connection>>,
    slots: Arc<Semaphore>,
}
impl App {
    async fn db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, Fault> {
        let db = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.lock().map_err(|_| Fault::storage())?;
            f(&mut conn).map_err(|_| Fault::storage())
        })
        .await
        .map_err(|_| Fault::storage())?
    }
}
struct Fault(StatusCode, &'static str);
impl Fault {
    fn storage() -> Self {
        log(Level::Error, "storage_error", json!({}));
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database operation failed",
        )
    }
    fn bad() -> Self {
        Self(StatusCode::BAD_REQUEST, "Invalid request")
    }
}
impl IntoResponse for Fault {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
fn token() -> String {
    let mut b = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
fn digest(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}
fn matches(a: &str, b: &str) -> bool {
    Sha256::digest(a.as_bytes())
        .ct_eq(&Sha256::digest(b.as_bytes()))
        .into()
}
fn timestamp(t: f64) -> String {
    chrono::DateTime::from_timestamp_micros((t * 1e6).round() as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
static LEVEL: OnceLock<Level> = OnceLock::new();
fn log(level: Level, event: &'static str, fields: Value) {
    if level > *LEVEL.get().unwrap_or(&Level::Info) {
        return;
    }
    let name = match level {
        Level::Error => "error",
        Level::Warn => "warn",
        Level::Info => "info",
        Level::Debug => "debug",
        Level::Trace => "trace",
    };
    let mut record = fields;
    record["timestamp"] = json!(timestamp(now()));
    record["event"] = json!(event);
    record["level"] = json!(name);
    eprintln!("{record}");
}
async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let status = response.status().as_u16();
    log(
        if status >= 400 {
            Level::Warn
        } else {
            Level::Debug
        },
        "request_completed",
        json!({"status":status}),
    );
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    response
}
async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("signal registration");
    tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
    log(Level::Info, "shutdown", json!({}));
}
fn routes(app: App) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"ok","version":VERSION})) }),
        )
        .route("/even", post(even::handle))
        .route("/mcp", post(mcp::handle))
        .route("/.well-known/oauth-protected-resource", get(auth::resource))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(auth::resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(auth::metadata),
        )
        .route("/authorize", get(auth::pair_page).post(auth::pair))
        .route("/token", post(auth::exchange))
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn(headers))
        .with_state(app)
}
async fn serve(config: config::Config) -> Result<(), String> {
    let conn = Connection::open(&config.db).map_err(|_| "Cannot open SQLite database at DOT_DB")?;
    // SQLite owns the database file's locks. Elect the dispatcher on a separate inode.
    let mut lock_path = std::fs::canonicalize(&config.db)
        .map_err(|_| "Cannot resolve DOT_DB")?
        .into_os_string();
    lock_path.push(".lock");
    let dispatcher_lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| "Cannot open DOT_DB. Check its parent directory and permissions.")?;
    dispatcher_lock
        .try_lock()
        .map_err(|_| "Cannot lock DOT_DB. Check for another running service.")?;
    conn.busy_timeout(Duration::from_secs(10))
        .and_then(|_| conn.execute_batch("PRAGMA foreign_keys=OFF;"))
        .and_then(|_| conn.execute_batch(include_str!("schema.sql")))
        .map_err(
            |_| "Cannot initialize DOT_DB. Check SQLite format, disk space and permissions.",
        )?;
    let listener = tokio::net::TcpListener::bind(config.bind).await.map_err(
        |_| "Cannot listen on DOT_BIND. Check the address and whether the port is in use.",
    )?;
    let port = listener
        .local_addr()
        .map_err(|_| "Cannot inspect listener")?
        .port();
    let app = App {
        config: Arc::new(config),
        connection: Arc::new(Mutex::new(conn)),
        slots: Arc::new(Semaphore::new(5)),
    };
    let router = routes(app.clone());
    let slots = app.slots.clone();
    let worker = tokio::spawn(callback::worker(app));
    log(Level::Info, "listening", json!({"port":port}));
    tokio::select! {
        result=axum::serve(listener,router).with_graceful_shutdown(shutdown())=>{
            result.map_err(|_|"HTTP listener stopped unexpectedly")?;
            // Also drain question tasks whose clients disconnected before shutdown.
            let _permits = slots.acquire_many(5).await.map_err(|_| "Question queue closed")?;
            Ok(())
        },
        _=worker=>Err("Callback worker stopped unexpectedly".into())
    }
}
fn main() {
    // Set before creating threads so SQLite, its sidecars and configuration are owner-only.
    unsafe {
        libc::umask(0o077);
    }
    std::panic::set_hook(Box::new(|_| log(Level::Error, "internal_panic", json!({}))));
    if let Err(message) = run() {
        if std::env::args_os()
            .skip(1)
            .eq([std::ffi::OsString::from("serve")])
        {
            log(Level::Error, "command_failed", json!({"message":message}));
        } else {
            eprintln!("g2-dot-link: {message}");
        }
        std::process::exit(1)
    }
}
const HELP: &str = "G2 Dot Link — ask your personal Dot a question from your glasses.

  g2-dot-link demo                  Try a local question and reply
  g2-dot-link serve <HTTPS_ORIGIN>   Create and start your inbox
  g2-dot-link serve                 Restart from service.env or exported settings
  g2-dot-link ask \"<question>\"      Ask through the running inbox from this host
  g2-dot-link plugin                Create the ChatGPT upload ZIP for this inbox
  g2-dot-link configure <HTTPS_ORIGIN>   Create credentials without starting
  g2-dot-link --version             Print the version";

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);
    let mut settings = if matches!(command, Some("serve" | "ask" | "plugin")) {
        config::Settings::read()?
    } else {
        config::Settings::default()
    };
    let level = match settings
        .get("DOT_LOG_LEVEL")
        .as_deref()
        .unwrap_or("info")
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => Level::Error,
        "warn" => Level::Warn,
        "info" => Level::Info,
        "debug" => Level::Debug,
        "trace" => Level::Trace,
        _ => return Err("DOT_LOG_LEVEL must be error, warn, info, debug or trace".into()),
    };
    let _ = LEVEL.set(level);
    let runtime = || tokio::runtime::Runtime::new().map_err(|_| "Cannot start runtime");
    match args.as_slice() {
        [cmd, origin] if cmd == "configure" => {
            config::configure(origin)?;
            println!("Next: ./g2-dot-link serve");
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "serve" && rest.len() <= 1 => {
            if let Some(raw) = rest.first() {
                let origin = config::origin(raw)?;
                if settings.get("DOT_ORIGIN").is_none() {
                    config::configure(&origin)?;
                    settings = config::Settings::read()?;
                }
                let current = config::Config::load(&settings)?;
                if origin != current.origin {
                    return Err("This inbox uses another origin. Run g2-dot-link serve to keep it, or start in a new directory.".into());
                }
                println!("ChatGPT plugin URL: {origin}/mcp\nEven AI endpoint: {origin}/even");
                return runtime()?.block_on(serve(current));
            }
            runtime()?.block_on(serve(config::Config::load(&settings)?))
        }
        [cmd] if cmd == "plugin" => {
            let value = settings
                .get("DOT_ORIGIN")
                .ok_or("Run g2-dot-link plugin in the configured inbox directory")?;
            plugin::write(&config::origin(&value)?)
        }
        [cmd] if cmd == "demo" => runtime()?.block_on(first_use::demo()),
        [cmd, text] if cmd == "ask" => {
            let reply =
                runtime()?.block_on(first_use::ask(&config::Config::load(&settings)?, text))?;
            println!("{}", reply.text);
            Ok(())
        }
        [cmd] if cmd == "--version" => {
            println!("g2-dot-link {VERSION}");
            Ok(())
        }
        [] => {
            println!("{HELP}");
            Ok(())
        }
        [cmd] if matches!(cmd.as_str(), "help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(())
        }
        _ => Err("Unknown command or missing argument. Run g2-dot-link --help.".into()),
    }
}
