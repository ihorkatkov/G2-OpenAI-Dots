use crate::*;
use axum::extract::{FromRequest, State};
use callback::EVENT;
use rusqlite::params;
fn schema(names: &[&str]) -> Value {
    let props: serde_json::Map<_, _> = names
        .iter()
        .map(|s| (s.to_string(), json!({"type":"string"})))
        .collect();
    let mut v = json!({"type":"object","properties":props,"additionalProperties":false});
    if !names.is_empty() {
        v["required"] = json!(names)
    }
    v
}
fn invalid() -> Value {
    json!({"code":-32602,"message":"Invalid parameters"})
}
impl From<Fault> for Value {
    fn from(_: Fault) -> Self {
        json!({"code":-32603,"message":"Database operation failed"})
    }
}
async fn subscription(app: &App, method: &str, p: &Value) -> Result<Value, Value> {
    let d = &p["delivery"];
    if p["name"] != EVENT
        || p.get("arguments").is_some_and(|v| *v != json!({}))
        || d["mode"] != "webhook"
    {
        return Err(invalid());
    }
    let url = d["url"]
        .as_str()
        .filter(|s| callback::target(s).is_some())
        .ok_or_else(invalid)?
        .to_owned();
    let sub = format!(
        "sub_{}",
        digest(&format!("{}{EVENT}{url}", app.config.owner))
    );
    if method == "events/unsubscribe" {
        app.db(move |db| {
            let tx = db.transaction()?;
            tx.execute("DELETE FROM subscriptions WHERE id=?", [&sub])?;
            tx.execute("DELETE FROM deliveries WHERE subscription=?", [sub])?;
            tx.commit()
        })
        .await?;
        return Ok(json!({}));
    }
    let secret = d["secret"]
        .as_str()
        .filter(|s| callback::key(s).is_some())
        .ok_or_else(invalid)?
        .to_owned();
    let ttl = match &p["ttlMs"] {
        Value::Null => 86400000,
        v => v
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(invalid)?
            .min(86400000),
    };
    let expiry = now() + ttl as f64 / 1000.;
    let challenge = token();
    let verified = match callback::deliver(
        &url,
        &secret,
        &sub,
        &id(),
        json!({"type":"verification","challenge":challenge}),
    )
    .await
    {
        Ok((200..=299, body)) => serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v["challenge"].as_str().map(|s| matches(s, &challenge)))
            .unwrap_or(false),
        _ => false,
    };
    if !verified {
        return Err(
            json!({"code":-32015,"message":"CallbackEndpointError","data":{"reason":"challenge_failed"}}),
        );
    }
    let reply = json!({"id":sub,"refreshBefore":timestamp(expiry),"cursor":null,"truncated":false});
    app.db(move |db| {
        db.execute(
            "INSERT OR REPLACE INTO subscriptions VALUES(?,?,?,?)",
            params![sub, url, secret, expiry],
        )
    })
    .await?;
    Ok(reply)
}
async fn result(app: &App, method: &str, p: &Value) -> Result<Value, Value> {
    Ok(match method {
        "initialize" => {
            let version = p["protocolVersion"]
                .as_str()
                .filter(|v| matches!(*v, "2025-11-25" | "2025-06-18" | "2025-03-26"))
                .unwrap_or("2025-11-25");
            json!({"protocolVersion":version,"capabilities":{"events":{},"tools":{}},"serverInfo":{"name":"Glasses inbox","version":VERSION}})
        }
        "ping" => json!({}),
        "events/list" => {
            json!({"events":[{"name":EVENT,"description":"A question the owner asked through Even AI.","delivery":["webhook"],"inputSchema":schema(&[]),"payloadSchema":schema(&["question_id","text"])}]})
        }
        "events/subscribe" | "events/unsubscribe" => return subscription(app, method, p).await,
        "tools/list" => {
            json!({"tools":[
                {
                    "name":"get_profile", "description":"Identify the connected glasses owner.",
                    "inputSchema":schema(&[]), "outputSchema":schema(&["id"]),
                    "_meta":{"openai/profile":true}, "annotations":{"readOnlyHint":true}
                },
                {
                    "name":"reply_to_glasses",
                    "description":"Answer a glasses question by its question_id. Keep the answer within 350 characters.",
                    "inputSchema":schema(&["question_id","answer"]),
                    "annotations":{"readOnlyHint":false,"destructiveHint":false,
                                   "openWorldHint":false,"idempotentHint":true}
                }
            ]})
        }
        "tools/call" if p["name"] == "get_profile" => {
            let info = json!({"id":app.config.owner});
            json!({"structuredContent":info,"content":[{"type":"text","text":info.to_string()}]})
        }
        "tools/call" if p["name"] == "reply_to_glasses" => {
            let args = &p["arguments"];
            if p.get("arguments").is_some_and(|v| !v.is_object()) {
                return Err(invalid());
            }
            let answer = args["answer"]
                .as_str()
                .map(str::trim)
                .filter(|a| (1..=350).contains(&a.chars().count()))
                .ok_or_else(
                    || json!({"code":-32602,"message":"Answer must be 1 to 350 characters"}),
                )?
                .to_owned();
            let key = args["question_id"].as_str().map(str::to_string);
            let changed = app
                .db(move |db| {
                    db.execute(
                        "UPDATE questions SET answer=? WHERE id=? AND (answer IS NULL OR answer=?)",
                        params![answer, key, answer],
                    )
                })
                .await?;
            json!({"isError":changed==0,"content":[{"type":"text","text":if changed==0{"Unknown question or a different reply already exists"}else{"Saved"}}]})
        }
        _ => return Err(json!({"code":-32601,"message":"Unknown method or tool"})),
    })
}
pub async fn handle(State(app): State<App>, req: Request) -> Result<Response, Fault> {
    if !auth::authorized(&app, req.headers()).await? {
        return Ok((
            StatusCode::UNAUTHORIZED,
            [(
                "www-authenticate",
                format!(
                    "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                    app.config.origin
                ),
            )],
            Json(json!({"error":"unauthorized"})),
        )
            .into_response());
    }
    let Json(body) = Json::<Value>::from_request(req, &app)
        .await
        .map_err(|e| Fault(e.status(), "Invalid JSON request body"))?;
    if body["jsonrpc"] != "2.0" || !body["method"].is_string() {
        return Ok((StatusCode::BAD_REQUEST,Json(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}))).into_response());
    }
    let Some(id) = body.get("id") else {
        return Ok(StatusCode::ACCEPTED.into_response());
    };
    let empty = json!({});
    let p = body.get("params").unwrap_or(&empty);
    let response = if p.is_object() {
        result(&app, body["method"].as_str().unwrap(), p).await
    } else {
        Err(invalid())
    };
    let mut v = json!({"jsonrpc":"2.0","id":id});
    match response {
        Ok(value) => v["result"] = value,
        Err(error) => v["error"] = error,
    };
    Ok(Json(v).into_response())
}
