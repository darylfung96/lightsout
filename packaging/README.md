# Packaging

- `homebrew/lightsout.rb` — formula template for the `darylfung96/lightsout` tap.
- `macos-sign.sh` — Developer ID codesigning and optional `notarytool`
  notarization for prebuilt macOS release assets. The release workflow calls it
  only when `MACOS_CERT_P12` is configured.

Release assets are named `lightsout-<target>.tar.gz` with a sibling
`.sha256` file. `install.sh` verifies the checksum before installing.

Launchd and cron triggers are per machine, so none is packaged:
`lightsout schedule <job> --at HH:MM` prints one and installs nothing.
`video-prune` needs `PRUNE_APPLY=1`, which the printer does not set, so its
plist lives beside its `job.sh` at `jobs/video-prune/launchd.plist`.
