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
`lib/SoapySDR/modules<ABI>`, and its `brew test` step confirms
`SoapySDRUtil --check=aaronia` reports the driver as present.

## Each release

The release workflow renders this file against the published archives,
with the version and both SHA-256 checksums filled in, and uploads it
as the **`homebrew-formula` workflow artifact** — not a release asset.
Download it from the release run's Artifacts section, copy it into the
tap repository as `Formula/soapy-aaronia.rb`, and commit. Nothing needs
hashing by hand.

It is deliberately not attached to the release page: Homebrew 4 and
later cannot install from a formula URL, so there it would be a file no
user could act on, sitting among ones they can. Once the tap exists,
the better move is to have the workflow commit to it directly and skip
this step.

Before pushing, `brew audit --strict --online soapy-aaronia` checks the
formula, and `brew install --build-from-source` proves it works.

## Coverage

The formula covers macOS on Apple silicon and Linux on x86-64, matching
the platforms the release builds. Elsewhere, `brew` has nothing to
install and the plugin has to be built from source; see
[soapy-aaronia/README.md](../../soapy-aaronia/README.md).

The module directory follows the installed SoapySDR: the formula reads
the ABI version from `SoapySDRUtil --info` and installs into
`modules<ABI>`, falling back to `modules0.8` only when that output cannot
be parsed. A SoapySDR ABI bump needs no change here.

The two `sha256` lines in the checked-in formula are placeholders, not
checksums. Homebrew refuses a formula whose `sha256` is not 64 hex digits,
so the file as it sits in this repository cannot install anything; only
the copy the release workflow renders can. The same release also
publishes `SHA256SUMS` and a build-provenance attestation, so the archive
checksums in the rendered formula can be checked against either
(`gh attestation verify <archive> --repo isaacbentley/sdr-aaronia-rs`).
