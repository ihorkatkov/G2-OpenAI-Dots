"""Single-owner authorization code flow with a pre-registered confidential ChatGPT client."""

import base64
import hashlib
import hmac
import os
import re
import secrets
import time
from urllib.parse import urlencode

from flask import Blueprint, abort, jsonify, redirect, render_template_string, request

from storage import database

auth = Blueprint("auth", __name__)
scope = "glasses"
client_id = "g2-dot-link"


def origin():
    return os.environ["DOT_ORIGIN"].rstrip("/")


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def matches(value, expected):
    return isinstance(value, str) and hmac.compare_digest(
        digest(value), digest(expected)
    )


def issue(db, kind, ttl, challenge=None, redirect_uri=None):
    token = secrets.token_urlsafe(32)
    db.execute(
        "INSERT INTO grants VALUES(?,?,?,?,?)",
        (digest(token), kind, time.time() + ttl, challenge, redirect_uri),
    )
    return token


def authorized():
    token = request.headers.get("Authorization", "").removeprefix("Bearer ")
    if request.headers.get("Authorization") != "Bearer " + token:
        return False
    with database() as db:
        return bool(
            db.execute(
                "SELECT 1 FROM grants WHERE digest=? AND kind='access' AND expires>?",
                (digest(token), time.time()),
            ).fetchone()
        )


@auth.get("/.well-known/oauth-protected-resource")
@auth.get("/.well-known/oauth-protected-resource/mcp")
def resource_metadata():
    return jsonify(
        resource=origin() + "/mcp",
        authorization_servers=[origin()],
        scopes_supported=[scope],
    )


@auth.get("/.well-known/oauth-authorization-server")
def metadata():
    return jsonify(
        issuer=origin(),
        authorization_endpoint=origin() + "/authorize",
        token_endpoint=origin() + "/token",
        scopes_supported=[scope],
        response_types_supported=["code"],
        grant_types_supported=["authorization_code", "refresh_token"],
        code_challenge_methods_supported=["S256"],
        authorization_response_iss_parameter_supported=True,
        token_endpoint_auth_methods_supported=[
            "client_secret_basic",
            "client_secret_post",
        ],
    )


@auth.route("/authorize", methods=["GET", "POST"])
def authorize():
    q = request.args
    if (
        q.get("client_id") != client_id
        or q.get("response_type") != "code"
        or q.get("scope", scope) != scope
        or q.get("resource") != origin() + "/mcp"
        or q.get("code_challenge_method") != "S256"
        or not re.fullmatch(r"[A-Za-z0-9_-]{43}", q.get("code_challenge", ""))
        or not re.fullmatch(
            r"https://chatgpt\.com/(connector_platform_oauth_redirect|connector/oauth/[\w-]+)",
            q.get("redirect_uri", ""),
        )
    ):
        abort(400)
    if request.method == "GET":
        return render_template_string("""<!doctype html><title>Connect Dot to your glasses</title>
        <h1>Connect Dot to your glasses</h1><p>Allow your Dot to receive glasses questions and write replies.</p>
        <form method="post"><label>Owner pairing code <input type="password" name="pair" required></label>
        <button>Connect</button></form>""")
    if not matches(request.form.get("pair"), os.environ["DOT_PAIR"]):
        abort(403)
    with database() as db:
        code = issue(db, "code", 180, q["code_challenge"], q["redirect_uri"])
    return redirect(
        q["redirect_uri"]
        + "?"
        + urlencode({"code": code, "state": q.get("state", ""), "iss": origin()})
    )


@auth.post("/token")
def exchange():
    form = request.form
    basic = request.authorization
    client, secret = (
        (basic.username, basic.password)
        if basic and basic.type == "basic"
        else (form.get("client_id"), form.get("client_secret"))
    )
    if client != client_id or not matches(secret, os.environ["DOT_CLIENT_SECRET"]):
        return jsonify(error="invalid_client"), 401
    kind = {"authorization_code": "code", "refresh_token": "refresh"}.get(
        form.get("grant_type")
    )
    if not kind or form.get("resource", origin() + "/mcp") != origin() + "/mcp":
        return jsonify(error="invalid_grant"), 400
    token = form.get("code" if kind == "code" else "refresh_token", "")
    with database() as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute(
            "SELECT * FROM grants WHERE digest=? AND kind=? AND expires>?",
            (digest(token), kind, time.time()),
        ).fetchone()
        if not row:
            return jsonify(error="invalid_grant"), 400
        if kind == "code":
            verifier = form.get("code_verifier", "")
            if not re.fullmatch(r"[\w.~-]{43,128}", verifier, re.ASCII):
                return jsonify(error="invalid_grant"), 400
            challenge = (
                base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
                .decode()
                .rstrip("=")
            )
            if (
                not matches(challenge, row["challenge"])
                or form.get("redirect_uri") != row["redirect"]
            ):
                return jsonify(error="invalid_grant"), 400
        db.execute(
            "DELETE FROM grants WHERE digest=? OR expires<=?",
            (digest(token), time.time()),
        )
        return jsonify(
            access_token=issue(db, "access", 3600),
            refresh_token=issue(db, "refresh", 30 * 86400),
            expires_in=3600,
            token_type="Bearer",
            scope=scope,
        )
