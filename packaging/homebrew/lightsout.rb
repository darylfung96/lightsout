class Lightsout < Formula
  desc "Run contract, guards, gates and dashboard for unattended jobs"
  homepage "https://github.com/darylfung96/lightsout"
  url "https://github.com/darylfung96/lightsout/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "b99eecaaf4b22cfc08355f81dac3416a3df88b4c27d3f2365d97b50e3522dac9"
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
    assert_path_exists testpath/"office/triage.conf"
    assert_path_exists testpath/"office/lib/heal.sh"
  end
end
