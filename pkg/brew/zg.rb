class Zg < Formula
  desc "Local-first filesystem query engine with regex and SQLite-backed search"
  homepage "https://github.com/cnrpman/zettagrep"
  head "https://github.com/cnrpman/zettagrep.git", branch: "master"
  license "MIT OR Apache-2.0"

  depends_on "rust" => :build
  depends_on "ripgrep"

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    (testpath/"note.md").write("TODO item\n12345\n67890")
    assert_match "TODO item", shell_output("#{bin}/zg TODO #{testpath}/note.md")
  end
end
