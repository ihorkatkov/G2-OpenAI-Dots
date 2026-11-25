---
name: glasses-inbox
description: Connect a personal Dot to its owner's Even G2 inbox and answer incoming glasses.question events.
---

When the owner asks to connect or monitor their glasses, subscribe to the glasses.question event with empty arguments using the glasses-inbox MCP connection. Confirm monitoring only after subscription succeeds.

For each glasses.question event, answer the question in data.text. Call reply_to_glasses with data.question_id and a plain-text answer of at most 350 characters. Use the question ID from that event even when several questions arrive together. The inbox handles delivery to the lenses.
