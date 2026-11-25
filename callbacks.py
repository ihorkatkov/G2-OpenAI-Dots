"""Signed MCP Events webhooks. Resolve, validate and pin the address for each TLS connection."""

import base64
import hashlib
import hmac
import http.client
import ipaddress
import json
import socket
import ssl
import time
from urllib.parse import urlsplit


def validate_url(url):
    target = urlsplit(url)
    if (
        target.scheme != "https"
        or not target.hostname
        or target.username is not None
        or target.password is not None
        or target.port not in (None, 443)
        or target.fragment
    ):
        raise ValueError("Expected a public HTTPS callback")
    return target


def signing_key(secret):
    if not isinstance(secret, str) or not secret.startswith("whsec_"):
        raise ValueError("Invalid signing secret")
    key = base64.b64decode(secret[6:], validate=True)
    if not 24 <= len(key) <= 64:
        raise ValueError("Invalid signing key length")
    return key


def deliver(url, secret, sub_id, event_id, event):
    target = validate_url(url)
    addresses = socket.getaddrinfo(target.hostname, 443, type=socket.SOCK_STREAM)
    for _, _, _, _, endpoint in addresses:
        ip = ipaddress.ip_address(endpoint[0])
        if (
            not ip.is_global
            or ip.is_multicast
            or (
                ip.version == 6
                and (
                    ip.ipv4_mapped
                    or ip.sixtofour
                    or ip.teredo
                    or ip in ipaddress.ip_network("64:ff9b::/96")
                )
            )
        ):
            raise ValueError("Non-public callback")
    body = json.dumps(event, separators=(",", ":")).encode()
    ts = str(int(time.time()))
    sig = hmac.digest(
        signing_key(secret), f"{event_id}.{ts}.".encode() + body, hashlib.sha256
    )
    conn = http.client.HTTPSConnection(target.hostname, timeout=8)
    with socket.create_connection((addresses[0][4][0], 443), timeout=8) as raw:
        conn.sock = ssl.create_default_context().wrap_socket(
            raw, server_hostname=target.hostname
        )
        try:
            path = target.path or "/"
            if target.query:
                path += "?" + target.query
            conn.request(
                "POST",
                path,
                body,
                {
                    "Content-Type": "application/json",
                    "webhook-id": event_id,
                    "webhook-timestamp": ts,
                    "webhook-signature": "v1," + base64.b64encode(sig).decode(),
                    "X-MCP-Subscription-Id": sub_id,
                },
            )
            response = conn.getresponse()
            return response.status, response.read(8192)
        finally:
            conn.close()
