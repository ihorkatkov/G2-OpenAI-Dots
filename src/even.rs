use crate::*;
use axum::extract::{FromRequest, State};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use tokio::time::{Instant, sleep};
const SAVED: &str = r#"Question saved. Ask "What did Dot say?" to check for a reply."#;
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
    tokio::spawn(async move {
        let _permit = permit;
        question(app, text).await
    })
    .await
    .map_err(|_| Fault::storage())?
    .map(IntoResponse::into_response)
}
async fn question(app: App, text: String) -> Result<Json<Value>, Fault> {
    if text
        .strip_suffix(['?', '!', '.'])
        .unwrap_or(&text)
        .eq_ignore_ascii_case("what did dot say")
    {
        return app
            .db(|db| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let ready = tx
                    .query_row(
                        "SELECT q.id, q.answer FROM deferred_replies d
                 JOIN questions q ON q.id = d.question
                 WHERE q.answer IS NOT NULL ORDER BY q.created, q.id LIMIT 1",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                if let Some((key, text)) = ready {
                    tx.execute("DELETE FROM deferred_replies WHERE question=?", [&key])?;
                    tx.commit()?;
                    return Ok(answer(&text, Some(key)));
                }
                let latest = tx
                    .query_row(
                        "SELECT answer FROM questions ORDER BY created DESC LIMIT 1",
                        [],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?;
                let text = match &latest {
                    None => "There are no glasses questions yet.",
                    Some(None) => "Dot has not replied to your latest question yet.",
                    Some(Some(text)) => text,
                };
                Ok(answer(text, None))
            })
            .await;
    }
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
    if !queued {
        return Ok(answer(
            "Dot is not connected. Subscribe to glasses questions in ChatGPT first.",
            None,
        ));
    }
    let start = Instant::now();
    loop {
        let expired = start.elapsed() >= app.config.wait;
        let k = key.clone();
        let reply = app
            .db(move |db| {
                let tx = db.transaction_with_behavior(if expired {
                    TransactionBehavior::Immediate
                } else {
                    TransactionBehavior::Deferred
                })?;
                let reply = tx.query_row("SELECT answer FROM questions WHERE id=?", [&k], |r| {
                    r.get::<_, Option<String>>(0)
                })?;
                if expired && reply.is_none() {
                    tx.execute("INSERT INTO deferred_replies VALUES(?)", [k])?;
                }
                tx.commit()?;
                Ok(reply)
            })
            .await?;
        if let Some(text) = reply {
            log(
                Level::Info,
                "even_reply",
                json!({"elapsed_ms":start.elapsed().as_millis() as u64}),
            );
            return Ok(answer(&text, Some(key)));
        }
        if expired {
            return Ok(answer(SAVED, Some(key)));
        }
        sleep(Duration::from_millis(200).min(app.config.wait.saturating_sub(start.elapsed())))
            .await;
    }
}
