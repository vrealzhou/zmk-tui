class ZmkTui < Formula
  desc "Terminal UI for editing ZMK keymaps over USB or Bluetooth"
  homepage "https://github.com/vrealzhou/zmk-tui"
  version "0.1.3"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.3/zmk-tui-0.1.3-universal-apple-darwin.tar.gz"
      sha256 "af1ede23622dc1f4f5e7a504a980bae1a2c2205435adb6611e68f25515f57f57"
    end
    on_intel do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.3/zmk-tui-0.1.3-universal-apple-darwin.tar.gz"
      sha256 "af1ede23622dc1f4f5e7a504a980bae1a2c2205435adb6611e68f25515f57f57"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/vrealzhou/zmk-tui/releases/download/v0.1.3/zmk-tui-0.1.3-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "bb994447ada8b3fa5dc012d293d71b855d63280c9352bc8d4136f529166d9d63"
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
