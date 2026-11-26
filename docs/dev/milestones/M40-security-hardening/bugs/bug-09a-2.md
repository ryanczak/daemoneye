# Bug 2 on phase-09a: a non-object JSON body no longer becomes a generic alert

**Severity:** major (a regression in the generic payload fallback)
**Status:** closed (2026-11-26, fixed in `7ad1d417`; see the phase's review verdict)
**Filed:** 2026-11-26

## What's wrong

The phase changed `parse_generic` (`src/webhook/parse.rs`) so that `{}`
yields no alert. The spec's handler test had claimed that `{}` was already
"today's no-parseable-alerts path". That claim was an architect error: before
the phase, `{}` became a `GenericAlert` whose description was the serialized
`{}`.

The new guard is `body.as_object().is_some_and(|o| !o.is_empty())`. It drops
the full-body description fallback for **every** non-object body, not just
for `{}`. A string body (`"disk full"`), an array or a number now yields
`None`, so `parse_payload` returns nothing and the POST gets `400`. Before the
phase, each of those became a `GenericAlert` carrying the body as its
description.

## What should happen

Only an **empty JSON object** yields no alert. Any other body that reaches
the generic fallback keeps today's behaviour: a non-empty object, a string, an
array or a number becomes a `GenericAlert` whose description is the
pretty-printed body.

`src/webhook/parse.rs` is now authorized for this phase. Keep the change
there to the empty-object rule.

## Root cause

The guard tests "is a non-empty object" where the rule is "is not an empty
object". A non-object fails the first test but passes the second.

## Definition of done

- [ ] A new test, `generic_non_object_body_still_alerts`, in
      `src/webhook/parse.rs`'s tests, passes.
      - **Asserts:** `parse_payload(&json!("disk full"))` returns exactly
        one alert, and its description contains `disk full`.
        `parse_payload(&json!([1, 2]))` also returns exactly one alert.
      - **Today:** it fails, because both return no alerts.
- [ ] `generic_empty_object_yields_no_alert` still passes.
