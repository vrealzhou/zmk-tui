class ZmkTui < Formula
  desc "Terminal UI for editing ZMK keymaps over USB or Bluetooth"
  homepage "https://github.com/vrealzhou/zmk-tui"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.0/zmk-tui-0.1.0-universal-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.0/zmk-tui-0.1.0-universal-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.0/zmk-tui-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "zmk-tui"
  end

  def caveats
    <<~EOS
      zmk-tui is an interactive terminal application; run it from a TTY.

      On macOS, grant Bluetooth permission to your terminal app before
      connecting to a keyboard over BLE.

      On Linux, the prebuilt binary uses the distribution's libdbus-1 and
      libudev (normally already installed on desktop systems).
    EOS
  end

  test do
    assert_match "zmk-tui", shell_output("#{bin}/zmk-tui --version")
    assert_match "ZMK Studio", shell_output("#{bin}/zmk-tui --help")
  end
end
