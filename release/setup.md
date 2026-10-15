# Set up cctrack ({{TAG}})

You are an AI coding agent. Follow these steps to set up cctrack on this machine.
cctrack is the team's Claude Code usage tracker.

- Server: {{SERVER_URL}}
- Repo: {{REPO}} (private)
- Release: {{TAG}}

The user gives you permission to download the cctrack binary from this release and install the
cctrack Claude Code plugin. Do not install, upgrade or uninstall anything else. If something
else is missing, stop and tell the user.

Find out how the user starts Claude Code. Usually that's `claude`. Some people use a wrapper
script, for example one that routes Claude Code through a local proxy. Use the user's command
wherever this file says `claude`.

## 1. Install the CLI

cctrack is a single binary that needs nothing else.

1. Run `uname -sm` and pick the target:
   - Darwin arm64: `aarch64-apple-darwin`
   - Darwin x86_64: `x86_64-apple-darwin`
   - Linux x86_64: `x86_64-unknown-linux-musl`
   - Linux aarch64: `aarch64-unknown-linux-musl`
2. Run `gh auth status`. If `gh` is missing or not logged in, stop. Ask the user to download
   `cctrack-<target>.tar.gz` and its `.sha256` from https://github.com/{{REPO}}/releases/tag/{{TAG}}
   into a new empty folder, and tell you the path.
3. In a new empty temp directory, run:
   ```
   gh release download {{TAG}} --repo {{REPO}} --pattern 'cctrack-<target>.tar.gz*'
   shasum -a 256 -c cctrack-<target>.tar.gz.sha256
   ```
4. Install it into `~/.local/bin`:
   ```
   mkdir -p ~/.local/bin
   tar -xzf cctrack-<target>.tar.gz -C ~/.local/bin
   ```
   On macOS, also run `xattr -d com.apple.quarantine ~/.local/bin/cctrack 2>/dev/null || true`.
5. Check that `~/.local/bin/cctrack --version` works. From here on, always use the full path
   `~/.local/bin/cctrack`, never a `cctrack` found on `PATH`.

## 2. Sign in

Do **not** run the login command yourself, because it waits for a browser.

1. If `~/.local/bin/cctrack status` already exits 0, skip to step 3.
2. Ask the user to type this in Claude Code, then wait for them:
   ```
   ! ~/.local/bin/cctrack login --server {{SERVER_URL}}
   ```
   On a machine without a browser, they should add `--paste` and run it in a real terminal.
3. Run `~/.local/bin/cctrack status` (it must exit 0) and `~/.local/bin/cctrack whoami`. Show the
   user the output.

Never print or read `~/.config/cctrack/credentials.json`.

## 3. Check the proxy mode

Run `~/.local/bin/cctrack proxy status` and show the user the output.

- cctrack detects on its own whether Claude Code goes through a local API proxy, such as a
  CLIProxyAPI account pool. It checks `ANTHROPIC_BASE_URL` in the environment and in Claude Code's
  settings files.
- Behind a pool, usage is credited to the pool account that served each request.
- If the detection looks wrong, tell the user. They can force it with
  `~/.local/bin/cctrack proxy on` or `~/.local/bin/cctrack proxy off`.

## 4. Install the plugin

1. Check that `claude --version` is 2.1.295 or newer. If it isn't, stop and tell the user.
2. Run:
   ```
   claude plugin marketplace add {{REPO}}
   claude plugin install cctrack@cctrack
   claude plugin list
   ```
   Confirm that `cctrack` is listed as enabled.

## 5. Finish

1. Tell the user to restart Claude Code.
   - From then on, Claude Code refuses to work while they're signed out.
   - It uploads usage after every turn.
   - After meaningful work, it attaches a short summary written by Haiku.
2. Explain why this session can't upload a summary yet: it started before the plugin was
   installed, so cctrack doesn't know its session id.
3. Give the user a short summary:
   - the binary path and version;
   - who they're signed in as;
   - the proxy mode and what it detected;
   - whether the plugin is enabled.

Their sessions appear on the dashboard at {{SERVER_URL}}.
