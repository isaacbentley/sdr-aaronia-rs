# Homebrew formula for the Aaronia SoapySDR plugin.
#
# Lives here so it is versioned with the code it installs. To publish it,
# copy this file into a tap repository named homebrew-tap, as
# Formula/soapy-aaronia.rb, and users get:
#
#     brew install <owner>/tap/soapy-aaronia
#
# At each release, update `version`, both `sha256` values (from the
# release archives) and nothing else. `brew audit --strict soapy-aaronia`
# checks the formula before publishing.
class SoapyAaronia < Formula
  desc "SoapySDR plugin for Aaronia SPECTRAN V6 spectrum analyzers"
  homepage "https://github.com/isaacbentley/sdr-aaronia-rs"
  version "0.7.3"
  license "GPL-3.0-or-later"

  # Prebuilt modules: the plugin statically links its Rust core, so there
  # is nothing to build here and no Rust toolchain to depend on.
  on_macos do
    on_arm do
      url "https://github.com/isaacbentley/sdr-aaronia-rs/releases/download/v#{version}/SoapyAaronia-#{version}-macos-arm64.tar.gz"
      sha256 "REPLACE_WITH_SHA256_OF_MACOS_ARM64_ARCHIVE"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/isaacbentley/sdr-aaronia-rs/releases/download/v#{version}/SoapyAaronia-#{version}-linux-x86_64.tar.gz"
      sha256 "REPLACE_WITH_SHA256_OF_LINUX_X86_64_ARCHIVE"
    end
  end

  depends_on "soapysdr"

  def install
    # SoapySDR loads modules from a versioned directory under its own
    # prefix; Homebrew links this into place for us.
    (lib/"SoapySDR/modules0.8").install "libaaroniaSupport.so"
    doc.install "INSTALL.md"
  end

  test do
    # Proves the module loads into SoapySDR rather than merely existing.
    assert_match "aaronia", shell_output("#{Formula["soapysdr"].bin}/SoapySDRUtil --check=aaronia")
  end
end
