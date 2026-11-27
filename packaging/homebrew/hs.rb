class Hs < Formula
  desc "Local time tracking, tasks, and focus sessions for your terminal"
  homepage "https://github.com/trivial-si/hs"
  version "0.1.2"
  license "GPL-3.0-or-later"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-aarch64.tar.gz"
      sha256 "REPLACE_WITH_MACOS_AARCH64_SHA256"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-x86_64.tar.gz"
      sha256 "REPLACE_WITH_MACOS_X86_64_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-aarch64.tar.gz"
      sha256 "REPLACE_WITH_LINUX_AARCH64_SHA256"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-x86_64.tar.gz"
      sha256 "REPLACE_WITH_LINUX_X86_64_SHA256"
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
