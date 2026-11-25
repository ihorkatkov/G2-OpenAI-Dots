import base64
import hashlib
import json
import os
import secrets
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from service import EVENT, app, database, dispatch_one, init


class Exchange(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        config = patch.dict(
            os.environ,
            dict(
                DOT_DB=folder.name + "/db",
                DOT_ORIGIN="https://dot.example",
                DOT_OWNER_ID="owner",
                DOT_PAIR="pair",
                DOT_CLIENT_SECRET="client-secret",
                DOT_EVEN_TOKEN="glasses",
                DOT_WAIT_SECONDS="1",
            ),
        )
        config.start()
        self.addCleanup(config.stop)
        init()
        self.client = app.test_client()
        self.even_headers = {"Authorization": "Bearer glasses"}
        self.delivery = {
            "mode": "webhook",
            "url": "https://callback.example/hook",
            "secret": "whsec_" + base64.b64encode(secrets.token_bytes(32)).decode(),
        }
        self.params = {"name": EVENT, "delivery": self.delivery, "arguments": {}}
        verifier = secrets.token_urlsafe(48)
        query = dict(
            client_id="g2-dot-link",
            resource="https://dot.example/mcp",
            scope="glasses",
            state="abc",
            response_type="code",
            code_challenge_method="S256",
            redirect_uri="https://chatgpt.com/connector_platform_oauth_redirect",
            code_challenge=base64.urlsafe_b64encode(
                hashlib.sha256(verifier.encode()).digest()
            )
            .decode()
            .rstrip("="),
        )
        from urllib.parse import parse_qs, urlsplit

        self.assertEqual(
            self.client.post(
                "/authorize", query_string=query, data={"pair": "wrong"}
            ).status_code,
            403,
        )
        redirect = self.client.post(
            "/authorize", query_string=query, data={"pair": "pair"}
        ).headers["Location"]
        code = parse_qs(urlsplit(redirect).query)["code"][0]
        form = dict(
            client_id="g2-dot-link",
            client_secret="client-secret",
            grant_type="authorization_code",
            code=code,
            code_verifier=verifier,
            redirect_uri=query["redirect_uri"],
        )
        self.assertEqual(
            self.client.post(
                "/token", data={**form, "code_verifier": "x" * 43}
            ).status_code,
            400,
        )
        token = self.client.post("/token", data=form).json["access_token"]
        self.assertEqual(self.client.post("/token", data=form).status_code, 400)
        self.headers = {"Authorization": "Bearer " + token}

    def rpc(self, method, p):
        return self.client.post(
            "/mcp",
            headers=self.headers,
            json={"jsonrpc": "2.0", "id": 1, "method": method, "params": p},
        ).json

    def ask(self, text):
        return self.client.post(
            "/even",
            headers=self.even_headers,
            json={"model": "openclaw", "messages": [{"role": "user", "content": text}]},
        ).json["choices"][0]["message"]["content"]

    def test_exchange(self):
        self.assertEqual(
            self.client.post("/mcp", headers=self.even_headers).status_code, 401
        )
        self.assertEqual(
            self.client.post("/even", headers=self.headers).status_code, 401
        )
        self.assertIn("not connected", self.ask("hello"))

        def receiver(url, secret, sub, event_id, event):
            if "challenge" in event:
                return 200, json.dumps({"challenge": event["challenge"]}).encode()
            self.assertEqual(event["data"]["text"], "reverse MINT")
            self.question_id = event["data"]["question_id"]
            return 200, b""

        with patch("callbacks.deliver", side_effect=receiver):
            sub = self.rpc("events/subscribe", self.params)["result"]
            self.assertEqual(
                self.rpc("events/subscribe", self.params)["result"]["id"], sub["id"]
            )

            def answer():
                for _ in range(100):
                    if dispatch_one():
                        break
                    time.sleep(0.01)
                self.save("TNIM")

            t = threading.Thread(target=answer)
            t.start()
            self.assertEqual(self.ask("reverse MINT"), "TNIM")
            t.join()
            self.assertFalse(self.save("TNIM")["isError"])
            self.assertTrue(self.save("different")["isError"])
            init()
            self.assertEqual(self.ask("What did Dot say?"), "TNIM")
            self.rpc("events/unsubscribe", self.params)
            self.assertIn("not connected", self.ask("hello"))

    def save(self, answer):
        return self.rpc(
            "tools/call",
            {
                "name": "reply_to_glasses",
                "arguments": {"answer": answer, "question_id": self.question_id},
            },
        )["result"]

    def test_deferred_reply_and_retry(self):
        with patch(
            "callbacks.deliver",
            side_effect=lambda *args: (200, json.dumps(args[-1]).encode()),
        ):
            self.rpc("events/subscribe", self.params)
        with patch.dict(os.environ, DOT_WAIT_SECONDS="0"):
            self.assertIn("Question saved", self.ask("slow"))
        self.assertIn("not replied", self.ask("What did Dot say?"))
        with patch("callbacks.deliver", return_value=(503, b"")):
            dispatch_one()
        with database() as db:
            row = db.execute("SELECT * FROM deliveries").fetchone()
            self.assertEqual(row["attempts"], 1)
            self.question_id = row["question"]
            db.execute("UPDATE deliveries SET due=0")
        with patch("callbacks.deliver", return_value=(200, b"")):
            dispatch_one()
        self.save("Late reply")
        self.assertEqual(self.ask("What did Dot say?"), "Late reply")

    def test_overlapping_deferred_replies_survive_restart(self):
        with patch("callbacks.deliver", side_effect=lambda *args: (200, json.dumps(args[-1]).encode())):
            self.rpc("events/subscribe", self.params)
        with patch.dict(os.environ, DOT_WAIT_SECONDS="0"):
            for question in ("first", "second", "third"):
                self.assertIn("Question saved", self.ask(question))
        with database() as db:
            ids = {row["text"]: row["id"] for row in db.execute("SELECT id,text FROM questions")}
        for question in ("second", "first"):
            self.question_id = ids[question]
            self.save(question + " answer")
        init()
        self.assertEqual(self.ask("What did Dot say?"), "first answer")
        self.assertEqual(self.ask("What did DOT say?"), "second answer")
        self.assertIn("not replied", self.ask("What did Dot say?"))
        self.question_id = ids["third"]
        self.save("third answer")
        self.assertEqual(self.ask("What did Dot say?"), "third answer")

    def test_verification_and_destinations(self):
        with patch("callbacks.deliver", return_value=(200, b'{"challenge":"wrong"}')):
            self.assertEqual(
                self.rpc("events/subscribe", self.params)["error"]["code"], -32015
            )
        for url in ["http://example.com", "https://127.0.0.1/h", "https://[::1]/h"]:
            result = self.rpc(
                "events/subscribe",
                {**self.params, "delivery": {**self.delivery, "url": url}},
            )
            self.assertIn("error", result)
        self.assertIn("not connected", self.ask("hello"))


if __name__ == "__main__":
    unittest.main()
