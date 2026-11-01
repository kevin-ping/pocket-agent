# Hermes Tuning for Pocket Agent

Almost all of a turn's latency is spent inside Hermes, not Pocket Agent: PA forwards Hermes' reply to TTS within ~0.1s. In our measurements, LLM round trips accounted for ~78% of turn time; the rest went to tools, mostly screenshot vision.

The settings below go in your Hermes profile's `config.yaml` (e.g. `~/.hermes/profiles/<profile>/config.yaml`). Back the file up first.

```yaml
compression:
  enabled: true
  threshold_tokens: 80000      # absolute trigger; see note 1

auxiliary:
  vision:
    extra_body:
      thinking:
        type: disabled         # see note 2
  compression:
    extra_body:
      thinking:
        type: disabled

computer_use:
  capture_after_mode: ax       # see note 3
  no_overlay: false            # optional; see "Agent cursor"
```

1. **Cap the context.** PA uses one Hermes session per day (`<agent>-YYYY-MM-DD`), so without compression a busy day grows to 150K–370K tokens per request. At that size we saw stream stalls of 5–9 minutes, and a model switch that drops the prompt cache made a single call take ~70s. `compression.threshold` (default `0.5`) is a fraction of the model's context window, so on a 1M-token model it would only fire at 500K and never trigger. Use `threshold_tokens` instead; Hermes takes the lower of the two.
2. **Turn off thinking for auxiliary calls.** Screenshot description and compression summaries do not need reasoning, and output tokens (including reasoning) dominate their latency. On z.ai GLM models, vision went from 25s to 10s and compression from 24s to 13s. For GLM, `auxiliary.<task>.reasoning_effort: none` did **not** disable thinking in our tests; use `extra_body.thinking` as shown. Other providers use different fields.
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
