class ZmkTui < Formula
  desc "Terminal UI for editing ZMK keymaps over USB or Bluetooth"
  homepage "https://github.com/vrealzhou/zmk-tui"
  url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.0/zmk-tui-0.1.0-universal-apple-darwin.tar.gz"
  version "0.1.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"

  depends_on :macos

  def install
    bin.install "zmk-tui"
  end

  def caveats
    <<~EOS
      zmk-tui is an interactive terminal application; run it from a TTY.

      On macOS, grant Bluetooth permission to your terminal app before
      connecting to a keyboard over BLE.
    EOS
  end

  test do
    assert_match "zmk-tui", shell_output("#{bin}/zmk-tui --version")
    assert_match "ZMK Studio", shell_output("#{bin}/zmk-tui --help")
  end
end
