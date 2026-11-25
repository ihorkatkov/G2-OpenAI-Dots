use crate::*;
use axum::extract::{FromRequest, State};
use rusqlite::{TransactionBehavior, params};
use tokio::time::{Instant, sleep};
const SAVED: &str = "Question saved.";
const OFFLINE: &str = "Dot is not connected. Subscribe to glasses questions in ChatGPT first.";
fn answer(text: &str, key: Option<String>) -> Json<Value> {
    Json(
        json!({"id":key.unwrap_or_else(id),"object":"chat.completion","created":now() as i64,"model":"dot","choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}]}),
    )
}
pub async fn handle(State(app): State<App>, req: Request) -> Result<Response, Fault> {
    if !auth::bearer(req.headers()).is_some_and(|t| matches(t, &app.config.even)) {
        return Err(Fault(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let permit = app
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Fault(StatusCode::TOO_MANY_REQUESTS, "busy"))?;
    let body = match Json::<Value>::from_request(req, &app).await {
        Ok(Json(v)) => v,
        Err(e) => return Err(Fault(e.status(), "Invalid JSON request body")),
    };
    let msg = body["messages"]
        .as_array()
        .and_then(|v| v.last())
        .ok_or_else(Fault::bad)?;
    let text = msg["content"]
        .as_str()
        .map(str::trim)
        .ok_or_else(Fault::bad)?;
    if msg["role"] != "user" || !(1..=4000).contains(&text.chars().count()) {
        return Err(Fault::bad());
    }
    let text = text.to_owned();
    // A disconnected phone still needs its question moved into the late-reply queue.
    let (client, _connected) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _permit = permit;
        question(app, text, Arc::new(client)).await
    })
    .await
    .map_err(|_| Fault::storage())?
    .map(IntoResponse::into_response)
}
async fn question(
    app: App,
    text: String,
    client: Arc<tokio::sync::oneshot::Sender<()>>,
) -> Result<Json<Value>, Fault> {
    let key = id();
    let question_id = key.clone();
    let queued = app
        .db(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let created = now();
            let count = tx.execute(
                "INSERT INTO deliveries(question,subscription,due)
             SELECT ?,id,? FROM subscriptions WHERE expires>?",
                params![question_id, created, created],
            )?;
            if count == 0 {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO questions VALUES(?,?,?,NULL)",
                params![question_id, text, created],
            )?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    let start = Instant::now();
    loop {
        let expired = start.elapsed() >= app.config.wait;
        let current = key.clone();
        let client = client.clone();
        let result = app
            .db(move |db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                // A cancelled long poll still owns its question, but should not take
                // previously saved replies away from the next connected request.
                if client.is_closed() {
                    if queued {
                        tx.execute(
                            "INSERT OR IGNORE INTO deferred_replies VALUES(?)",
                            [&current],
                        )?;
                    }
                    tx.commit()?;
                    return Ok(Some((String::new(), false)));
                }
                let direct: Option<String> = if queued {
                    tx.query_row("SELECT answer FROM questions WHERE id=?", [&current], |r| {
                        r.get(0)
                    })?
                } else {
                    None
                };
                let mut ready = tx.prepare(
                    "SELECT q.answer FROM deferred_replies d JOIN questions q ON q.id=d.question
                     WHERE q.answer IS NOT NULL ORDER BY q.created,q.id"
                )?;
                let earlier = ready.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(ready);
                if queued && direct.is_none() && earlier.is_empty() && !expired {
                    return Ok(None);
                }
                if !earlier.is_empty() {
                    tx.execute(
                        "DELETE FROM deferred_replies WHERE EXISTS
                         (SELECT 1 FROM questions q WHERE q.id=deferred_replies.question AND q.answer IS NOT NULL)",
                        [],
                    )?;
                }
                if queued && direct.is_none() {
                    tx.execute("INSERT INTO deferred_replies VALUES(?)", [&current])?;
                }
                tx.commit()?;
                let latest = direct
                    .as_deref()
                    .unwrap_or(if queued { SAVED } else { OFFLINE });
                let message = if earlier.is_empty() {
                    latest.to_owned()
                } else {
                    let saved = earlier.join("\n\n");
                    if direct.is_some() {format!("Earlier replies:\n{saved}\n\nCurrent reply: {latest}")} else {format!("Earlier replies:\n{saved}\n\n{latest}")}
                };
                Ok(Some((message, direct.is_some())))
            })
            .await?;
        if let Some((message, direct)) = result {
            if direct {
                log(
                    Level::Info,
                    "even_reply",
                    json!({"elapsed_ms":start.elapsed().as_millis() as u64}),
                );
            }
            return Ok(answer(&message, Some(key)));
        }
        sleep(Duration::from_millis(200).min(app.config.wait.saturating_sub(start.elapsed())))
            .await;
    }
}
