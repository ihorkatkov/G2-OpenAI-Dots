use crate::*;
use axum::{
    Form,
    extract::{FromRequest, Multipart, Query, State},
    http::HeaderMap,
    response::Html,
};
use base64::engine::general_purpose::STANDARD;
use regex::Regex;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::{collections::HashMap, sync::LazyLock};

type Fields = HashMap<String, String>;

async fn form_fields(req: Request, app: &App) -> Result<Fields, Fault> {
    let media = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if media.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return Form::<Fields>::from_request(req, app)
            .await
            .map(|f| f.0)
            .map_err(|e| Fault(e.status(), "Invalid form body"));
    }
    let mut values = Fields::new();
    if media.eq_ignore_ascii_case("multipart/form-data") {
        let mut parts = Multipart::from_request(req, app)
            .await
            .map_err(|_| Fault::bad())?;
        while let Some(part) = parts
            .next_field()
            .await
            .map_err(|e| Fault(e.status(), "Invalid form body"))?
        {
            if part.file_name().is_none()
                && let Some(name) = part.name().map(str::to_owned)
            {
                let value = part
                    .text()
                    .await
                    .map_err(|e| Fault(e.status(), "Invalid form field"))?;
                values.entry(name).or_insert(value);
            }
        }
    }
    Ok(values)
}
const CLIENT: &str = "g2-dot-link";
const FORM: &str = include_str!("pair.html");
static REDIRECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://chatgpt\.com/(connector_platform_oauth_redirect|connector/oauth/[\w-]+)$")
        .unwrap()
});
fn field<'a>(fields: &'a Fields, name: &str) -> &'a str {
    fields.get(name).map_or("", String::as_str)
}
pub async fn resource(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"resource":format!("{}/mcp",app.config.origin),"authorization_servers":[app.config.origin],"scopes_supported":["glasses"]}),
    )
}
pub async fn metadata(State(app): State<App>) -> Json<Value> {
    let o = &app.config.origin;
    Json(json!({"issuer":o,
            "authorization_endpoint":format!("{o}/authorize"),
            "token_endpoint":format!("{o}/token"),
            "scopes_supported":["glasses"],
            "response_types_supported":["code"],
            "grant_types_supported":["authorization_code",
            "refresh_token"],
            "code_challenge_methods_supported":["S256"],
            "authorization_response_iss_parameter_supported":true,
            "token_endpoint_auth_methods_supported":["client_secret_basic",
            "client_secret_post"]}))
}
fn validate(app: &App, q: &Fields) -> Result<(), Fault> {
    let challenge = field(q, "code_challenge");
    if field(q, "client_id") == CLIENT
        && field(q, "response_type") == "code"
        && q.get("scope").is_none_or(|v| v == "glasses")
        && field(q, "resource") == format!("{}/mcp", app.config.origin)
        && field(q, "code_challenge_method") == "S256"
        && challenge.len() == 43
        && challenge
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        && REDIRECT.is_match(field(q, "redirect_uri"))
    {
        Ok(())
    } else {
        Err(Fault::bad())
    }
}
pub async fn pair_page(
    State(app): State<App>,
    Query(q): Query<Fields>,
) -> Result<Html<&'static str>, Fault> {
    validate(&app, &q)?;
    Ok(Html(FORM))
}
fn issue(
    db: &Connection,
    kind: &str,
    ttl: f64,
    challenge: Option<&str>,
    redirect: Option<&str>,
) -> rusqlite::Result<String> {
    let value = token();
    db.execute(
        "INSERT INTO grants VALUES(?,?,?,?,?)",
        params![digest(&value), kind, now() + ttl, challenge, redirect],
    )?;
    Ok(value)
}
pub async fn pair(
    State(app): State<App>,
    Query(q): Query<Fields>,
    req: Request,
) -> Result<Response, Fault> {
    validate(&app, &q)?;
    let browser = req
        .headers()
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    let form = form_fields(req, &app).await?;
    let submitted = field(&form, "pair");
    if !matches(submitted, &app.config.pair) && !matches(submitted.trim(), &app.config.pair) {
        if browser {
            let page = FORM.replace(r#"aria-describedby="hint""#, r#"aria-invalid="true" aria-describedby="hint feedback""#).replace(
                "<!--feedback-->",
                r#"<p id="feedback" role="alert">That code didn’t match. Copy DOT_PAIR again, then retry.</p>"#,
            );
            return Ok((StatusCode::FORBIDDEN, Html(page)).into_response());
        }
        return Err(Fault(StatusCode::FORBIDDEN, "Invalid pairing code"));
    }
    let mut redirect = url::Url::parse(field(&q, "redirect_uri")).map_err(|_| Fault::bad())?;
    let target = q["redirect_uri"].clone();
    let challenge = q["code_challenge"].clone();
    let code = app
        .db(move |db| issue(db, "code", 180., Some(&challenge), Some(&target)))
        .await?;
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", field(&q, "state"))
        .append_pair("iss", &app.config.origin);
    Ok((StatusCode::FOUND, [("location", redirect.as_str())]).into_response())
}
pub async fn authorized(app: &App, headers: &HeaderMap) -> Result<bool, Fault> {
    let Some(token) = bearer(headers) else {
        return Ok(false);
    };
    let key = digest(token);
    app.db(move |db| {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM grants WHERE digest=? AND kind='access' AND expires>?)",
            params![key, now()],
            |r| r.get(0),
        )
    })
    .await
}
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
pub async fn exchange(
    State(app): State<App>,
    headers: HeaderMap,
    req: Request,
) -> Result<Json<Value>, Fault> {
    let form = form_fields(req, &app).await?;
    let basic = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(kind, _)| kind.eq_ignore_ascii_case("basic"));
    let credentials = basic
        .and_then(|(_, s)| STANDARD.decode(s).ok())
        .and_then(|v| String::from_utf8(v).ok());
    let (client, secret) = if basic.is_some() {
        credentials
            .as_deref()
            .and_then(|s| s.split_once(':'))
            .unwrap_or(("", ""))
    } else {
        (field(&form, "client_id"), field(&form, "client_secret"))
    };
    if client != CLIENT || !matches(secret, &app.config.secret) {
        return Err(Fault(StatusCode::UNAUTHORIZED, "invalid_client"));
    }
    let invalid = || Fault(StatusCode::BAD_REQUEST, "invalid_grant");
    let (kind, key) = match field(&form, "grant_type") {
        "authorization_code" => ("code", "code"),
        "refresh_token" => ("refresh", "refresh_token"),
        _ => return Err(invalid()),
    };
    if form
        .get("resource")
        .is_some_and(|r| r != &format!("{}/mcp", app.config.origin))
    {
        return Err(invalid());
    }
    let key = digest(field(&form, key));
    app.db(move |db| {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = tx
            .query_row(
                "SELECT challenge,redirect FROM grants WHERE digest=? AND kind=? AND expires>?",
                params![key, kind, now()],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((challenge, redirect)) = row else {
            return Ok(None);
        };
        if kind == "code" {
            let verifier = field(&form, "code_verifier");
            if !(43..=128).contains(&verifier.len())
                || !verifier
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.~-".contains(&c))
                || !matches(
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                    challenge.as_deref().unwrap_or(""),
                )
                || redirect.as_ref() != form.get("redirect_uri")
            {
                return Ok(None);
            }
        }
        tx.execute(
            "DELETE FROM grants WHERE digest=? OR expires<=?",
            params![key, now()],
        )?;
        let response = json!({
            "access_token":issue(&tx,"access",3600.,None,None)?,
            "refresh_token":issue(&tx,"refresh",30.*86400.,None,None)?,
            "expires_in":3600, "token_type":"Bearer", "scope":"glasses"
        });
        tx.commit()?;
        Ok(Some(Json(response)))
    })
    .await?
    .ok_or_else(invalid)
}
