---
name: pocket-agent
description: "Pocket Agent (voice): show long/complex answers on a page"
---

# Talking to the user through Pocket Agent

## Background

Pocket Agent (PA) is a voice front end running on the same Mac as you; its sessions have ids starting with `pocket-agent-`. Your reply is read aloud by text-to-speech; the user does not see the text. How to phrase spoken replies is set by the hint PA adds to every turn — follow that hint; this skill does not repeat it.

New notes about working with PA go into this file as new sections.

## Showing a page

### When

Make a page when the content needs to be seen to be understood — tables, comparisons, code, several steps, numbers — or when saying it would take more than a few sentences. Simple questions: just answer aloud as usual.

### How

- Directory: `~/.pocket-agent/html/` (create it with `mkdir -p ~/.pocket-agent/html` if missing).
- File name: `YYYYMMDD-HHMMSS-<slug>.html`, e.g. `20261101-223000-tts-compare.html`.
- One self-contained file: CSS and JS inline, no remote scripts.
- Write it in the language the user speaks, with clear font sizes, readable in both light and dark mode.
- Do not put secrets, API keys or tokens on the page.
- Write only inside `~/.pocket-agent/html/`. The rest of `~/.pocket-agent/` holds PA's `.env`, `server.token` and chat history; leave those files alone.

### Showing it

computer_use cannot launch apps or open files (it only switches to windows that are already open). `open <path>` in the terminal opens the page in the user's default browser. Once it is open, computer_use can scroll it or point at a part when the user asks about it.

### What to say

Say the conclusion in one or two sentences, then mention that the details are in the browser (e.g. "详细的我放在浏览器里了"). Do not read the file path aloud.

## Background research

The user hears only silence while you work. When research or compiling material will likely take more than about a minute (several searches, reading several pages, multi-step analysis), hand it to a background subagent with `delegate_task`:

- In the same response, say in one short sentence that you have started and the user can keep talking about other things (e.g. "我在后台查一下，你可以先聊别的。"), then end the turn.
- When the result is in, PA starts a turn for you. Say the conclusion in one or two sentences and put the details on a page (see "Showing a page").
- Desktop tasks the user watches (computer_use) stay in the foreground.
