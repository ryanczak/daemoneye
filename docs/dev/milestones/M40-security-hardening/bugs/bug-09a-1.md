# Bug 1 on phase-09a: a disabled webhook on a non-loopback bind can never reload

**Severity:** major (a regression: `daemoneye reload` stops working for a valid config)
**Status:** open
**Filed:** 2026-11-26

## What's wrong

The new fail-closed check in `ConfigHandle::reload` (`src/config/handle.rs`,
the `validate_webhook_bind(&bind_ip, &fresh_secret)` call at the top of the
`Ok(mut fresh)` arm) runs on **every** reload. It ignores whether the webhook
is enabled.

At startup the bind check runs only for an enabled webhook:
`src/daemon/startup_tasks.rs` returns early on `!startup_config.webhook.enabled`
before `crate::webhook::bind`. A config with all three of these therefore
starts cleanly:

- `[webhook] enabled = false`;
- `bind_addr = "0.0.0.0"`;
- no `secret`.

Every reload of that config is then refused with "reload refused
(webhook.secret)". This happens even when the edit only touches `[limits]`
and the secret was never set. Nothing listens, so nothing is exposed, yet the
user loses `daemoneye reload` entirely.

## What should happen

The refusal guards a listener that is actually exposed. It applies only when
the **running** config has `webhook.enabled = true`. With the webhook
disabled, a reload proceeds as before, whatever `bind_addr` and `secret` hold.

## Root cause

The phase spec said "if the running `bind_addr` is not loopback and the new
secret is empty, refuse". That is an architect omission: the spec did not
name `enabled`, and the implementation followed it literally.

## Definition of done

- [ ] A new test, `reload_with_a_disabled_webhook_skips_the_secret_check`, in
      `src/config/handle.rs`'s tests, passes.
      - **Setup:** the running file has `[webhook] enabled = false`,
        `bind_addr = "0.0.0.0"` and no secret, plus
        `[limits] total_tool_calls_per_turn = 7`. The rewrite changes only
        the limit, to 8.
      - **Asserts:** `report.error` is `None`; `applied` names
        `limits.total_tool_calls_per_turn`; the running value is 8.
      - **Today:** it fails, because the reload is refused.
- [ ] `reload_refuses_to_empty_the_secret_on_a_public_bind` still passes, with
      `enabled = true` added to **both** of its `[webhook]` fixtures. Without
      it, the fixed check would no longer apply to that test.
