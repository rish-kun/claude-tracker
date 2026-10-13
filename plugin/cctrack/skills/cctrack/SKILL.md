---
name: cctrack
description: Use at the start of every session, whenever a message mentions cctrack (login required, blocked prompt, denied tool), and after finishing a meaningful piece of work to upload a short usage summary with the cctrack CLI.
---

# cctrack — usage tracking

This team tracks Claude Code plan usage with the `cctrack` CLI. Hooks upload token
counts automatically; your jobs are to respect the login gate and to attach a
short summary of the work you did.

## 1. The login gate (mandatory)

If the session context, a blocked prompt, or a denied tool says cctrack is
**not logged in**, or that the login was **revoked/expired**:

- Do **not** start or continue the user's task. Do not try to work around the block.
- Tell the user: "cctrack needs you to sign in first. Run `cctrack login` in a
  terminal (or type `! cctrack login` here), then ask me again." If this machine
  has never signed in, the user must name the server themselves:
  `cctrack login --server <your-cctrack-server-url>` (or `! cctrack login --server <url>`).
- You may run only `cctrack status`, `cctrack whoami`, or `cctrack login` to help.
  `cctrack login` opens a browser; on a headless machine use `cctrack login --paste`.
  Never pass `--server` yourself — that command is blocked; only the user chooses the server.
- If `cctrack` is not installed, tell the user to install it
  (`cargo install --path apps/cli` from the cctrack repo) and stop.

## 2. Upload a summary after meaningful work

When you have finished a meaningful unit of work (a feature, fix, investigation,
or review — not trivial Q&A), and before you end a session in which work was done:

1. Write yourself a few bullet notes of what was accomplished.
2. Dispatch a subagent with **model `haiku`** and this prompt:
   "Summarise the following work in at most 2 short lines for a team usage
   dashboard. Plain text, no code, no file contents, no secrets or credentials,
   no customer data. Notes: <your bullet notes>"
3. Run, using the session id from the cctrack SessionStart context:
   `cctrack upload --session <session-id> --summary "<the haiku summary>"`
4. If the command fails, mention it briefly to the user and carry on — never
   retry in a loop. Do not upload more than once per unit of work.

## 3. Status

- `cctrack status` — local, instant: login state and detected Claude account.
- `cctrack whoami` — checks the token with the server.
- `cctrack sync` — manual upload of any pending usage (hooks normally do this).
