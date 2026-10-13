---
name: cctrack
description: Use at the start of every session, whenever a message mentions cctrack (login required, blocked prompt, denied tool), and after finishing a meaningful piece of work to upload a short usage summary with the cctrack CLI.
---

# cctrack — usage tracking

This team tracks Claude Code plan usage with the `cctrack` CLI. Hooks upload token
counts automatically; your jobs are to respect the login gate and to attach a
short summary of the work you did. The plugin requires Claude Code 2.1.295+.

## 1. The login gate (mandatory)

If the session context, a blocked prompt, or a denied tool says cctrack is
**not logged in**, or that the login was **revoked/expired**:

- Do **not** start or continue the user's task. Do not try to work around the block.
- Ask the user to sign in **themselves**, then ask you again:
  - Login revoked/expired (this machine is already linked to a server): run
    `cctrack login` in a terminal, or type `! cctrack login` here.
  - Not logged in (never signed in on this machine, or after `cctrack logout`):
    run `cctrack login --server <your-cctrack-url>` in a terminal, or type
    `! cctrack login --server <your-cctrack-url>` here.
  - Headless/SSH machine: run `cctrack login --paste` (plus `--server <url>` the
    first time) in a real terminal. It reads the code from stdin, so it cannot
    run through `!` or through you.
- Do **not** run `cctrack login` yourself: the browser login can wait up to
  5 minutes for the user, and `--paste` needs a terminal. Never pass `--server`;
  only the user chooses the server.
- You may run `cctrack status` or `cctrack whoami` to check whether the user has
  signed in.
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
3. Remove every `'` from the summary, then run it wrapped in **single quotes**
   (never double quotes, so backticks or `$` in the summary are never executed),
   using the session id from the cctrack SessionStart context:
   `cctrack upload --session <session-id> --summary '<the haiku summary>'`
   If you don't know the session id, omit `--session`; it defaults to the most
   recently started session.
4. If the command fails, mention it briefly to the user and carry on — never
   retry in a loop. Do not upload more than once per unit of work.

## 3. Status

- `cctrack status` — local, instant: login state and detected Claude account.
- `cctrack whoami` — checks the token with the server.
- `cctrack sync` — manual upload of any pending usage (hooks normally do this).
