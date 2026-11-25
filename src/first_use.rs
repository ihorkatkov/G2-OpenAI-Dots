use crate::{App, config::Config, digest, now, routes, token};
use reqwest::{Client, Response, redirect::Policy};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore};

pub struct Reply {
    id: String,
    pub text: String,
}

fn client(wait: Duration) -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(wait.saturating_add(Duration::from_secs(10)))
        .build()
        .map_err(|_| "Could not start local HTTP client".into())
}
async fn body(response: Response) -> Result<Value, String> {
    response
        .json()
        .await
        .map_err(|_| "The local inbox returned an unreadable response".into())
}

pub async fn ask(config: &Config, text: &str) -> Result<Reply, String> {
    if !(1..=4000).contains(&text.trim().chars().count()) {
        return Err("Use a question between 1 and 4000 characters".into());
    }
    let mut addr = config.bind;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        });
    }
    if !addr.ip().is_loopback() {
        // DOT_BIND can name a specific interface. Require it to belong to this host
        // before sending a bearer credential over plain HTTP.
        std::net::UdpSocket::bind(SocketAddr::new(addr.ip(), 0))
            .map_err(|_| "DOT_BIND must name a local interface to use ask")?;
    }
    let unavailable =
        "Cannot reach the inbox. Start g2-dot-link serve and run ask from the same directory.";
    let response = client(config.wait)?
        .post(format!("http://{addr}/even"))
        .bearer_auth(&config.even)
        .json(&json!({"messages":[{"role":"user","content":text}]}))
        .send()
        .await
        .map_err(|_| unavailable)?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 => "The running inbox rejected the token. Run ask from its service.env directory.",
            429 => "Five questions are already waiting. Try again when one finishes.",
            _ => "The inbox could not process the question. Check the service logs.",
        }
        .into());
    }
    let v = body(response).await?;
    let invalid = "The service returned an incomplete glasses reply";
    Ok(Reply {
        id: v["id"].as_str().ok_or(invalid)?.into(),
        text: v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or(invalid)?
            .into(),
    })
}

pub async fn demo() -> Result<(), String> {
    // The demo owns its listener, credentials and memory-only database. It never
    // starts the outbound dispatcher or touches an existing inbox.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "Cannot open local demo listener")?;
    let bind = listener
        .local_addr()
        .map_err(|_| "Cannot inspect demo listener")?;
    let config = Arc::new(Config {
        origin: "https://demo.invalid".into(),
        db: ":memory:".into(),
        owner: "demo".into(),
        pair: token(),
        secret: token(),
        even: token(),
        wait: Duration::ZERO,
        bind,
    });
    let access = token();
    let db = Connection::open_in_memory().map_err(|_| "Cannot open demo inbox")?;
    db.execute_batch(include_str!("schema.sql"))
        .and_then(|_| {
            db.execute(
                "INSERT INTO grants VALUES(?,'access',?,NULL,NULL)",
                params![digest(&access), now() + 60.],
            )?;
            db.execute(
                "INSERT INTO subscriptions VALUES('demo','https://demo.invalid',?,?)",
                params![token(), now() + 60.],
            )
        })
        .map_err(|_| "Cannot prepare demo inbox")?;
    let app = App {
        config: config.clone(),
        connection: Arc::new(Mutex::new(db)),
        slots: Arc::new(Semaphore::new(5)),
    };
    tokio::spawn(async move { axum::serve(listener, routes(app)).await });
    let question = "What’s a simple vinaigrette ratio?";
    println!("Local demo · simulated Dot\n\nYou: {question}");
    let saved = ask(&config, question).await?;
    println!("Glasses: {}", saved.text);
    let reply = json!({
        "jsonrpc":"2.0", "id":1, "method":"tools/call",
        "params":{
            "name":"reply_to_glasses",
            "arguments":{"question_id":saved.id, "answer":"Three parts oil to one part vinegar."}
        }
    });
    let response = client(Duration::ZERO)?
        .post(format!("http://{bind}/mcp"))
        .bearer_auth(access)
        .json(&reply)
        .send()
        .await
        .map_err(|_| "Cannot send demo reply")?;
    if body(response).await?["result"]["isError"] != false {
        return Err("Could not save demo reply".into());
    }
    println!("Dot’s reply is now in your inbox.\n\nYou: What did Dot say?");
    let answer = ask(&config, "What did Dot say?").await?;
    println!("Glasses: {}", answer.text);
    Ok(())
}
