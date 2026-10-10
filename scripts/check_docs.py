#!/usr/bin/env python3
"""Check the public environment contract and reproducible build pins."""
from pathlib import Path
import re
import tomllib

root = Path(__file__).resolve().parent.parent
config = (root / "src/config.rs").read_text()
keys_block = config.split("const KEYS:", 1)[1].split("];", 1)[0]
actual = {"INGESTOR_" + name for name in re.findall(r'"([A-Z_]+)"', keys_block)}
readme = (root / "README.md").read_text()
documented = set(re.findall(r"`(INGESTOR_[A-Z_]+)`", readme))
assert actual == documented, f"configuration mismatch: {actual ^ documented}"
manifest = tomllib.loads((root / "Cargo.toml").read_text())
toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
assert manifest["package"]["edition"] == "2024"
assert toolchain.startswith(manifest["package"]["rust-version"] + ".")
for dependency in manifest["dependencies"].values():
    version = dependency if isinstance(dependency, str) else dependency["version"]
    assert version.startswith("="), "implementation dependencies must be pinned"
for file in ["Dockerfile", ".github/workflows/ci.yml", ".devcontainer/devcontainer.json"]:
    assert toolchain in (root / file).read_text(), f"toolchain mismatch: {file}"
for line in (root / "Dockerfile").read_text().splitlines():
    if line.startswith("FROM "):
        assert re.search(r"@sha256:[a-f0-9]{64}", line), "container base must be immutable"
for file in (root / ".github/workflows").glob("*.yml"):
    text = file.read_text()
    for action in re.findall(r"uses: ([^\s]+)", text):
        assert action.startswith("./") or re.search(r"@[a-f0-9]{40}$", action), f"unpinned action: {action}"
    assert "pull_request_target" not in text
assert "Status:** architecture and scope only" not in readme
print("Documentation/configuration/build pins agree")
