use std::{
    collections::HashMap,
    env,
    fs::{self, DirBuilder},
    io::Write,
    net::SocketAddr,
    os::unix::fs::DirBuilderExt,
    path::Path,
    time::Duration,
};
use url::Url;
pub struct Config {
    pub origin: String,
    pub db: String,
    pub owner: String,
    pub pair: String,
    pub secret: String,
    pub even: String,
    pub wait: Duration,
    pub bind: SocketAddr,
}
pub fn origin(raw: &str) -> Result<String, String> {
    let raw = raw.trim_end_matches('/');
    let valid = Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "https"
            && u.has_host()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
    }) && raw
        .split_once("://")
        .is_some_and(|(_, authority)| !authority.contains(['@', '/', '\\']))
        && !raw.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then(|| raw.to_owned()).ok_or_else(|| {
        "Expected an HTTPS origin without credentials, path, query or fragment".into()
    })
}
#[derive(Default)]
pub struct Settings(HashMap<String, String>);

impl Settings {
    pub fn read() -> Result<Self, String> {
        let text = match fs::read_to_string("service.env") {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(_) => {
                return Err("Cannot read service.env. Check its permissions and encoding.".into());
            }
        };
        let mut vars = HashMap::new();
        for entry in shlex::split(&text).ok_or("Unclosed quote in service.env")? {
            let (key, value) = entry
                .split_once('=')
                .ok_or("Use NAME=value assignments in service.env")?;
            vars.insert(key.to_owned(), value.to_owned());
        }
        Ok(Self(vars))
    }

    pub fn get(&self, key: &str) -> Option<String> {
        env::var(key).ok().or_else(|| self.0.get(key).cloned())
    }
}

impl Config {
    pub fn load(settings: &Settings) -> Result<Self, String> {
        let required = |key| {
            settings.get(key)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("Missing {key}. Start in your inbox directory or run g2-dot-link serve <HTTPS_ORIGIN> for a new inbox."))
        };
        let seconds = settings
            .get("DOT_WAIT_SECONDS")
            .unwrap_or_else(|| "90".into())
            .parse::<f64>()
            .ok()
            .and_then(|n| Duration::try_from_secs_f64(n).ok())
            .ok_or("DOT_WAIT_SECONDS must be a finite, nonnegative duration")?;
        Ok(Self {
            origin: origin(&required("DOT_ORIGIN")?)?,
            db: required("DOT_DB")?,
            owner: required("DOT_OWNER_ID")?,
            pair: required("DOT_PAIR")?,
            secret: required("DOT_CLIENT_SECRET")?,
            even: required("DOT_EVEN_TOKEN")?,
            wait: seconds,
            bind: settings
                .get("DOT_BIND")
                .unwrap_or_else(|| "127.0.0.1:8080".into())
                .parse()
                .map_err(|_| "DOT_BIND must be an IP address and port")?,
        })
    }
}
pub fn configure(raw: &str) -> Result<(), String> {
    let external = origin(raw)?;
    let dir = Path::new("data");
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(dir)
        .map_err(|_| "Cannot create private data directory")?;
    let db = dir
        .canonicalize()
        .map_err(|_| "Cannot resolve data directory")?
        .join("inbox.sqlite");
    let mut body = String::new();
    for (key, value) in [
        ("DOT_ORIGIN", external),
        (
            "DOT_DB",
            db.to_str().ok_or("Data path must be valid UTF-8")?.into(),
        ),
        ("DOT_OWNER_ID", crate::token()),
        ("DOT_PAIR", crate::token()),
        ("DOT_CLIENT_SECRET", crate::token()),
        ("DOT_EVEN_TOKEN", crate::token()),
    ] {
        let quoted = if value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c))
        {
            value
        } else {
            format!("'{}'", value.replace('\'', "'\"'\"'"))
        };
        body.push_str(&format!("{key}={quoted}\n"));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open("service.env")
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                "Existing service.env preserved. Edit it to change configuration."
            } else {
                "Cannot create service.env in this directory"
            }
        })?;
    if file
        .write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file("service.env");
        return Err("Cannot write service.env. Check available disk space.".into());
    }
    println!("Created service.env and data/ in this directory.");
    Ok(())
}
