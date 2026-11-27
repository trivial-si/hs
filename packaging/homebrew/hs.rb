class Hs < Formula
  desc "Local time tracking, tasks, and focus sessions for your terminal"
  homepage "https://github.com/trivial-si/hs"
  version "0.1.2"
  license "GPL-3.0-or-later"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-aarch64.tar.gz"
      sha256 "1be375ac5c70164fff4f4307a07d1e37e6c3b7109b2e5e4347f0c7cdbc807a03"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-x86_64.tar.gz"
      sha256 "ccab3cca41493321275ef27a66913fdea6c07c899b97c1ed930bcd49cc94ce6b"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-aarch64.tar.gz"
      sha256 "b350d7cfb1f696a265030754099d23a8e8873ffca557370351a61b9153fc1622"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-x86_64.tar.gz"
      sha256 "6f8dc9f0b701129549506814afb5916ebc4838a47d0ac63a5504620b7528d78b"
    end
  end

  def install
    bin.install "hs"
    bash_completion.install "completions/hs.bash" => "hs"
    zsh_completion.install "completions/_hs"
    fish_completion.install "completions/hs.fish"
    doc.install "NOTICE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hs --version")
    db = testpath/"trial.db"
    system bin/"hs", "--db", db, "task", "add", "Test task", "--quadrant", "plan"
    assert_match "Test task", shell_output("#{bin}/hs --db #{db} matrix")
  end
end
