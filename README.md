# G2 Dot Link

Keep your phone in your pocket. Ask your personal assistant in ChatGPT (your Dot) a question through Even AI and read the answer on your Even Realities G2 lenses. Your always-on host keeps the inbox available when your laptop is off.

## Try it with one command

Put the release executable for your platform in an empty directory, name it `g2-dot-link`, and open a terminal there. If you need to build a copy, see [Build from source](#build-from-source).

```sh
./g2-dot-link demo
```

The local demo uses a simulated Dot to show what happens when an answer arrives late. It runs offline, needs no account, and leaves the directory untouched.

```text
You: What’s a simple vinaigrette ratio?
Glasses: Question saved. Ask "What did Dot say?" to check for a reply.
Dot’s reply is now in your inbox.

You: What did Dot say?
Glasses: Three parts oil to one part vinegar.
```

On your glasses, an answer that arrives within 90 seconds appears on the original request. For later answers, ask **“What did Dot say?”** again to collect each ready reply in question order.

## Connect your own glasses

You’ll need an always-on macOS or Linux host with persistent disk, a public HTTPS domain pointing to it, a personal Dot with custom MCP plugin access, and **Agent Configuration** in the Even phone app. This inbox serves one wearer.

**1. Start the inbox.** In the directory where you want to keep it, replace the example domain with yours.

```sh
./g2-dot-link serve https://glasses.example.com
```

Leave it running. The command creates private credentials in `service.env`, stores the inbox in `data/`, and prints the two URLs you’ll use below. You can rerun the same command after a restart. For always-on use, run it under your service manager. [Hosting guide](#hosting-reference)

Point your HTTPS reverse proxy at `127.0.0.1:8080`. With Caddy, use the following site block. Allow inbound HTTPS on the host and outbound HTTPS to Dot. Set any upstream response timeout to at least 100 seconds.

```caddyfile
glasses.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Open `https://glasses.example.com/health` in a browser and look for `"status":"ok"`. In a second terminal **in the same directory**, check the glasses endpoint with the binary:

```sh
./g2-dot-link ask "Hello"
```

A fresh inbox answers **“Dot is not connected. Subscribe to glasses questions in ChatGPT first.”** That means the local service accepted your glasses token and is ready to pair.

**2. Connect ChatGPT.** Open **Plugins → Add → Add custom MCP server**. Name it **Glasses inbox** and paste the **ChatGPT plugin URL** printed at startup. Under **OAuth → Advanced OAuth settings → User-Defined OAuth Client**, enter:

| ChatGPT field | Value |
| --- | --- |
| Client ID | `g2-dot-link` |
| Client secret | Copy `DOT_CLIENT_SECRET` from your private `service.env` |
| Token endpoint authentication | `client_secret_basic` |
| Scope, if requested | `glasses` |

Click **Connect**. On the pairing page, paste `DOT_PAIR` from the same file. Then paste this instruction into your **personal Dot conversation**:

> Subscribe to the Glasses inbox glasses.question event. For each event, answer its text in at most 350 plain-text characters and call reply_to_glasses with the event’s question_id and your answer.

Wait for Dot to confirm the subscription. Run `./g2-dot-link ask "What is seventeen plus twenty-six?"` to check the live connection. Expect **43**, or use `./g2-dot-link ask "What did Dot say?"` to collect a late reply.

**3. Connect Even AI.** On your phone, open **Settings → Even AI → Agent Configuration**. Name the agent **Dot**, paste the **Even AI endpoint** printed at startup, and enter `DOT_EVEN_TOKEN` as the token. A private password manager shared with your phone makes the token easier to paste. Save, activate, and ask the same question on your glasses.

## If you get stuck

| What you see | What to check |
| --- | --- |
| Permission denied when starting the binary | Run `chmod +x ./g2-dot-link`, then retry. |
| Public health check fails or returns 502 | Check public DNS, TLS, the proxy upstream, and whether `serve` is running. |
| Pairing page shows “Invalid request” | In ChatGPT, check the plugin URL printed at startup and the client ID `g2-dot-link`, then start Connect again. |
| Pairing code rejected | Use the value after `DOT_PAIR=` from the running inbox’s file. |
| Dot is disconnected | Ask your personal Dot to subscribe again. Check that the plugin is connected. |
| “Question saved” on every request | Ask for the late reply. Check the proxy timeout and whether Dot called `reply_to_glasses`. |
| The terminal gets answers but Even AI returns 401 | Check the active agent, the `/even` endpoint, and `DOT_EVEN_TOKEN` on the phone. |

## Build from source

With Rust 1.92 or newer and a C toolchain on macOS or Linux, run:

```sh
git clone https://github.com/ihorkatkov/g2-dot-link.git
cd g2-dot-link
cargo build --release --locked
./target/release/g2-dot-link demo
```

Copy `target/release/g2-dot-link` to a compatible host with a system CA trust store. SQLite is bundled. The source checkout includes [release test instructions](docs/testing.md).

## Hosting reference

<details>
<summary>Service settings, logs, backups and upgrades</summary>

Run `g2-dot-link serve` under your service manager with the inbox directory as its working directory. The binary reads `service.env` directly. Exported variables take precedence. Keep the OS CA trust store current. Use a dedicated service account and persistent local disk. SIGTERM and Ctrl-C drain active requests. Run one instance per inbox.

| Setting | Purpose or default |
| --- | --- |
| `DOT_ORIGIN` | Public HTTPS origin used for OAuth discovery |
| `DOT_DB` | SQLite path, generated as an absolute path under `data/` |
| `DOT_OWNER_ID` | Stable identity returned to Dot |
| `DOT_PAIR`, `DOT_CLIENT_SECRET`, `DOT_EVEN_TOKEN` | Owner pairing code, OAuth client secret, and glasses bearer token |
| `DOT_BIND` | Listener address, default `127.0.0.1:8080` |
| `DOT_WAIT_SECONDS` | Time to wait for a direct reply, default `90`. Fractional seconds are supported. |
| `DOT_LOG_LEVEL` | `error`, `warn`, `info` (default), `debug`, or `trace` |

The service writes JSON logs to stderr. Look for `listening` at startup, `event_delivery` for callback status and attempt, and `even_reply` for direct-reply latency. A successful delivery means the receiver accepted the question. Dot must still call the reply tool. If the glasses say Dot is disconnected, check the plugin connection and ask Dot to subscribe again.

Application logs omit credentials, headers, URLs, and question and answer text at every level. Configure proxy logs to omit OAuth query strings and sensitive headers too. Keep `service.env`, the database, backups, and SQLite sidecars private. The database contains conversations and webhook signing keys.

**Upgrading an existing installation.** Stop the Python service, back up `service.env` and the complete database directory together, then start the Rust executable with the same environment. It opens the existing schema in place, including grants, queued deliveries, and late replies. Keep the original owner ID, credentials, origin, and database path. Run `configure` only for a new inbox.

**Moving or restoring the inbox**

Stop the service before copying the entire inbox directory, including SQLite sidecars, to private persistent storage. Restore it with the same owner, permissions, secrets and public origin. Update the absolute `DOT_DB` path in `service.env` if the directory moved. Restart with `g2-dot-link serve` and use `ask` to check delivery. Changing the origin also requires updating the ChatGPT plugin and Even endpoint and reconnecting Dot.

To change the glasses token, update `DOT_EVEN_TOKEN`, restart the service, and paste the new value into Even AI. Changing `DOT_PAIR` changes the code used for future pairings. If you change `DOT_CLIENT_SECRET`, update the ChatGPT OAuth settings too. Existing access grants remain valid until they expire.

</details>

Independent community project. [ChatGPT plugin setup](https://developers.openai.com/plugins/deploy/connect-chatgpt) · [Even’s custom agent guide](https://support.evenrealities.com/hc/en-us/articles/16280926743695-Tutorial-Bridging-G2-to-OpenClaw-Bring-Your-Own-Agent)
