# Host an inbox and prepare its plugin

You’ll need an always-on macOS or Linux host with persistent disk, a stable public HTTPS URL (your host can provide one), a personal Dot with custom MCP plugin access, and **Agent Configuration** in the Even phone app. This inbox serves one wearer.

**1. Start the inbox.** In the directory where you want to keep it, use the HTTPS origin assigned to your service.

```sh
./g2-dot-link serve https://glasses.example.com
```

Leave it running. The command creates private credentials in `service.env`, stores the inbox in `data/`, and prints the two URLs you’ll use below. You can rerun the same command after a restart. For always-on use, run it under your service manager. [Service settings](#hosting-reference)

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

For personal ChatGPT accounts, give the wearer `g2-dot-link.zip`. For a managed workspace, use the [marketplace setup](../README.md#connect-your-glasses). For either route, share the Even AI endpoint printed at startup and the three values from the private `service.env` file directly with the wearer: `DOT_PAIR` (pairing code), `DOT_CLIENT_SECRET` (OAuth client secret), and `DOT_EVEN_TOKEN` (glasses token). The archive contains only the plugin manifest, public MCP connection settings and instructions for Dot. Follow [Connect your glasses](../README.md#connect-your-glasses) to pair.

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

## Use your own inbox with the marketplace

Register your inbox’s MCP connection in your ChatGPT workspace and give the wearer access to that app. Fork this repository and replace the app ID in [plugin/.app.json](../plugin/.app.json) with your app’s ID. Use your fork’s URL in the [marketplace import step](../README.md#connect-your-glasses). [ChatGPT marketplace guide](https://learn.chatgpt.com/docs/enterprise/plugin-management)
