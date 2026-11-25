# G2 Dot Link

Talk to your personal ChatGPT Dot from Even Realities G2 glasses. Ask a question through the built-in Even AI assistant and read Dot’s reply on the lenses. You host a small Python service for one wearer. Once paired, your Mac can be off.

## How a question travels

1. Even AI transcribes your speech and sends the text to your service.
2. The service saves the question in SQLite and sends a signed MCP event to your subscribed Dot.
3. Dot calls the plugin’s reply tool with the question ID and a short answer. If it arrives within 90 seconds, the service returns it on the original Even AI request.
4. If Even AI displays “Question saved,” ask **“What did Dot say?”** to collect a late reply. If several questions timed out, repeat the phrase to collect ready replies in question order.

Each installation has its own inbox. The native Dot chat history stays in ChatGPT. Even AI displays replies to its own requests. A late reply stays in the inbox until requested.

## Run the service

You need Python 3.12 or newer, [uv](https://docs.astral.sh/uv/), persistent disk, and a public HTTPS origin you control. Your ChatGPT account needs a personal Dot and permission to install a custom MCP plugin. Your Even app needs the **Agent Configuration** menu. [OpenAI MCP Events](https://developers.openai.com/plugins/build/mcp-events) · [Even’s custom agent guide](https://support.evenrealities.com/hc/en-us/articles/16280926743695-Tutorial-Bridging-G2-to-OpenClaw-Bring-Your-Own-Agent)

On your host, replace the example origin with your own domain.

```sh
git clone https://github.com/ihorkatkov/g2-dot-link.git
cd g2-dot-link
uv sync --frozen
uv run python -m unittest -v
uv run python configure.py https://glasses.example.com
set -a
. ./service.env
set +a
umask 077
uv run gunicorn --workers 1 --threads 12 --timeout 120 --bind 127.0.0.1:8080 'service:start()'
```

Put an HTTPS reverse proxy in front of port 8080 and allow at least 100 seconds for an upstream response. With Caddy on the same host, a site block containing `reverse_proxy 127.0.0.1:8080` works with the command above. Check that `https://glasses.example.com/health` returns JSON with `"status":"ok"`. Run Gunicorn under your service manager for unattended use. Keep one worker because it owns the event dispatcher. The host needs outbound HTTPS access to ChatGPT’s callback.

The setup command generates private credentials in `service.env`. Keep that file and `data/` on persistent storage accessible only to the service owner. The SQLite database contains question and reply text and subscription signing secrets. Back up both privately.

## Connect your Dot

1. Open **ChatGPT → Plugins → Add → Add custom MCP server**. Name it **Glasses inbox** and use `https://glasses.example.com/mcp` as the server URL.
2. Choose **OAuth → Advanced OAuth settings → User-Defined OAuth Client**. Set the client ID to `g2-dot-link`, the client secret to the value of `DOT_CLIENT_SECRET` in your local `service.env`, and the token endpoint authentication method to `client_secret_basic`. The service advertises the `glasses` scope.
3. Create and connect the plugin. Enter `DOT_PAIR` on your own service’s pairing page.
4. Open your personal Dot conversation and send the instruction below. Wait for Dot to confirm monitoring is active. [ChatGPT plugin setup](https://developers.openai.com/plugins/deploy/connect-chatgpt)

> Subscribe to the Glasses inbox glasses.question event. For each event, answer its text in at most 350 plain-text characters and call reply_to_glasses with the event’s question_id and your answer.

## Connect your glasses

In the Even phone app, open **Settings → Even AI → Agent Configuration**. Enter **Dot** as the agent name, the full `https://glasses.example.com/even` URL as the endpoint, and the value of `DOT_EVEN_TOKEN` as the token. Save and activate it. Ask a question with a recognizable answer, such as “What is seventeen plus twenty-six?”

If Even AI says Dot is disconnected, check the plugin connection and ask Dot to subscribe again. A successful webhook receipt means ChatGPT accepted the event. The reply tool call is what delivers the answer back to the service.

## Verification

A deployed instance exchanged messages with a real personal Dot. Two measured replies arrived on the original HTTP request after 16.45 and 40.75 seconds. A wearer also saw the Dot conversation in the Even AI phone app. Those timings are individual measurements. Phone connectivity, Dot scheduling and the client timeout affect each turn. Automated tests exercise OAuth, signed-callback verification failure, retries, persistence, and out-of-order replies using a simulated receiver.

Independent community project. The [community G2 adapter](https://github.com/kucau0901/even-g2-agent-adapter) documents the Even AI request format used here.
