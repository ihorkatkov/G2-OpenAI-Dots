import datetime as dt
import http.client
import hashlib
import json
import logging
import os
import re
import secrets
import threading
import time
import uuid

from flask import Flask, abort, jsonify, request

import callbacks
from auth import auth, authorized, matches, origin
from storage import database, init

app = Flask(__name__)
app.config["MAX_CONTENT_LENGTH"] = 32 * 1024
app.register_blueprint(auth)
EVENT = "glasses.question"
EMPTY = {"type": "object", "properties": {}, "additionalProperties": False}


def strings(*names):
    return {
        "type": "object",
        "properties": {k: {"type": "string"} for k in names},
        "required": list(names),
        "additionalProperties": False,
    }


def timestamp(t):
    return dt.datetime.fromtimestamp(t, dt.timezone.utc).isoformat()


def wire_answer(text, identifier=None):
    return jsonify(
        id=identifier or uuid.uuid4().hex,
        object="chat.completion",
        created=int(time.time()),
        model="dot",
        choices=[
            {
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }
        ],
    )


@app.after_request
def no_store(response):
    response.headers.update(
        {
            "Cache-Control": "no-store",
            "Referrer-Policy": "no-referrer",
            "X-Frame-Options": "DENY",
            "X-Content-Type-Options": "nosniff",
        }
    )
    return response


@app.get("/health")
def health():
    return jsonify(status="ok", version="0.1.0")


waiting_clients = threading.BoundedSemaphore(5)


@app.post("/even")
def even():
    if not matches(
        request.headers.get("Authorization"), "Bearer " + os.environ["DOT_EVEN_TOKEN"]
    ):
        abort(401)
    if not waiting_clients.acquire(blocking=False):
        return jsonify(error="busy"), 429
    try:
        return handle_question()
    finally:
        waiting_clients.release()


def handle_question():
    body = request.get_json()
    messages = body.get("messages") if isinstance(body, dict) else None
    if (
        not isinstance(messages, list)
        or not messages
        or not isinstance(messages[-1], dict)
    ):
        abort(400)
    text = messages[-1].get("content")
    if (
        messages[-1].get("role") != "user"
        or not isinstance(text, str)
        or not 1 <= len(text.strip()) <= 4000
    ):
        abort(400)
    text = text.strip()
    with database() as db:
        if re.fullmatch(r"what did dot say[?!.]?", text, re.I):
            db.execute("BEGIN IMMEDIATE")
            ready = db.execute("""
                SELECT q.id, q.answer FROM deferred_replies AS d
                JOIN questions AS q ON q.id=d.question
                WHERE q.answer IS NOT NULL ORDER BY q.created, q.id LIMIT 1
            """).fetchone()
            if ready:
                db.execute("DELETE FROM deferred_replies WHERE question=?", (ready["id"],))
                return wire_answer(ready["answer"], ready["id"])
            row = db.execute(
                "SELECT answer FROM questions ORDER BY created DESC LIMIT 1"
            ).fetchone()
            if not row:
                return wire_answer("There are no glasses questions yet.")
            return wire_answer(
                row["answer"] or "Dot has not replied to your latest question yet."
            )
        db.execute("BEGIN IMMEDIATE")
        subs = db.execute(
            "SELECT id FROM subscriptions WHERE expires>?", (time.time(),)
        ).fetchall()
        if not subs:
            return wire_answer(
                "Dot is not connected. Subscribe to glasses questions in ChatGPT first."
            )
        key = uuid.uuid4().hex
        now = time.time()
        db.execute("INSERT INTO questions VALUES(?,?,?,NULL)", (key, text, now))
        db.executemany(
            "INSERT INTO deliveries(question,subscription,due) VALUES(?,?,?)",
            [(key, s["id"], now) for s in subs],
        )
    start = time.monotonic()
    while time.monotonic() - start < float(os.environ.get("DOT_WAIT_SECONDS", "90")):
        with database() as db:
            answer = db.execute(
                "SELECT answer FROM questions WHERE id=?", (key,)
            ).fetchone()[0]
        if answer:
            app.logger.info(
                "even_reply elapsed_ms=%d", 1000 * (time.monotonic() - start)
            )
            return wire_answer(answer, key)
        time.sleep(0.2)
    # Decide at the timeout boundary atomically with respect to the reply writer.
    with database() as db:
        db.execute("BEGIN IMMEDIATE")
        answer = db.execute("SELECT answer FROM questions WHERE id=?", (key,)).fetchone()[0]
        if answer:
            return wire_answer(answer, key)
        db.execute("INSERT INTO deferred_replies(question) VALUES(?)", (key,))
    return wire_answer(
        'Question saved. Ask "What did Dot say?" to check for a reply.', key
    )


class RpcError(Exception):
    def __init__(self, code, message, data=None):
        self.error = {"code": code, "message": message}
        if data:
            self.error["data"] = data


TOOLS = [
    {
        "name": "get_profile",
        "description": "Identify the connected glasses owner.",
        "inputSchema": EMPTY,
        "outputSchema": strings("id"),
        "_meta": {"openai/profile": True},
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "reply_to_glasses",
        "description": "Answer a glasses question by its question_id. Keep the answer within 350 characters.",
        "inputSchema": strings("question_id", "answer"),
        "annotations": {
            "readOnlyHint": False,
            "destructiveHint": False,
            "openWorldHint": False,
            "idempotentHint": True,
        },
    },
]


def subscription(method, p):
    delivery = p.get("delivery")
    if (
        p.get("name") != EVENT
        or p.get("arguments", {}) != {}
        or not isinstance(delivery, dict)
        or delivery.get("mode") != "webhook"
    ):
        raise ValueError()
    url = delivery.get("url", "")
    callbacks.validate_url(url)
    sub_id = (
        "sub_"
        + hashlib.sha256(
            (os.environ["DOT_OWNER_ID"] + EVENT + url).encode()
        ).hexdigest()
    )
    with database() as db:
        if method == "events/unsubscribe":
            db.execute("DELETE FROM subscriptions WHERE id=?", (sub_id,))
            db.execute("DELETE FROM deliveries WHERE subscription=?", (sub_id,))
            return {}
        secret = delivery.get("secret")
        callbacks.signing_key(secret)
        ttl = p.get("ttlMs")
        if ttl is not None and (type(ttl) != int or ttl <= 0):
            raise ValueError()
        expiry = time.time() + min(ttl / 1000 if ttl else 86400, 86400)
        challenge = secrets.token_urlsafe(32)
        try:
            status, body = callbacks.deliver(
                url,
                secret,
                sub_id,
                uuid.uuid4().hex,
                {"type": "verification", "challenge": challenge},
            )
            if not 200 <= status < 300 or not matches(
                json.loads(body).get("challenge"), challenge
            ):
                raise ValueError()
        except (
            ValueError,
            OSError,
            AttributeError,
            TypeError,
            http.client.HTTPException,
        ) as err:
            raise RpcError(
                -32015, "CallbackEndpointError", {"reason": "challenge_failed"}
            ) from err
        db.execute(
            "INSERT OR REPLACE INTO subscriptions VALUES(?,?,?,?)",
            (sub_id, url, secret, expiry),
        )
        return {
            "id": sub_id,
            "refreshBefore": timestamp(expiry),
            "cursor": None,
            "truncated": False,
        }


def result(method, p):
    if method == "initialize":
        supported = ("2025-11-25", "2025-06-18", "2025-03-26")
        version = p.get("protocolVersion")
        return {
            "protocolVersion": version if version in supported else supported[0],
            "capabilities": {"events": {}, "tools": {}},
            "serverInfo": {"name": "Glasses inbox", "version": "0.1.0"},
        }
    if method == "ping":
        return {}
    if method == "events/list":
        return {
            "events": [
                {
                    "name": EVENT,
                    "description": "A question the owner asked through Even AI.",
                    "delivery": ["webhook"],
                    "inputSchema": EMPTY,
                    "payloadSchema": strings("question_id", "text"),
                }
            ]
        }
    if method in ("events/subscribe", "events/unsubscribe"):
        return subscription(method, p)
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        name = p.get("name")
        if name == "get_profile":
            info = {"id": os.environ["DOT_OWNER_ID"]}
            return {
                "structuredContent": info,
                "content": [{"type": "text", "text": json.dumps(info)}],
            }
        if name == "reply_to_glasses":
            args = p.get("arguments", {})
            answer = args.get("answer")
            if not isinstance(answer, str) or not 1 <= len(answer.strip()) <= 350:
                raise RpcError(-32602, "Answer must be 1 to 350 characters")
            answer = answer.strip()
            with database() as db:
                changed = db.execute(
                    "UPDATE questions SET answer=? WHERE id=? AND (answer IS NULL OR answer=?)",
                    (answer, args.get("question_id"), answer),
                ).rowcount
            return {
                "isError": not changed,
                "content": [
                    {
                        "type": "text",
                        "text": "Saved"
                        if changed
                        else "Unknown question or a different reply already exists",
                    }
                ],
            }
    raise RpcError(-32601, "Unknown method or tool")


@app.post("/mcp")
def mcp():
    if not authorized():
        return (
            jsonify(error="unauthorized"),
            401,
            {
                "WWW-Authenticate": f'Bearer resource_metadata="{origin()}/.well-known/oauth-protected-resource/mcp"'
            },
        )
    body = request.get_json()
    if (
        not isinstance(body, dict)
        or body.get("jsonrpc") != "2.0"
        or not isinstance(body.get("method"), str)
    ):
        return jsonify(
            jsonrpc="2.0", id=None, error={"code": -32600, "message": "Invalid request"}
        ), 400
    if "id" not in body:
        return "", 202
    try:
        params = body.get("params", {})
        if not isinstance(params, dict):
            raise ValueError()
        return jsonify(
            jsonrpc="2.0", id=body["id"], result=result(body["method"], params)
        )
    except RpcError as err:
        error = err.error
    except (ValueError, TypeError, AttributeError):
        error = {"code": -32602, "message": "Invalid parameters"}
    return jsonify(jsonrpc="2.0", id=body["id"], error=error)


def dispatch_one():
    now = time.time()
    with database() as db:
        row = db.execute(
            """SELECT q.id,q.text,q.created,s.url,s.secret,s.id AS sub,d.attempts
                          FROM deliveries d JOIN questions q ON d.question=q.id JOIN subscriptions s ON s.id=d.subscription
                          WHERE d.due<=? AND s.expires>? ORDER BY d.due LIMIT 1""",
            (now, now),
        ).fetchone()
    if not row:
        return False
    event = {
        "eventId": row["id"],
        "name": EVENT,
        "timestamp": timestamp(row["created"]),
        "data": {"question_id": row["id"], "text": row["text"]},
        "cursor": None,
    }
    try:
        status, _ = callbacks.deliver(
            row["url"], row["secret"], row["sub"], row["id"], event
        )
    except (OSError, ValueError, http.client.HTTPException):
        status = 503
    with database() as db:
        args = (row["id"], row["sub"])
        if (status in (408, 429) or status >= 500) and row["attempts"] < 5:
            db.execute(
                "UPDATE deliveries SET due=?, attempts=attempts+1 WHERE question=? AND subscription=?",
                (now + 2 ** (row["attempts"] + 1), *args),
            )
        else:
            db.execute(
                "DELETE FROM deliveries WHERE question=? AND subscription=?", args
            )
            if status == 410:
                db.execute(
                    "DELETE FROM subscriptions WHERE id=? AND url=?",
                    (row["sub"], row["url"]),
                )
    app.logger.info("event_delivery status=%d attempt=%d", status, row["attempts"] + 1)
    return True


def worker():
    while True:
        try:
            if not dispatch_one():
                time.sleep(0.25)
        except Exception:
            app.logger.error("event_worker_error")
            time.sleep(1)


def start():
    for key in (
        "DOT_ORIGIN",
        "DOT_DB",
        "DOT_OWNER_ID",
        "DOT_PAIR",
        "DOT_CLIENT_SECRET",
        "DOT_EVEN_TOKEN",
    ):
        if not os.environ.get(key):
            raise RuntimeError("Missing " + key)
    init()
    app.logger.setLevel(logging.INFO)
    threading.Thread(target=worker, daemon=True).start()
    return app
