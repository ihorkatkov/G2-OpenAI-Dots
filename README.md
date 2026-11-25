# G2 Dot Link

Keep your phone in your pocket. Ask your personal assistant in ChatGPT (your Dot) a question through Even AI and read the answer on your Even Realities G2 lenses. Your always-on host keeps the inbox available when your laptop is off.

## Connect an existing inbox

If you have the prepared **g2-dot-link.zip** for your inbox, its server address is already inside. You can pair your glasses without running the executable or setting up DNS.

1. In ChatGPT on the web, select **Plugins → Add → Upload plugin** and choose the ZIP. In a managed workspace, check **Admin console → Plugins**. Availability depends on upload permissions. [ChatGPT plugin uploads](https://help.openai.com/en/articles/20001256-plugins-in-chatgpt)
2. Connect the plugin. If prompted for an OAuth client secret, use the inbox’s `DOT_CLIENT_SECRET`. On the pairing page, enter `DOT_PAIR`. Enable the plugin in your personal Dot conversation and ask **“Connect my glasses.”** The ZIP includes the subscription and reply instructions.
3. In the Even phone app, open **Settings → Even AI → Agent Configuration**. Name the agent **Dot**, paste the **Even AI endpoint** from your inbox operator, and enter `DOT_EVEN_TOKEN`. Save, activate, and ask a question.

Each package points to one wearer’s inbox. The operator provides credentials privately. ChatGPT and Even AI both reach the running service over HTTPS. A hosting provider’s address works without buying a domain.

## Try it with one command

Put the release executable for your platform in an empty directory, name it `g2-dot-link`, and open a terminal there. If you need to build a copy, see [Build from source](#build-from-source).

```sh
./g2-dot-link demo
```

The local demo uses a simulated Dot to show what happens when an answer arrives late. It runs offline, needs no account, and leaves the directory untouched.

```text
You: What’s a simple vinaigrette ratio?
Glasses: Question saved.
Dot’s reply is now in your inbox.

You: Which mustard goes well with that?
Glasses: Earlier replies:
Three parts oil to one part vinegar.

Question saved.
```

On your glasses, an answer that arrives within 90 seconds appears on the original request. If a reply arrives late, ask your next question normally. That request collects **all ready replies** in question order. Your new question also goes to Dot.

## Host an inbox and prepare its plugin

You’ll need an always-on macOS or Linux host with persistent disk, a stable public HTTPS URL (your host can provide one), a personal Dot with custom MCP plugin access, and **Agent Configuration** in the Even phone app. This inbox serves one wearer.

**1. Start the inbox.** In the directory where you want to keep it, use the HTTPS origin assigned to your service.

```sh
./g2-dot-link serve https://glasses.example.com
```

Leave it running. The command creates private credentials in `service.env`, stores the inbox in `data/`, and prints the two URLs you’ll use below. You can rerun the same command after a restart. For always-on use, run it under your service manager. [Hosting guide](#hosting-reference)

If your platform provides HTTPS, route it to the listener. If you manage TLS yourself, point your HTTPS reverse proxy at `127.0.0.1:8080`. With Caddy, use the following site block. Allow inbound HTTPS on the host and outbound HTTPS to Dot. Set any upstream response timeout to at least 100 seconds.

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

**2. Prepare the upload.** From the inbox directory, run:

```sh
./g2-dot-link plugin
```

Give the wearer `g2-dot-link.zip` and the Even AI endpoint printed at startup. The archive contains only the plugin manifest, public MCP connection settings and instructions for Dot. Follow [Connect an existing inbox](#connect-an-existing-inbox) to pair.

## If you get stuck

| What you see | What to check |
| --- | --- |
| Permission denied when starting the binary | Run `chmod +x ./g2-dot-link`, then retry. |
| Public health check fails or returns 502 | Check public DNS, TLS, the proxy upstream, and whether `serve` is running. |
| Pairing page shows “Invalid request” | In ChatGPT, check the plugin URL printed at startup and the client ID `g2-dot-link`, then start Connect again. |
| Pairing code rejected | Use the value after `DOT_PAIR=` from the running inbox’s file. |
| Dot is disconnected | Ask your personal Dot to subscribe again. Check that the plugin is connected. |
| “Question saved” on every request | Ask another question normally to receive any ready replies. Check the proxy timeout and whether Dot called `reply_to_glasses`. |
| The terminal gets answers but Even AI returns 401 | Check the active agent, the `/even` endpoint, and `DOT_EVEN_TOKEN` on the phone. |

## Build from source

With Rust 1.92 or newer and a C toolchain on macOS or Linux, run:

```sh
git clone https://github.com/ihorkatkov/G2-OpenAI-Dots.git
cd G2-OpenAI-Dots
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
