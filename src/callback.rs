use crate::*;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use rusqlite::{OptionalExtension, params};
use std::net::{IpAddr, SocketAddr};
use tokio::time::{sleep, timeout};
use url::{Host, Url};

pub const EVENT: &str = "glasses.question";
pub fn target(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    let authority = raw.split_once("://")?.1.split(['/', '?', '#']).next()?;
    (u.scheme() == "https"
        && u.has_host()
        && !authority.contains('@')
        && !raw.contains('\\')
        && !raw.chars().any(char::is_control)
        && u.port_or_known_default() == Some(443)
        && u.fragment().is_none())
    .then_some(u)
}
pub fn key(secret: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(secret.strip_prefix("whsec_")?)
        .ok()
        .filter(|b| (24..=64).contains(&b.len()))
}
fn public(ip: IpAddr) -> bool {
    if let IpAddr::V6(v) = ip {
        let s = v.segments();
        if v.to_ipv4().is_some()
            || s[0] == 0x2002
            || s[..2] == [0x2001, 0]
            || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
        {
            return false;
        }
    }
    rune_ip::is_global(ip) && !ip.is_multicast()
}
pub async fn deliver(
    raw: &str,
    secret: &str,
    sub: &str,
    event_id: &str,
    event: Value,
) -> Result<(u16, Vec<u8>), &'static str> {
    let outcome = timeout(Duration::from_secs(8), async {
        let u = target(raw).ok_or("Invalid callback URL")?;
        let (host, addresses) = match u.host().ok_or("Invalid callback URL")? {
            Host::Domain(h) => (
                h.to_string(),
                tokio::net::lookup_host((h, 443))
                    .await
                    .map_err(|_| "Cannot resolve callback hostname")?
                    .collect::<Vec<_>>(),
            ),
            Host::Ipv4(ip) => (ip.to_string(), vec![SocketAddr::new(ip.into(), 443)]),
            Host::Ipv6(ip) => (ip.to_string(), vec![SocketAddr::new(ip.into(), 443)]),
        };
        if addresses.is_empty() || !addresses.iter().all(|a| public(a.ip())) {
            return Err("Callback must resolve exclusively to public addresses");
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .resolve(&host, addresses[0])
            .build()
            .map_err(|_| "Cannot initialize callback TLS client")?;
        let body = serde_json::to_vec(&event).map_err(|_| "Cannot serialize event")?;
        let ts = (now() as i64).to_string();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&key(secret).ok_or("Invalid callback signing key")?)
                .map_err(|_| "Invalid callback signing key")?;
        mac.update(format!("{event_id}.{ts}.").as_bytes());
        mac.update(&body);
        let sig = STANDARD.encode(mac.finalize().into_bytes());
        let mut resp = client
            .post(u)
            .header("content-type", "application/json")
            .header("webhook-id", event_id)
            .header("webhook-timestamp", ts)
            .header("webhook-signature", format!("v1,{sig}"))
            .header("x-mcp-subscription-id", sub)
            .body(body)
            .send()
            .await
            .map_err(|_| "Callback HTTPS connection failed")?;
        let status = resp.status().as_u16();
        let mut bytes = Vec::new();
        while bytes.len() < 8192 {
            let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|_| "Cannot read callback response")?
            else {
                break;
            };
            bytes.extend_from_slice(&chunk[..chunk.len().min(8192 - bytes.len())]);
        }
        Ok((status, bytes))
    })
    .await
    .unwrap_or(Err("Callback deadline exceeded"));
    if let Err(reason) = outcome {
        log(Level::Warn, "callback_failed", json!({"message":reason}));
    }
    outcome
}
struct Delivery {
    question: String,
    text: String,
    created: f64,
    url: String,
    secret: String,
    sub: String,
    attempts: u32,
    expires: f64,
}
async fn dispatch(app: &App) -> Result<bool, Fault> {
    let t = now();
    let row = app
        .db(move |db| {
            db.query_row(
                "SELECT q.id,q.text,q.created,s.url,s.secret,s.id,d.attempts,s.expires
         FROM deliveries d JOIN questions q ON q.id=d.question
         JOIN subscriptions s ON s.id=d.subscription
         WHERE d.due<=? AND s.expires>? ORDER BY d.due LIMIT 1",
                params![t, t],
                |r| {
                    Ok(Delivery {
                        question: r.get(0)?,
                        text: r.get(1)?,
                        created: r.get(2)?,
                        url: r.get(3)?,
                        secret: r.get(4)?,
                        sub: r.get(5)?,
                        attempts: r.get(6)?,
                        expires: r.get(7)?,
                    })
                },
            )
            .optional()
        })
        .await?;
    let Some(d) = row else { return Ok(false) };
    let event = json!({"eventId":d.question,"name":EVENT,"timestamp":timestamp(d.created),"data":{"question_id":d.question,"text":d.text},"cursor":null});
    let status = deliver(&d.url, &d.secret, &d.sub, &d.question, event)
        .await
        .map_or(503, |r| r.0);
    app.db(move |db| {
        let tx = db.transaction()?;
        if (matches!(status, 408 | 429) || status >= 500) && d.attempts < 5 {
            let due = t + f64::from(2_u32.pow(d.attempts + 1));
            tx.execute(
                "UPDATE deliveries SET due=?,attempts=attempts+1
                        WHERE question=? AND subscription=?",
                params![due, d.question, d.sub],
            )?;
        } else {
            tx.execute(
                "DELETE FROM deliveries WHERE question=? AND subscription=?",
                params![d.question, d.sub],
            )?;
            if status == 410 {
                tx.execute(
                    "DELETE FROM subscriptions WHERE id=? AND expires=? AND secret=?",
                    params![d.sub, d.expires, d.secret],
                )?;
            }
        }
        tx.commit()
    })
    .await?;
    log(
        if status >= 300 {
            Level::Warn
        } else {
            Level::Info
        },
        "event_delivery",
        json!({"status":status,"attempt":d.attempts+1}),
    );
    Ok(true)
}
pub async fn worker(app: App) {
    loop {
        match dispatch(&app).await {
            Ok(true) => {}
            Ok(false) => sleep(Duration::from_millis(250)).await,
            Err(_) => sleep(Duration::from_secs(1)).await,
        }
    }
}
