# Review Pack — T-E08 / T-E09: Surgical Merge + Unregister

## Objective
- T-E08: Fix `apply_hooks()` to use surgical per-type merge instead of full `hooks` key replacement
- T-E09: Add `remove_hooks()` function + `agtmux setup-hooks --unregister` CLI flag

## Summary

### T-E08 — Surgical merge

**Bug**: `apply_hooks()` called `obj.insert("hooks".to_string(), hooks)` — silently destroying other
tools' hook entries whenever `agtmux setup-hooks` was run.

**Fix**: Extracted pure `merge_hooks_into_settings(settings, quoted_script)` helper. For each of the
11 HOOK_TYPES, it `get_or_insert`s the array, `retain`s non-agtmux entries
(by absence of `AGTMUX_HOOK_TYPE=` sentinel), then appends the new entry.
The top-level `hooks` object is never replaced.
`generate_hooks_config()` (full-replacement helper) removed as it was dead code after the refactor.

### T-E09 — remove_hooks + --unregister

Added `remove_hooks_from_settings(settings)` (pure value-level) and `remove_hooks(scope)` (file I/O wrapper):
- Strips entries with `AGTMUX_HOOK_TYPE=` from each hook_type array
- Removes empty arrays, removes empty `hooks` object
- Idempotent: file-absent returns `Ok(path)` without error; no-op if no agtmux entries

CLI (`SetupHooksOpts`): added `--unregister: bool`. Dispatch order: `unregister → check → apply`.

## Change scope

| File | Change |
|------|--------|
| `crates/agtmux-runtime/src/setup_hooks.rs` | Surgical merge + remove_hooks + 13 new tests |
| `crates/agtmux-runtime/src/cli.rs` | `--unregister` flag on `SetupHooksOpts` |
| `crates/agtmux-runtime/src/main.rs` | `--unregister` dispatch branch |
| `docs/60_tasks.md` | T-E08, T-E09, T-term01 task entries added |

## Verification evidence

- `cargo test -p agtmux` → **147 tests PASS** (13 new)
- `just verify` (fmt + clippy + all workspace tests) → **PASS, 0 warnings**

### New test coverage (T-E08)
- `merge_hooks_preserves_other_tools_entries` — foreign entry survives apply
- `merge_hooks_is_idempotent` — double-apply produces no duplicates
- `merge_hooks_creates_hooks_key_when_absent` — fresh settings.json
- `merge_hooks_all_hook_types_present` — all 11 HOOK_TYPES registered

### New test coverage (T-E09)
- `remove_hooks_removes_only_agtmux_entries` — foreign entry survives remove
- `remove_hooks_cleans_empty_arrays` — empty array → key removed
- `remove_hooks_removes_hooks_key_when_empty` — empty hooks object → key removed
- `remove_hooks_noop_when_no_agtmux_entries` — idempotent when already clean
- `remove_hooks_preserves_other_settings_keys` — `"model"` key untouched
- `merge_then_remove_leaves_settings_unchanged` — round-trip invariant

## Risk declaration

- **Breaking change**: No. Surgical merge is strictly more correct than full replacement; consumers
  that relied on other-tool hooks being destroyed were already broken.
  `remove_hooks` is new functionality; no existing callers.
- **generate_hooks_config() removed**: Was only used internally by `apply_hooks()`. No public API
  surface — the function was `pub` but was only tested via `apply_hooks`. Tests migrated to
  `merge_hooks_into_settings`.
- **Fallbacks**: None added (per policy).
- **Known gaps**: `--unregister` and `--check` flags are not mutually exclusive at the clap level
  (both can be passed simultaneously); dispatch order (`unregister` checked first) handles this
  gracefully but the combination is undocumented behavior.

## Reviewer request

Provide verdict: GO / GO_WITH_CONDITIONS / NO_GO / NEED_INFO
