# Install and operate

LightsOut is a single Rust binary plus a workspace. The binary owns the verbs,
gates, dashboard and workspace lifecycle; the workspace owns the shell job
libraries, job directories and state. Installing the binary globally and
creating a workspace per project matters because upgrades then replace one
executable while each project keeps its own state; the two lifecycles do not
share a directory.

## Install the binary

### From a checkout

```sh
cargo install --locked --path .
```

This puts `lightsout` in `~/.cargo/bin`, so make sure that directory is on
`PATH`. `cargo install --path .` is the development install; it compiles from
the local sources and includes the workspace templates in the binary.

### From a release

Each release publishes `lightsout-<target>.tar.gz` and a sibling `.sha256` for:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`

The installer verifies the archive checksum before writing anything and
defaults to `$HOME/.local/bin`. Each release also publishes the installer and
its own `install.sh.sha256`, so the script can be verified before it runs:

```sh
base=https://github.com/darylfung96/lightsout/releases/download/v0.1.0
tmp=$(mktemp -d)
curl -fsSL "$base/install.sh" -o "$tmp/install.sh"
curl -fsSL "$base/install.sh.sha256" -o "$tmp/install.sh.sha256"
( cd "$tmp" && shasum -a 256 -c install.sh.sha256 )
sh "$tmp/install.sh" --version 0.1.0
```

The short form is also supported, but it executes the downloaded script
immediately, so the download-and-inspect form above is the safer habit:

```sh
curl -fsSL https://github.com/darylfung96/lightsout/releases/download/v0.1.0/install.sh \
  | sh -s -- --version 0.1.0
```

Installer options:

```text
--version V       install a specific version, or "latest"
--prefix DIR      install under DIR/bin (default $HOME/.local)
--workspace DIR   run `lightsout init DIR` after installing
--dry-run         print the plan; download and write nothing
```

The installer never uses `sudo`; choose a writable `--prefix` or create the
target directory first. It prints a note when the prefix is not on `PATH`,
because a successful install that cannot be invoked is still a broken setup.
For a fully pinned download, set `LIGHTSOUT_SHA256` to the archive hash you
obtained out-of-band; the installer then verifies that value instead of
trusting a checksum fetched from the same release.

### Homebrew

From the published tap:

```sh
brew install darylfung96/lightsout/lightsout
```

The tap formula builds from source with `cargo install --locked`, because a
source build avoids shipping a quarantined macOS binary and does not need
notarization secrets on the tap side. The formula template is in
[`packaging/homebrew/lightsout.rb`](../packaging/homebrew/lightsout.rb); copy it
into the tap after the first tag, because the live formula needs that tag's
checksum.

## Create a workspace

```sh
lightsout init ~/my-office
cd ~/my-office
lightsout doctor
lightsout run noop
```

`init` writes a versioned marker and the minimum runnable office:

```text
lightsout.toml
lib/            guards, locks, notify, agent, memory, repairs, heal, prompts/
jobs/noop/      the smallest complete fixture
triage.conf     the auto-repair policy; all human by default
spec/           schema and a digest example
bin/            scaffold, job, report and lightsout launchers
.gitignore      state and output exclusions
```

The marker records the workspace schema and the manager version that wrote it:

```toml
schema = 1
manager = "0.1.0"
```

Once the marker exists, `init` is idempotent: it reports the existing
workspace and writes nothing, so a repeated setup command cannot overwrite
local edits. A workspace written before auto-repair and agent profiles lacks
`triage.conf`, `lib/heal.sh`, `lib/prompts/repair.md` and `agents.conf`;
`doctor` names each missing file, and copying it from a fresh `init` is enough
because the run contract is additive. It refuses to overwrite existing LightsOut
files and writes nothing when any collision is found; `--dry-run` prints the same plan without
creating the directory or any file. An existing `.gitignore` is kept rather
than replaced, because the file often holds rules the project owned before
LightsOut arrived.

The binary finds the workspace by walking up from the current directory for
`lightsout.toml`, or from its own installation directory, or through
`LIGHTSOUT_REPO`. `--workspace DIR` overrides both for one command:

```sh
lightsout --workspace ~/my-office status --all
```

`bin/job`, `bin/report` and `bin/lightsout` in the workspace are convenience
launchers; they call the installed binary. The long-standing `bin/job <verb>`
spelling therefore keeps working.

## Check the installation

`lightsout doctor` reads the marker, verifies the shell libraries and the
repair prompt, then checks the tools the jobs actually reference:

```sh
lightsout doctor
```

- `bash` is required because jobs are shell.
- `python3` is required when a job mentions `python3` or turns on `U_MEMORY=1`;
  the memory gate and several job plans use it.
- `ffprobe` and `ffmpeg` are checked when a job mentions duration, resolution or
  loudness gates.
- `osascript` is checked on macOS for the desktop notification fallback.

A missing required tool is an error and exits `1`; a missing optional tool is a
warning and still exits `0`. The command never changes the workspace; it only
reads the marker, libraries, job files and `PATH`, so it is safe on a
production checkout.

## Upgrade and uninstall

The binary and the workspace have separate lifecycles, because the binary is
replaced by the package manager while the workspace holds project state.
Upgrade the binary with Homebrew, `cargo install`, or the installer, then run
`lightsout doctor` to see whether the workspace schema or manager version
changed. An upgrade never touches job directories or state, since those live
under the workspace rather than under the installed prefix.

Uninstall removes the binary only; workspaces remain wherever you created them:

```sh
rm ~/.local/bin/lightsout
```

## Release process

Tagging `v0.1.0` runs `.github/workflows/release.yml`, which tests the crate,
builds the four target archives, generates a `.sha256` beside each archive, and
attaches them to the GitHub Release. The workflow is also manual-dispatchable,
so an operator can rebuild assets from `main` without creating a tag.

For prebuilt macOS assets, set these repository secrets to enable Developer ID
codesigning and optional notarization:

```text
MACOS_CERT_P12
MACOS_CERT_PASSWORD
MACOS_SIGNING_IDENTITY
MACOS_NOTARY_APPLE_ID
MACOS_NOTARY_TEAM_ID
MACOS_NOTARY_PASSWORD
```

Without the certificate secrets the macOS assets are unsigned; the Homebrew
path builds locally and avoids that issue. The signing helper is
[`packaging/macos-sign.sh`](../packaging/macos-sign.sh); it signs with a
temporary keychain and optionally submits a zip to `notarytool`.

## Verify the package tests

```sh
cargo test
tests/run.sh        # macOS reference suite, builds the release binary first
tests/install.sh    # installer against a local fake release, no network
```

The Rust suite keeps the 261 contract checks and adds workspace lifecycle
tests; the shell suite is still useful because it drives the real shell jobs
through the same contract, so a manager change has to satisfy both
implementations. The contract fixtures call macOS tools such as `md5`,
`stat -f` and `date -r`, so the full `cargo test` and `tests/run.sh` run on
macOS; `tests/workspace_cli.rs`, the unit tests and `tests/install.sh` are
portable and run on Linux CI as well.
