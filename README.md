# G2 Dot Link

Ask your personal ChatGPT assistant (Dot) a question through Even AI and read the answer on your Even Realities G2 lenses. Keep your phone in your pocket. If Dot answers late, your next question picks up all ready replies in order.

## Try it in a minute

On an Apple Silicon Mac, paste this into a terminal. The offline demo uses a simulated Dot and needs no glasses, account, or server.

```sh
curl -fL https://github.com/ihorkatkov/G2-OpenAI-Dots/releases/download/v0.1.0/g2-dot-link-macos-arm64 -o g2-dot-link && chmod +x g2-dot-link && ./g2-dot-link demo
```

The demo asks two questions. On the second, you see the answer to the first: **“Three parts oil to one part vinegar.”**

On **Linux ARM64**, use the [Linux executable](https://github.com/ihorkatkov/G2-OpenAI-Dots/releases/download/v0.1.0/g2-dot-link-linux-arm64) in the same command. To build on another Mac or Linux machine, clone this repo and run `cargo run --locked --release -- demo` with Rust 1.92+ and a C compiler.

## Connect your glasses

Even AI sends your question to a private inbox, which forwards it to Dot and returns the answer to your lenses. Each inbox serves one wearer. Its operator gives you the Even AI endpoint, glasses token, and pairing code privately. To run your own, follow the [hosting guide](docs/hosting.md).

1. **Import the marketplace.** In ChatGPT, open **Admin → Plugins → Add → Import marketplace** and paste `https://github.com/ihorkatkov/G2-OpenAI-Dots`. Leave **Path** and **Branch** blank and select **Import marketplace**. The plugin uses the existing **Glasses inbox** app. If you host a different inbox, follow the [custom inbox instructions](docs/hosting.md#use-your-own-inbox-with-the-marketplace) and import your fork instead.
2. **Install G2 Dot Link.** Make the plugin available to your account, then open **Plugins → G2 Dot Link → Install**.
3. **Connect Dot.** Connect the inbox in ChatGPT, enter the pairing code, and use the operator’s OAuth client secret if prompted. Enable the plugin in your personal Dot conversation and ask **“Connect my glasses.”** Wait for subscription confirmation.
4. **Connect Even AI.** In the Even phone app, open **Settings → Even AI → Agent Configuration**. Name the agent **Dot**, paste the endpoint and glasses token, save, and activate. Ask **“What’s a simple vinaigrette ratio?”** A prompt reply appears on the lenses. If you see **“Question saved”**, ask your next question normally.

If **Import marketplace** is missing from your Add menu, use **Plugins → Add → Upload plugin archive** with the ZIP prepared for your inbox, then continue at **Connect Dot**.

[Hosting and troubleshooting](docs/hosting.md) · [Release tests](docs/testing.md)
