# Homebrew tap

The live tap is
[`darylfung96/homebrew-lightsout`](https://github.com/darylfung96/homebrew-lightsout).
This directory is the source copy of the formula; the tap copy carries the
same `url` and `sha256`.

A live tap needs a real checksum, because Homebrew refuses a formula whose
`sha256` does not match its `url`; after tagging a release, copy
`lightsout.rb` into `darylfung96/homebrew-lightsout/Formula/`, set `url` to the
tag tarball and fill `sha256` with:

```sh
curl -fsSL https://github.com/darylfung96/lightsout/archive/refs/tags/v0.1.0.tar.gz \
  | shasum -a 256
```

Then users install with:

```sh
brew install darylfung96/lightsout/lightsout
```

The formula builds from source with `cargo install --locked`, so macOS
Gatekeeper does not quarantine a downloaded binary and no notarization secrets
are needed on the tap side. The release workflow still codesigns prebuilt
binaries for users who install from GitHub Releases.
