class Lightsout < Formula
  desc "Run contract, guards, gates and dashboard for unattended jobs"
  homepage "https://github.com/darylfung96/lightsout"
  url "https://github.com/darylfung96/lightsout/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "e54c38e363d0e6194d56cae2bed712d13268b09f6a7e3d8fb2c7492129f185d1"
  license "MIT"
  head "https://github.com/darylfung96/lightsout.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", "--locked", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/lightsout --version")
    system bin/"lightsout", "init", testpath/"office"
    assert_path_exists testpath/"office/lightsout.toml"
    assert_path_exists testpath/"office/jobs/noop/job.sh"
  end
end
