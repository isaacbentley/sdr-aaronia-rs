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
  version "0.13.3"
  license "GPL-3.0-or-later"

  # Prebuilt modules: the plugin statically links its Rust core, so there
  # is nothing to build here and no Rust toolchain to depend on.
  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/isaacbentley/sdr-aaronia-rs/releases/download/v#{version}/SoapyAaronia-#{version}-macos-arm64.dmg",
          using: :nounzip
      sha256 "REPLACE_WITH_SHA256_OF_MACOS_ARM64_ARCHIVE"
    end
  end

  on_linux do
    depends_on arch: :x86_64

    on_intel do
      url "https://github.com/isaacbentley/sdr-aaronia-rs/releases/download/v#{version}/SoapyAaronia-#{version}-linux-x86_64.tar.gz"
      sha256 "REPLACE_WITH_SHA256_OF_LINUX_X86_64_ARCHIVE"
    end
  end

  depends_on "soapysdr"

  def install
    # Discover SoapySDR's module ABI version dynamically rather than
    # hardcoding "0.8" across future SoapySDR releases.
    info = Utils.safe_popen_read(Formula["soapysdr"].opt_bin/"SoapySDRUtil", "--info")
    abi = info[/ABI Version:\s*v?([0-9]+\.[0-9]+)/i, 1] || "0.8"
    mod_dir = lib/"SoapySDR/modules#{abi}"

    if OS.mac?
      dmg = Dir["*.dmg"].first
      odie "No .dmg archive found in buildpath" unless dmg
      mountpoint = buildpath/"dmg_mount"
      mountpoint.mkpath
      system "hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mountpoint, dmg
      begin
        so_file = Pathname.glob(mountpoint/"**/libaaroniaSupport.so").first
        odie "libaaroniaSupport.so not found inside mounted DMG" unless so_file
        mod_dir.install so_file
        install_doc = Pathname.glob(mountpoint/"**/INSTALL.md").first
        doc.install install_doc if install_doc
      ensure
        system "hdiutil", "detach", mountpoint, "-force"
      end
    else
      so_file = Pathname.glob("**/libaaroniaSupport.so").first
      odie "libaaroniaSupport.so not found in archive" unless so_file
      mod_dir.install so_file
      install_doc = Pathname.glob("**/INSTALL.md").first
      doc.install install_doc if install_doc
    end
  end

  test do
    # Proves the module loads into SoapySDR rather than merely existing.
    assert_match "aaronia", shell_output("#{Formula["soapysdr"].opt_bin}/SoapySDRUtil --check=aaronia")
  end
end
