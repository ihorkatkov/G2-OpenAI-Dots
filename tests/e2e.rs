// Black-box tests. The only child process is the shipping executable.
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
const BIN: &str = env!("CARGO_BIN_EXE_g2-dot-link");
const ORIGIN: &str = "https://dot.example";
const REDIRECT: &str = "https://chatgpt.com/connector_platform_oauth_redirect";
const VERIFIER: &str = "opaque_verifier_123456789012345678901234567890123456789";
fn cmd(dir: &Path) -> Command {
    let mut c = Command::new(std::env::var_os("G2_TEST_BINARY").unwrap_or_else(|| BIN.into()));
    c.env_clear().current_dir(dir);
    c
}
struct Setup {
    dir: TempDir,
    vars: HashMap<String, String>,
}
impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let configure = cmd(dir.path())
            .args(["configure", ORIGIN])
            .output()
            .unwrap();
        assert!(configure.status.success());
        let file = fs::read_to_string(dir.path().join("service.env")).unwrap();
        let vars: HashMap<_, _> = file
            .lines()
            .map(|l| {
                let (k, v) = l.split_once('=').unwrap();
                (k.to_owned(), v.trim_matches('\'').to_owned())
            })
            .collect();
        assert_eq!(vars.len(), 6);
        for (f, mode) in [("service.env", 0o600), ("data", 0o700)] {
            assert_eq!(
                fs::metadata(dir.path().join(f))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                mode
            )
        }
        for k in [
            "DOT_OWNER_ID",
            "DOT_PAIR",
            "DOT_CLIENT_SECRET",
            "DOT_EVEN_TOKEN",
        ] {
            assert!(!String::from_utf8_lossy(&configure.stdout).contains(&vars[k]));
        }
        Self { dir, vars }
    }
    fn command(&self) -> Command {
        let mut c = cmd(self.dir.path());
        c.envs(&self.vars)
            .env("DOT_BIND", "127.0.0.1:0")
            .arg("serve");
        c
    }
    fn start(&self, wait: &str) -> Server {
        let file = tempfile::NamedTempFile::new_in(self.dir.path()).unwrap();
        let child = self
            .command()
            .env("DOT_LOG_LEVEL", "trace")
            .env("DOT_WAIT_SECONDS", wait)
            .stderr(file.reopen().unwrap())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Server {
            child,
            base: String::new(),
            log: file,
            http: Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            even: self.vars["DOT_EVEN_TOKEN"].clone(),
        };
        until(|| {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "startup failed: {}",
                server.logs()
            );
            server
                .logs()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find(|v| v["event"] == "listening")
                .is_some_and(|v| {
                    server.base = format!("http://127.0.0.1:{}", v["port"]);
                    true
                })
        });
        assert_eq!(
            server.get("/health").json::<Value>().unwrap(),
            json!({"status":"ok","version":"0.1.0"})
        );
        server
    }
}
fn until(mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(12), "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
struct Server {
    child: Child,
    base: String,
    log: tempfile::NamedTempFile,
    http: Client,
    even: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn logs(&self) -> String {
        fs::read_to_string(self.log.path()).unwrap()
    }
    fn get(&self, path: &str) -> Response {
        self.http
            .get(format!("{}{path}", self.base))
            .send()
            .unwrap()
    }
    fn post(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http.post(format!("{}{path}", self.base))
    }
    fn rpc(&self, token: &str, method: &str, params: Value) -> Value {
        let r = self
            .post("/mcp")
            .bearer_auth(token)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 200);
        let body = r.json::<Value>().unwrap();
        assert_eq!(body["id"], 1);
        body
    }
    fn save(&self, token: &str, id: &str, answer: &str) -> Value {
        self.rpc(
            token,
            "tools/call",
            json!({"name":"reply_to_glasses","arguments":{"question_id":id,"answer":answer}}),
        )["result"]
            .clone()
    }
    fn ask(&self, text: &str) -> Value {
        self.post("/even")
            .bearer_auth(&self.even)
            .json(&json!({"messages":[{"role":"user","content":text}]}))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }
    fn login(&self, setup: &Setup) -> String {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER));
        let query = [
            ("client_id", "g2-dot-link"),
            ("resource", "https://dot.example/mcp"),
            ("response_type", "code"),
            ("code_challenge_method", "S256"),
            ("redirect_uri", REDIRECT),
            ("code_challenge", &challenge),
            ("state", "sensitive-state"),
        ];
        assert!(
            self.http
                .get(format!("{}/authorize", self.base))
                .query(&query)
                .send()
                .unwrap()
                .text()
                .unwrap()
                .contains("Owner pairing code")
        );
        let pair = |s: &str| {
            self.post("/authorize")
                .query(&query)
                .form(&[("pair", s)])
                .send()
                .unwrap()
        };
        assert_eq!(pair("wrong").status(), 403);
        let r = pair(&format!(" {}\n", setup.vars["DOT_PAIR"]));
        assert_eq!(r.status(), 302);
        let u = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        let fields: HashMap<_, _> = u.query_pairs().collect();
        assert_eq!(fields["iss"], ORIGIN);
        assert_eq!(fields["state"], "sensitive-state");
        let code = &fields["code"];
        let exchange = |v: &str| {
            self.post("/token")
                .basic_auth("g2-dot-link", Some(&setup.vars["DOT_CLIENT_SECRET"]))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", code.as_ref()),
                    ("redirect_uri", REDIRECT),
                    ("code_verifier", v),
                ])
                .send()
                .unwrap()
        };
        assert_eq!(exchange(&"x".repeat(43)).status(), 400);
        let tokens: Value = exchange(VERIFIER)
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(exchange(VERIFIER).status(), 400);
        assert_eq!(tokens["expires_in"], 3600);
        assert_eq!(tokens["scope"], "glasses");
        assert_eq!(tokens["token_type"], "Bearer");
        let refresh = || {
            self.post("/token")
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", "g2-dot-link"),
                    ("client_secret", &setup.vars["DOT_CLIENT_SECRET"]),
                    ("refresh_token", tokens["refresh_token"].as_str().unwrap()),
                ])
                .send()
                .unwrap()
        };
        let rotated: Value = refresh().error_for_status().unwrap().json().unwrap();
        assert_eq!(refresh().status(), 400);
        assert_ne!(tokens["refresh_token"], rotated["refresh_token"]);
        assert_eq!(
            self.rpc(tokens["access_token"].as_str().unwrap(), "ping", json!({}))["result"],
            json!({})
        );
        self.check_logs(&[
            code,
            VERIFIER,
            tokens["access_token"].as_str().unwrap(),
            tokens["refresh_token"].as_str().unwrap(),
            rotated["refresh_token"].as_str().unwrap(),
        ]);
        rotated["access_token"].as_str().unwrap().to_owned()
    }
    fn check_logs(&self, secrets: &[&str]) {
        let logs = self.logs();
        for s in secrets {
            assert!(!logs.contains(s), "private value in logs")
        }
        for line in logs.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            assert!(v.as_object().unwrap().keys().all(|k| {
                [
                    "level",
                    "event",
                    "timestamp",
                    "port",
                    "status",
                    "attempt",
                    "elapsed_ms",
                    "message",
                ]
                .contains(&k.as_str())
            }));
        }
    }
}
fn content(v: &Value) -> &str {
    v["choices"][0]["message"]["content"].as_str().unwrap()
}
#[test]
fn configuration_and_startup_failures() {
    let s = Setup::new();
    let path = s.dir.path();
    let old = fs::read(path.join("service.env")).unwrap();
    let rerun = cmd(path).args(["configure", ORIGIN]).output().unwrap();
    assert!(!rerun.status.success());
    assert_eq!(fs::read(path.join("service.env")).unwrap(), old);
    for bad in [
        "http://example.com",
        "https://sensitive@example.com",
        "https://example.com/path",
        "https://example.com?secret",
    ] {
        assert!(
            !cmd(path)
                .args(["configure", bad])
                .output()
                .unwrap()
                .status
                .success()
        )
    }
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    for (k, v, reason) in [
        ("DOT_PAIR", "", "Missing DOT_PAIR"),
        ("DOT_BIND", "bad", "DOT_BIND"),
        ("DOT_WAIT_SECONDS", "NaN", "DOT_WAIT_SECONDS"),
        ("DOT_LOG_LEVEL", "unsafe-secret", "DOT_LOG_LEVEL"),
        ("DOT_DB", "missing/sub/db", "DOT_DB"),
        (
            "DOT_BIND",
            &occupied.local_addr().unwrap().to_string(),
            "Cannot listen",
        ),
    ] {
        let o = s.command().env(k, v).output().unwrap();
        assert!(!o.status.success());
        let v: Value = serde_json::from_slice(&o.stderr).unwrap();
        assert!(v["message"].as_str().unwrap().contains(reason));
        assert!(!v.to_string().contains("unsafe-secret"));
    }
    let server = s.start("0");
    assert_eq!(
        fs::metadata(&s.vars["DOT_DB"])
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let other = s.command().output().unwrap();
    assert!(!other.status.success());
    assert!(String::from_utf8_lossy(&other.stderr).contains("lock DOT_DB"));
    drop(server);
}
#[test]
fn protocol_and_migrated_inbox() {
    let s = Setup::new();
    let db = rusqlite::Connection::open(&s.vars["DOT_DB"]).unwrap();
    db.execute_batch(include_str!("fixtures/legacy.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO grants VALUES(?,'access',4102444800,NULL,NULL)",
        [hex::encode(Sha256::digest(b"legacy-access"))],
    )
    .unwrap();
    db.execute_batch("INSERT INTO questions VALUES('old','prior question',1,'prior reply'); INSERT INTO deferred_replies VALUES('old');").unwrap();
    drop(db);
    let server = s.start("0");
    let token = server.login(&s);
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        assert_eq!(
            server.get(path).json::<Value>().unwrap(),
            json!({"resource":"https://dot.example/mcp","scopes_supported":["glasses"],"authorization_servers":[ORIGIN]})
        );
    }
    let metadata = server
        .get("/.well-known/oauth-authorization-server")
        .json::<Value>()
        .unwrap();
    assert_eq!(
        metadata["token_endpoint_auth_methods_supported"],
        json!(["client_secret_basic", "client_secret_post"])
    );
    let unauthorized = server
        .post("/mcp")
        .bearer_auth(&server.even)
        .send()
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        unauthorized.headers()["www-authenticate"],
        "Bearer resource_metadata=\"https://dot.example/.well-known/oauth-protected-resource/mcp\""
    );
    assert_eq!(
        server
            .post("/even")
            .bearer_auth(&token)
            .send()
            .unwrap()
            .status(),
        401
    );
    let missing = server.get("/missing");
    assert_eq!(missing.status(), 404);
    for (k, v) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
    ] {
        assert_eq!(missing.headers()[k], v);
    }
    assert_eq!(
        server
            .post("/even")
            .bearer_auth(&server.even)
            .json(&json!({"messages":[],"padding":"a".repeat(32768)}))
            .send()
            .unwrap()
            .status(),
        413
    );
    assert_eq!(
        server
            .post("/even")
            .bearer_auth(&server.even)
            .json(&json!({"messages":[]}))
            .send()
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        server.rpc(
            "legacy-access",
            "initialize",
            json!({"protocolVersion":"2025-06-18"})
        )["result"]["protocolVersion"],
        "2025-06-18"
    );
    assert_eq!(
        server.rpc(&token, "initialize", json!({}))["result"]["protocolVersion"],
        "2025-11-25"
    );
    assert_eq!(
        server.rpc(&token, "tools/list", json!({}))["result"]["tools"][0]["_meta"]["openai/profile"],
        true
    );
    assert_eq!(
        server.rpc(&token, "events/list", json!({}))["result"]["events"][0]["payloadSchema"]["required"],
        json!(["question_id", "text"])
    );
    assert_eq!(
        server.rpc(&token, "tools/call", json!({"name":"get_profile"}))["result"]["structuredContent"]
            ["id"],
        s.vars["DOT_OWNER_ID"]
    );
    assert_eq!(
        server.rpc(&token, "unknown", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        server.rpc(&token, "ping", json!([]))["error"]["code"],
        -32602
    );
    assert_eq!(
        server
            .post("/mcp")
            .bearer_auth(&token)
            .json(&json!([]))
            .send()
            .unwrap()
            .status(),
        400
    );
    let note = server
        .post("/mcp")
        .bearer_auth(&token)
        .json(&json!({"jsonrpc":"2.0","method":"unknown"}))
        .send()
        .unwrap();
    assert_eq!(note.status(), 202);
    assert!(note.bytes().unwrap().is_empty());
    assert_eq!(content(&server.ask("What did Dot say?")), "prior reply");
    assert!(content(&server.ask("private question marker")).contains("not connected"));
    for (url, code) in [
        ("http://callback.example", -32602),
        ("https://127.0.0.1/private-path", -32015),
        ("https://[::1]/", -32015),
    ] {
        let p = json!({"name":"glasses.question","delivery":{"mode":"webhook","url":url,"secret":format!("whsec_{}",STANDARD.encode([1;32]))}});
        assert_eq!(
            server.rpc(&token, "events/subscribe", p)["error"]["code"],
            code
        );
    }
    server.check_logs(&[
        &server.even,
        &s.vars["DOT_PAIR"],
        &s.vars["DOT_CLIENT_SECRET"],
        &token,
        "legacy-access",
        "private-path",
        "private question marker",
        "prior reply",
        "sensitive-state",
    ]);
}

#[test]
#[ignore = "requires the isolated TLS network described in README"]
fn signed_webhook_round_trip_restart_and_retry() {
    use hmac::{Hmac, Mac};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    let ip = std::env::var("G2_TEST_CALLBACK_IP")
        .expect("set G2_TEST_CALLBACK_IP to the isolated test interface");
    let mut setup = Setup::new();
    let cert = rcgen::generate_simple_self_signed(vec!["dot-callback.test".into()]).unwrap();
    let pem = cert.cert.pem();
    let cert_path = setup.dir.path().join("receiver.pem");
    fs::write(&cert_path, &pem).unwrap();
    setup
        .vars
        .insert("SSL_CERT_FILE".into(), cert_path.to_str().unwrap().into());
    let receiver = tiny_http::Server::https(
        format!("{ip}:443"),
        tiny_http::SslConfig {
            certificate: pem.into_bytes(),
            private_key: cert.key_pair.serialize_pem().into_bytes(),
        },
    )
    .unwrap();
    let secret = format!("whsec_{}", STANDARD.encode([7; 32]));
    let url = "https://dot-callback.test/inbox?private-callback-marker";
    let sub = format!(
        "sub_{}",
        hex::encode(Sha256::digest(format!(
            "{}glasses.question{url}",
            setup.vars["DOT_OWNER_ID"]
        )))
    );
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let history = seen.clone();
    let expected = sub.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let thread = thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            let Some(mut req) = receiver.recv_timeout(Duration::from_millis(100)).unwrap() else {
                continue;
            };
            let mut raw = Vec::new();
            req.as_reader().read_to_end(&mut raw).unwrap();
            let header = |name: &'static str| {
                req.headers()
                    .iter()
                    .find(|h| h.field.equiv(name))
                    .unwrap()
                    .value
                    .as_str()
                    .to_owned()
            };
            let id = header("webhook-id");
            let ts = header("webhook-timestamp");
            let sig = header("webhook-signature");
            assert!(ts.parse::<u64>().is_ok());
            let mut mac = Hmac::<Sha256>::new_from_slice(&[7; 32]).unwrap();
            mac.update(format!("{id}.{ts}.").as_bytes());
            mac.update(&raw);
            mac.verify_slice(&STANDARD.decode(sig.strip_prefix("v1,").unwrap()).unwrap())
                .unwrap();
            let v: Value = serde_json::from_slice(&raw).unwrap();
            let (status, body) = if v["type"] == "verification" {
                let response = if req.url().contains("wrong-challenge") {
                    json!("incorrect")
                } else {
                    assert_eq!(header("x-mcp-subscription-id"), expected);
                    v["challenge"].clone()
                };
                (200, json!({"challenge":response}).to_string())
            } else {
                assert_eq!(header("x-mcp-subscription-id"), expected);
                assert_eq!(id, v["eventId"]);
                assert_eq!(id, v["data"]["question_id"]);
                assert_eq!(v["name"], "glasses.question");
                assert!(v["timestamp"].as_str().unwrap().ends_with("+00:00"));
                let mut received = history.lock().unwrap();
                let first = !received.iter().any(|old| old["eventId"] == id);
                let status = match v["data"]["text"].as_str().unwrap() {
                    "legacy queued" if first => 503,
                    "gone" => 410,
                    _ => 200,
                };
                received.push(v);
                (status, String::new())
            };
            req.respond(tiny_http::Response::from_string(body).with_status_code(status))
                .unwrap();
        }
    });
    let db = rusqlite::Connection::open(&setup.vars["DOT_DB"]).unwrap();
    db.execute_batch(include_str!("fixtures/legacy.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO subscriptions VALUES(?,?,?,4102444800)",
        rusqlite::params![sub, url, secret],
    )
    .unwrap();
    db.execute_batch("INSERT INTO questions VALUES('oldpending','legacy queued',1234,NULL);")
        .unwrap();
    db.execute("INSERT INTO deliveries VALUES('oldpending',?,0,0)", [&sub])
        .unwrap();
    drop(db);
    let first = setup.start("4");
    let token = first.login(&setup);
    until(|| first.logs().contains("\"status\":503"));
    drop(first);
    let server = setup.start("3");
    until(|| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|v| v["eventId"] == "oldpending")
            .count()
            == 2
    });
    let p = json!({"name":"glasses.question","arguments":{},"delivery":{"mode":"webhook","url":url,"secret":secret},"ttlMs":90000000});
    let mut wrong = p.clone();
    wrong["delivery"]["url"] = json!("https://dot-callback.test/wrong-challenge");
    assert_eq!(
        server.rpc(&token, "events/subscribe", wrong)["error"],
        json!({"code":-32015,"message":"CallbackEndpointError","data":{"reason":"challenge_failed"}})
    );
    for _ in 0..2 {
        let result = server.rpc(&token, "events/subscribe", p.clone());
        assert_eq!(result["result"]["id"], sub);
        assert!(
            result["result"]["refreshBefore"]
                .as_str()
                .unwrap()
                .ends_with("+00:00")
        );
    }
    let mut invalid = p.clone();
    invalid["ttlMs"] = json!(false);
    assert_eq!(
        server.rpc(&token, "events/subscribe", invalid)["error"]["code"],
        -32602
    );
    let text = "private question: reverse MINT";
    let answer = "字".repeat(350);
    thread::scope(|scope| {
        let request = scope.spawn(|| server.ask(text));
        until(|| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|v| v["data"]["text"] == text)
        });
        let events = seen.lock().unwrap();
        let id = events.iter().find(|v| v["data"]["text"] == text).unwrap()["eventId"]
            .as_str()
            .unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(server.save(&token, id, &answer)["isError"], false);
        let reply = request.join().unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(content(&reply), answer);
        assert_eq!(reply["object"], "chat.completion");
        assert_eq!(server.save(&token, id, &answer)["isError"], false);
        assert_eq!(server.save(&token, id, "conflict")["isError"], true);
    });
    thread::scope(|scope| {
        let request = scope.spawn(|| {
            cmd(setup.dir.path())
                .args(["ask", "What is seventeen plus twenty-six?"])
                .env("DOT_BIND", server.base.trim_start_matches("http://"))
                .output()
                .unwrap()
        });
        until(|| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|v| v["data"]["text"] == "What is seventeen plus twenty-six?")
        });
        let events = seen.lock().unwrap();
        let id = events
            .iter()
            .find(|v| v["data"]["text"] == "What is seventeen plus twenty-six?")
            .unwrap()["eventId"]
            .as_str()
            .unwrap();
        assert_eq!(server.save(&token, id, "43")["isError"], false);
        let answer = request.join().unwrap();
        assert!(answer.status.success());
        assert_eq!(String::from_utf8(answer.stdout).unwrap().trim(), "43");
    });
    // Drop a real socket after accepting the question. The timeout still records it
    // for retrieval even after a newer unanswered question has arrived.
    {
        use std::{io::Write, net::TcpStream};
        let addr = server.base.strip_prefix("http://").unwrap();
        let mut socket = TcpStream::connect(addr).unwrap();
        let body = json!({"messages":[{"role":"user","content":"disconnected phone"}]}).to_string();
        write!(socket, "POST /even HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", server.even, body.len()).unwrap();
        until(|| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|v| v["data"]["text"] == "disconnected phone")
        });
        drop(socket);
        assert!(content(&server.ask("newer unanswered")).contains("Question saved"));
        let old = seen
            .lock()
            .unwrap()
            .iter()
            .find(|v| v["data"]["text"] == "disconnected phone")
            .unwrap()["eventId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            server.save(&token, &old, "recovered reply")["isError"],
            false
        );
        assert_eq!(content(&server.ask("what did dot say?")), "recovered reply");
    }
    // All five waiting requests retain their permits while OAuth/MCP remain responsive.
    thread::scope(|scope| {
        let queries: Vec<_> = (0..5)
            .map(|i| {
                let srv = &server;
                scope.spawn(move || srv.ask(&format!("waiting {i}")))
            })
            .collect();
        until(|| {
            seen.lock()
                .unwrap()
                .iter()
                .filter(|v| v["data"]["text"].as_str().unwrap().starts_with("waiting"))
                .count()
                == 5
        });
        assert_eq!(
            server
                .post("/even")
                .bearer_auth(&server.even)
                .json(&json!({}))
                .send()
                .unwrap()
                .status(),
            429
        );
        for query in queries {
            assert!(content(&query.join().unwrap()).contains("Question saved"));
        }
    });
    server.check_logs(&[
        &secret,
        url,
        "private-callback-marker",
        text,
        &answer,
        &token,
    ]);
    drop(server);
    let server = setup.start("0");
    let first = server.ask("first");
    let second = server.ask("second");
    let _third = server.ask("third");
    assert!(content(&first).contains("Question saved"));
    assert!(content(&server.ask("What did Dot say?")).contains("not replied"));
    assert_eq!(
        server.save(&token, second["id"].as_str().unwrap(), "second reply")["isError"],
        false
    );
    assert_eq!(
        server.save(&token, first["id"].as_str().unwrap(), "first reply")["isError"],
        false
    );
    drop(server);
    let mut server = setup.start("0");
    assert_eq!(content(&server.ask("What did DOT say!")), "first reply");
    assert_eq!(content(&server.ask("what did dot say.")), "second reply");
    assert!(content(&server.ask("What did Dot say?")).contains("not replied"));
    assert_eq!(
        server.rpc(&token, "events/unsubscribe", p.clone())["result"],
        json!({})
    );
    assert!(content(&server.ask("hello")).contains("not connected"));
    assert_eq!(
        server.rpc(&token, "events/subscribe", p)["result"]["id"],
        sub
    );
    server.ask("gone");
    until(|| server.logs().contains("\"status\":410"));
    assert!(content(&server.ask("hello")).contains("not connected"));
    unsafe {
        libc::kill(server.child.id() as i32, libc::SIGTERM);
    }
    until(|| {
        server
            .child
            .try_wait()
            .unwrap()
            .is_some_and(|s| s.success())
    });
    assert!(server.logs().contains("\"shutdown\""));
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
}

#[test]
fn log_level_and_empty_inbox() {
    let setup = Setup::new();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    let child = setup
        .command()
        .env("DOT_BIND", port.to_string())
        .env("DOT_LOG_LEVEL", "error")
        .stderr(file.reopen().unwrap())
        .spawn()
        .unwrap();
    let mut server = Server {
        child,
        base: format!("http://{port}"),
        http: Client::builder().no_proxy().build().unwrap(),
        even: setup.vars["DOT_EVEN_TOKEN"].clone(),
        log: file,
    };
    until(|| {
        server
            .http
            .get(format!("{}/health", server.base))
            .send()
            .is_ok()
    });
    assert_eq!(
        content(&server.ask("what did dot say?")),
        "There are no glasses questions yet."
    );
    assert_eq!(server.post("/token").send().unwrap().status(), 401);
    assert_eq!(server.get("/missing").status(), 404);
    unsafe {
        libc::kill(server.child.id() as i32, libc::SIGTERM);
    }
    until(|| {
        server
            .child
            .try_wait()
            .unwrap()
            .is_some_and(|s| s.success())
    });
    assert!(server.logs().is_empty());
}

#[test]
fn binary_only_first_use_and_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("Wearer's inbox");
    fs::create_dir(&path).unwrap();
    let demo = cmd(&path).arg("demo").output().unwrap();
    assert!(demo.status.success());
    let output = String::from_utf8(demo.stdout).unwrap();
    assert!(output.contains("simulated Dot"));
    assert!(output.contains("Glasses: Three parts oil to one part vinegar."));
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);

    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    for args in [vec!["serve", ORIGIN], vec!["serve"]] {
        let child = cmd(&path)
            .args(args)
            .env("DOT_BIND", addr.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let server = Server {
            child,
            base: format!("http://{addr}"),
            log: tempfile::NamedTempFile::new().unwrap(),
            http: Client::new(),
            even: String::new(),
        };
        until(|| {
            server
                .http
                .get(format!("{}/health", server.base))
                .send()
                .is_ok()
        });
        let reply = cmd(&path)
            .args(["ask", "Hello"])
            .env("DOT_BIND", addr.to_string())
            .output()
            .unwrap();
        assert!(reply.status.success());
        assert!(
            String::from_utf8(reply.stdout)
                .unwrap()
                .contains("Dot is not connected.")
        );
    }
    let before = fs::read(path.join("service.env")).unwrap();
    let conflict = cmd(&path)
        .args(["serve", "https://other.example"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert_eq!(fs::read(path.join("service.env")).unwrap(), before);
}
