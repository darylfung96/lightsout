class Lightsout < Formula
  desc "Run contract, guards, gates and dashboard for unattended jobs"
  homepage "https://github.com/darylfung96/lightsout"
  url "https://github.com/darylfung96/lightsout/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "41c056d8554c15024844170490eecbb07223ea2c4df394de4bfed0794b8578db"
  license "MIT"
  head "https://github.com/darylfung96/lightsout.git", branch: "main"

  depends_on "rust" => :build

  def install
    # std_cargo_args already passes --locked; cargo refuses a repeated flag.
    system "cargo", "install", *std_cargo_args
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
