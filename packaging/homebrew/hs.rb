class Hs < Formula
  desc "Local time tracking, tasks, and focus sessions for your terminal"
  homepage "https://github.com/trivial-si/hs"
  version "0.1.3"
  license "GPL-3.0-or-later"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-aarch64.tar.gz"
      sha256 "94d4785445d5bbde3f732787e4421bd9c3b8bb04052f466a6f2a7fec713c5891"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-macos-x86_64.tar.gz"
      sha256 "af2818259098ee77eac0ab0b6d9897ec5c4c38a1a7e882df934d7dbfe41808d5"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-aarch64.tar.gz"
      sha256 "a9317e963c95a4c51634c2a5b8fb83517218db89a46d59c4d8b06b1d05f97882"
    else
      url "https://github.com/trivial-si/hs/releases/download/v#{version}/hs-v#{version}-linux-x86_64.tar.gz"
      sha256 "ee4bf97fecb3f904f201c8b9f76100d08322d80a7eedee637deb3703a773e4ff"
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
