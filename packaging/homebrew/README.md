# Homebrew tap

`soapy-aaronia.rb` installs the prebuilt SoapySDR plugin from a GitHub
release. It is kept here so it is versioned alongside the code it
installs, and published from a tap repository.

## One-time setup

Create a public repository named `homebrew-tap` under the same owner as
this one. The name is fixed: Homebrew maps `owner/tap` to
`github.com/owner/homebrew-tap`. Add the formula as
`Formula/soapy-aaronia.rb`.

Users then install with:

```bash
brew install isaacbentley/tap/soapy-aaronia
```

Homebrew pulls in SoapySDR, drops the module into
`lib/SoapySDR/modules0.8`, and its `brew test` step confirms
`SoapySDRUtil --check=aaronia` reports the driver as present.

## Each release

The release workflow renders this file against the published archives
and attaches the result to the release as `soapy-aaronia.rb`, with the
version and both SHA-256 checksums filled in. Copy that file into the
tap repository as `Formula/soapy-aaronia.rb` and commit. Nothing needs
hashing by hand.

Before pushing, `brew audit --strict --online soapy-aaronia` checks the
formula, and `brew install --build-from-source` proves it works.

## Coverage

The formula covers macOS on Apple silicon and Linux on x86-64, matching
the platforms the release builds. Elsewhere, `brew` has nothing to
install and the plugin has to be built from source; see
[soapy-aaronia/README.md](../../soapy-aaronia/README.md).

The module directory is pinned to `modules0.8`, which is SoapySDR's ABI
0.8 path. A future SoapySDR ABI bump requires changing that line.
