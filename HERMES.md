# Hermes Tuning for Pocket Agent

> **Installing Pocket Agent with Hermes?** One setup step lives here: copy PA's skill into the Hermes profile — see [Pocket Agent skill](#pocket-agent-skill).

Almost all of a turn's latency is spent inside Hermes, not Pocket Agent: PA forwards Hermes' reply to TTS within ~0.1s. In our measurements, LLM round trips accounted for ~78% of turn time; the rest went to tools, mostly screenshot vision.

The settings below go in your Hermes profile's `config.yaml` (e.g. `~/.hermes/profiles/<profile>/config.yaml`). Back the file up first.

```yaml
compression:
  enabled: true
  threshold_tokens: 150000     # absolute trigger; see note 1
  protect_last_n: 8            # see note 1

auxiliary:
  vision:
    extra_body:
      thinking:
        type: disabled         # GLM only; see note 2
  compression:                 # on MiniMax: see note 2
    extra_body:
      thinking:
        type: disabled         # GLM only; remove on MiniMax

computer_use:
  capture_after_mode: ax       # see note 3
  no_overlay: false            # optional; see "Agent cursor"
```

1. **Cap the context.** PA uses one Hermes session per day (`<agent>-YYYY-MM-DD`), so without compression a busy day grows to 150K–370K tokens per request. At that size we saw stream stalls of 5–9 minutes, and a model switch that drops the prompt cache made a single call take ~70s. `compression.threshold` (default `0.5`) is a fraction of the model's context window, so on a 1M-token model it would only fire at 500K and never trigger. Use `threshold_tokens` instead; Hermes takes the lower of the two.

   Do not set it too close to what is left after a compaction. `protect_last_n` keeps that many recent messages verbatim, and PA's tool results are large: with the default 20, about 35K–45K tokens survived each compaction and ~50K remained in total, so an 80K trigger fired again after a few turns (three compactions in one afternoon on the iMac, each a 2–4 minute wait). We use 150K with `protect_last_n: 8`.
2. **Turn off thinking for auxiliary calls.** Screenshot description and compression summaries do not need reasoning, and output tokens (including reasoning) dominate their latency. On z.ai GLM models, vision went from 25s to 10s and compression from 24s to 13s. For GLM, `auxiliary.<task>.reasoning_effort: none` did **not** disable thinking in our tests; use `extra_body.thinking` as shown. Other providers use different fields.

   **These `thinking` settings are for GLM only.** MiniMax models (e.g. `MiniMax-M3.1-Flash`) require adaptive thinking and reject both `thinking.type: disabled` and `reasoning_effort: none` with a 400 (`requires adaptive thinking ... (2013)`). For compression this is silent but costly: no summary is generated (`Failed to generate context summary` in `agent.log`) and old messages are simply dropped. When the main model is MiniMax, delete `auxiliary.compression.reasoning_effort` and `auxiliary.compression.extra_body`. The `auxiliary.vision` entry has no effect on MiniMax, because screenshots go through the main model's native image path (`vision_analyze: native fast path`) instead of the auxiliary vision call; it only matters if you switch back to GLM.

   **On MiniMax, give compression a faster model instead.** Thinking cannot be turned off, but models differ a lot in how much they think. Summarizing the same ~18K characters of a PA session (2026-11-03, direct API calls): `MiniMax-M3.1-Flash` took 32–48s with 11K–13K characters of thinking; `MiniMax-M2.7-highspeed` took 7.5–8.4s; `MiniMax-M2.5-highspeed` 16s; `MiniMax-M3` 23s. Inside Hermes, whose summary prompt asks for more, M3.1-Flash took 122–221s per compaction. The main model stays the same; only the summary call changes, with the same API key:

   ```yaml
   auxiliary:
     compression:
       provider: minimax
       model: MiniMax-M2.7-highspeed
   ```

   Do not use M2.7-highspeed as the main model: even a one-line request takes ~7s, against ~1.5s for M3.1-Flash.
3. **Verify actions with the accessibility tree.** After a click or keypress, `capture_after` re-captures the window. The default `som` mode runs the vision model each time (~11s); `ax` returns window titles, URLs and on-page text in ~1s. Hermes can still request a `som`/`vision` capture explicitly when it needs pixels.

**Restart the gateway after changing `compression` or `no_overlay`.** The gateway caches one agent per session, and that cache is not rebuilt when compression settings change. The cua-driver daemon reads `no_overlay` only when it starts. `auxiliary.*` and `capture_after_mode` are read on every call. With launchd: `launchctl kickstart -k gui/$(id -u)/ai.hermes.gateway-<profile>`.

**Main model.** Each call on z.ai's coding endpoint has a ~5s floor, plus ~70 output tokens/s. Disabling the main model's thinking saved only ~1s per simple step, so we left it on.

## Agent cursor

Hermes' `computer_use` runs on [Cua Driver](https://github.com/trycua/cua), which can draw its own agent cursor to show where it is acting, without moving your real pointer. On macOS, Hermes turns it off unless the config says otherwise: when `no_overlay` is unset it passes `--no-overlay` (`tools/computer_use/cua_backend.py`), because the overlay's redraw loop has been known to peg a CPU core. Set `computer_use.no_overlay: false` to turn it on. If `cua-driver` then keeps a core busy in Activity Monitor, set it back to `true`.

What to expect with the driver Hermes pins (0.21.0):

- An action by element number (most clicks) snaps the cursor to the target with a brief pulse; only pixel clicks glide. The cursor hides after 20s idle. These are per-session motion settings (`set_agent_cursor_motion`: `glide_duration_ms`, `idle_hide_ms`, `dwell_after_click_ms`). Hermes does not call that tool and has no config key for it, so changing them means patching Hermes.
- The overlay window sits at the normal window level and can sink behind the app it just clicked ([trycua/cua#2993](https://github.com/trycua/cua/issues/2993)).
- Actions sent with `delivery_mode="foreground"` front the target window and do not use the agent cursor. Hermes' own computer-use skill prescribes foreground for browser address-bar input, so expect those steps to show up as real focus changes.

Each gateway restart can leave the previous `cua-driver serve --embedded` daemon running. They sit idle, but you can find them with `pgrep -fl "cua-driver serve"` and stop every one except the daemon your current `cua-driver mcp` process connects to (same `--socket` path).

## Pocket Agent skill

`skills/pocket-agent/SKILL.md` in this repo holds notes for Hermes when it talks to the user through PA. Today it covers one thing: when an answer is too long or complex to say well, write a single-file HTML page to `~/.pocket-agent/html/` (`YYYYMMDD-HHMMSS-<slug>.html`), open it in the default browser with `open <path>`, and say only the gist aloud. Spoken-style rules stay in PA's per-turn voice hint, not in the skill. PA's `COMPUTER_HINT` (local gateway only) points Hermes to the skill, so Hermes reads it only when it is about to make a page, which avoids an extra `skill_view` round trip on every turn.

**If you are an agent installing Pocket Agent with Hermes, install this skill as part of the setup.** Copy the folder into the skills directory of the Hermes profile PA connects to (do not symlink it), then check that Hermes lists it:

```bash
mkdir -p ~/.hermes/profiles/<profile>/skills/my-skills
cp -R <pocket-agent repo>/skills/pocket-agent ~/.hermes/profiles/<profile>/skills/my-skills/
hermes -p <profile> skills list | grep pocket-agent
```

The installed copy does not follow the repo. After the skill changes in the repo (e.g. after pulling a new PA version), copy it again.

- The system prompt's skill index cuts descriptions to 60 characters, so the skill's `description` is kept under that.
- Hermes builds the skill index once per session. Restart the gateway or wait for the next day's session (`<agent>-YYYY-MM-DD`) before Hermes sees a new or renamed skill. Changes to the body of the installed copy take effect on the next `skill_view`.

## Long tasks and background subagents

While Hermes works, the user only hears silence. On every UI turn PA appends a `LONG RESEARCH` hint (`DELEGATE_HINT` in `src-tauri/src/commands/chat.rs`; bridge turns do not get it): research that will likely take more than about a minute goes to a background subagent with `delegate_task`, Hermes says one sentence that it has started, and ends the turn. Desktop tasks the user watches (`computer_use`) stay in the foreground. This is the one place PA suggests how Hermes should work instead of only relaying facts (agreed with the user on 2026-11-03).

The gateway does not start a turn when an async delegation finishes; it only stores an `async_delegation_complete` row in the session. So while a delegation is out, PA reads the session's latest messages every 15 seconds (`src-tauri/src/api/subagents.rs`). When the completion row appears, PA starts a turn itself as soon as it is idle, with a fixed text asking Hermes to tell the user the result; the result row is already in the session, so Hermes sees it in that turn. If the user speaks first, that turn carries the result instead and no extra turn is sent.

- Top-level `delegate_task` always runs in the background (`tools/delegate_tool.py`); the profile's `delegation` section sets `child_timeout_seconds` and `max_concurrent_children`. Without that section Hermes uses its defaults.
- PA keeps only delegation ids, never the subagent's text; the result reaches the user through Hermes' reply.
- The page for the details follows the [Pocket Agent skill](#pocket-agent-skill) ("Background research").

## Reasoning and status on `/v1/runs` (local patch)

PA's reasoning panel needs the model's reasoning in the stream, and its status panel needs Hermes' lifecycle status (e.g. context compaction). `/v1/chat/completions` sends it (`delta.reasoning_content`), but `/v1/runs`, which PA uses for local desktop turns, registers no `reasoning_callback`, so the run stream carries none. Do not switch PA to chat completions to get it back: that endpoint has no stop command, and after PA disconnects Hermes logs `SSE client disconnected; interrupted agent task` but keeps running the turn (seen 2026-11-02: one turn kept driving Chrome for over 20 minutes).

`/v1/runs` also registers no `status_callback`, so the compaction notices chat completions sends as `hermes.status` never reach PA, and a turn that starts with a minutes-long compaction just looks stuck.

Until Hermes ships this upstream, patch `gateway/platforms/api_server_runs.py` in the Hermes checkout (`~/.hermes/hermes-agent`). Add both callbacks next to `_text_cb` in `_execute_run`, and pass them to both turn paths:

```diff
     def _text_cb(delta: Optional[str]) -> None:
         ...
+    def _reasoning_cb(text: Optional[str]) -> None:
+        # Model reasoning deltas as ``reasoning.delta``, the run twin of chat completions'
+        # ``delta.reasoning_content``; kept apart from answer text.
+        if not text or run_id not in self._run_streams:
+            return
+        with suppress(Exception):
+            loop.call_soon_threadsafe(run.put_event, _run_event(run_id, "reasoning.delta", delta=text))
+
+    def _status_cb(kind, message=None) -> None:
+        # Lifecycle status (compression, provider wait, fallback) as ``hermes.status``,
+        # the same event chat completions sends.
+        if run_id not in self._run_streams:
+            return
+        text = _redact_api_error_text(message if message is not None else kind or "").strip()
+        if not text:
+            return
+        with suppress(Exception):
+            loop.call_soon_threadsafe(run.put_event, _run_event(run_id, "hermes.status", kind=str(kind), text=text))

                 result, usage = await observe_api_turn(
-                    run.admission, stream_delta_callback=_text_cb,
-                    tool_start_callback=_tool_start, tool_complete_callback=_tool_complete)
+                    run.admission, stream_delta_callback=_text_cb, reasoning_callback=_reasoning_cb,
+                    status_callback=_status_cb, tool_start_callback=_tool_start, tool_complete_callback=_tool_complete)

                 agent = self._create_agent(
                     stream_delta_callback=_text_cb, tool_progress_callback=self._make_run_event_callback(run_id, loop),
-                    interim_assistant_callback=_interim_cb, **run.agent_kwargs)
+                    interim_assistant_callback=_interim_cb, reasoning_callback=_reasoning_cb,
+                    status_callback=_status_cb, **run.agent_kwargs)
```

Both paths already accept the callbacks: `observe_api_turn` lists `reasoning_callback` and `status_callback` in `_OBSERVER_KEYS` (`gateway/session_api_turn.py`), and `_create_agent` takes them as keywords. `_redact_api_error_text` is already bound at the top of `_execute_run`. PA reads each event as `{"event": "reasoning.delta", "delta": "<text>"}` and appends the chunks as they are (`src-tauri/src/api/runs.rs`); without the patch PA simply shows no reasoning. The panel also needs **Show reasoning** on in Settings → General.

Status arrives as `{"event": "hermes.status", "kind": "...", "text": "..."}`. PA only acts on compaction (`src-tauri/src/api/runs.rs` `compaction_status`): when it starts, the status panel shows that PA is tidying up the earlier conversation and says so once; `kind: "compacted"` removes the line. Other statuses are ignored, and without the patch nothing changes.

- Restart the gateway after patching (`launchctl kickstart -k gui/$(id -u)/ai.hermes.gateway-<profile>`), and patch each machine's checkout separately.
- A Hermes update stashes local changes (`updates.non_interactive_local_changes: stash`), which drops the patch. After an update, check with `git -C ~/.hermes/hermes-agent diff --stat` and apply it again.
- Check: start a run with a question that needs some thought (a trivial one may produce no reasoning) and count `reasoning.delta` events on `GET /v1/runs/<id>/events`. Tested 2026-11-02 against Hermes `bf0203a`: 44 events for one arithmetic question. The Hermes run tests (`tests/gateway/test_api_server_runs.py`, `test_api_runs_poll_shape.py`) give the same 44 passed / 21 failed with and without the patch.

## Measuring

`scripts/hermes-latency.py` (standard library only) reports where turns spend their time:

```bash
# Per-turn totals, LLM calls, tool time, input tokens, cache hits and stalls from the Hermes log
python3 scripts/hermes-latency.py analyze --since 2026-11-01 [--log ~/.hermes/profiles/<profile>/logs/agent.log]

# Live probes against the gateway (screen / code / flight), sent with PA's real COMPUTER_HINT.
# Each probe runs in its own pa-latency-probe-* session; your PA session is untouched. Approval requests are denied.
python3 scripts/hermes-latency.py probe --all
```

Run a probe before and after each change. If one category stays slow, `analyze` usually shows the cause: many LLM calls per turn, large input tokens, or a slow tool.
