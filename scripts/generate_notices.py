"""Build a versioned, auditable license bundle from the Windows Cargo graph.

Run from any directory with Python 3. Uses cached Cargo packages (offline) and
retrieves only the pinned WebView2 license texts omitted by their crate archives.
No binaries, authentication data, user paths, or browser profiles are collected.
--check validates the existing bundle without changing files.
"""
from pathlib import Path
import argparse
import hashlib
import html
import json
import re
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
SDK_VERSION = "1.0.3800.47"
WEBVIEW_REVISIONS = {
    "webview2-com": ("0.39.1", "edc2caf886175ccaebe86078c9cfe1ae2a187328"),
    "webview2-com-sys": ("0.39.1", "edc2caf886175ccaebe86078c9cfe1ae2a187328"),
    "webview2-com-macros": ("0.8.1", "dffa41a8a46d3f5565eefbff2de57d38d399f158"),
}

def command(*args):
    return subprocess.check_output(args, cwd=ROOT)

def fetch(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    metadata = json.loads(command("cargo", "metadata", "--offline", "--locked",
                                  "--format-version", "1", "--filter-platform", TARGET))
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    todo, resolved = [metadata["resolve"]["root"]], set()
    while todo:
        identity = todo.pop()
        if identity not in resolved:
            resolved.add(identity)
            todo.extend(nodes[identity]["dependencies"])
    # Include build/proc-macro dependencies conservatively, but not other target graphs.
    packages = sorted((p for p in metadata["packages"]
                       if p["id"] in resolved and p["id"] != metadata["resolve"]["root"]),
                      key=lambda p: (p["name"], p["version"]))
    output, components = {}, []
    def retain(data, original, origin):
        digest = hashlib.sha256(data).hexdigest()
        path = "third-party/licenses/" + digest + ".txt"
        output[path] = data
        return {"original_name": original, "source": origin, "path": path, "sha256": digest}
    for package in packages:
        name, version = package["name"], package["version"]
        directory = Path(package["manifest_path"]).parent
        source = f"https://crates.io/crates/{name}/{version}"
        notices = []
        # Preserve upstream additional notices, COPYRIGHT files, and SPDX attributions.
        for file in sorted(directory.rglob("*")):
            if (file.is_file() and not file.name.startswith(".")
                    and any(word in file.name.upper() for word in
                            ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE"))):
                notices.append(retain(file.read_bytes(), file.relative_to(directory).as_posix(), source))
        if name in WEBVIEW_REVISIONS:
            expected_version, revision = WEBVIEW_REVISIONS[name]
            vcs = json.loads((directory / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
            if version != expected_version or vcs["git"]["sha1"] != revision:
                raise RuntimeError("Update the pinned WebView2 license and SDK provenance before regenerating")
            url = f"https://raw.githubusercontent.com/wravery/webview2-rs/{revision}/LICENSE"
            notices.append(retain(fetch(url), "LICENSE (upstream repository root)", url))
        if not notices:
            raise RuntimeError(f"No license/notice text found for {name} {version}")
        components.append({"name": name, "version": version, "declared_license": package["license"],
                           "source": source, "notices": notices})
    # The native loader linked by webview2-com-sys has its own Microsoft license;
    # the Rust wrapper's MIT grant does not replace this notice.
    sdk_url = f"https://www.nuget.org/packages/Microsoft.Web.WebView2/{SDK_VERSION}/License"
    page = fetch(sdk_url).decode("utf-8")
    block = re.search(r"<pre[^>]*>(.*?)</pre>", page, re.S)
    if not block:
        raise RuntimeError("NuGet license page structure changed")
    license_text = html.unescape(block.group(1))
    if "Copyright (C) Microsoft Corporation" not in license_text:
        raise RuntimeError("Unexpected WebView2 SDK license")
    sdk_notice = retain(license_text.encode("utf-8"), "Microsoft.Web.WebView2 license", sdk_url)
    # Rust's installed standard-library copyright bundle includes platform-specific
    # notices conservatively. Retain its full upstream content, not just MIT/Apache.
    sysroot = Path(command("rustc", "--print", "sysroot").decode().strip())
    rust_file = sysroot / "share/doc/rust/COPYRIGHT-library.html"
    output["third-party/rust/COPYRIGHT-library.html"] = rust_file.read_bytes()
    rust_version = command("rustc", "--version").decode().strip()
    manifest = {"target": TARGET, "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
                "rust_toolchain": rust_version, "components": components,
                "webview2_sdk": {"version": SDK_VERSION, "notice": sdk_notice,
                    "provenance": "https://github.com/wravery/webview2-rs/blob/edc2caf886175ccaebe86078c9cfe1ae2a187328/crates/update-bindings/src/main.rs"},
                "rust_standard_library": {"path": "third-party/rust/COPYRIGHT-library.html",
                    "sha256": hashlib.sha256(rust_file.read_bytes()).hexdigest()}}
    output["third-party/manifest.json"] = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode()
    lines = ["# Third-party notices", "", "GPT File Browser's own code is licensed under [MIT](LICENSE). Third-party components retain their own licenses and copyright notices.", "",
             f"This bundle covers the locked `{TARGET}` dependency graph, including build dependencies. It records `{rust_version}`. Other targets or toolchains require regenerated/reviewed notices.", "",
             "Ship `LICENSE`, this file, and the entire `third-party/` directory with distributed executables. Keep all referenced license and notice texts intact. Identical texts are stored once by SHA-256; each component links to its applicable texts. Including alternative license texts does not turn an upstream OR license into an AND requirement.", "",
             "## Cargo dependencies", "", "| Component | Declared license | Upstream texts |", "| --- | --- | --- |"]
    for c in components:
        links = "; ".join(f"[{n['original_name']}]({n['path']})" for n in c["notices"])
        lines.append(f"| [{c['name']} {c['version']}]({c['source']}) | {c['declared_license']} | {links} |")
    lines += ["", "## Native components and runtime", "",
              f"- Microsoft WebView2 SDK loader {SDK_VERSION}: [copyright and license]({sdk_notice['path']}). Its version comes from the exact webview2-com-sys upstream revision recorded in the [manifest](third-party/manifest.json).",
              "- Rust standard library: [complete upstream copyright/license bundle](third-party/rust/COPYRIGHT-library.html). This deliberately preserves notices for multiple platforms rather than removing potentially applicable attributions.",
              "- Microsoft Edge WebView2 Runtime is a separately installed prerequisite, not included in this source or executable distribution. It retains Microsoft's terms and its own third-party notices. Windows SDK/MSVC tools are likewise not distributed here.", "",
              "## Maintaining this bundle", "",
              "Run `python scripts/generate_notices.py` after dependency or Rust toolchain updates. Run `python scripts/generate_notices.py --check` to verify the checked-in bundle. Cargo metadata uses the local cache; the script fetches the pinned WebView2 upstream license and official NuGet license page. It stops if the pinned WebView2 version/revision changes and needs review.", "",
              "The [manifest](third-party/manifest.json) records exact versions, public provenance URLs, the Cargo.lock digest, and every notice digest. Public upstream copyright-holder names/emails are intentional legal attributions, not local user/session data.", ""]
    output["THIRD_PARTY_NOTICES.md"] = "\n".join(lines).encode()
    for path, data in output.items():
        destination = ROOT / path
        if args.check:
            if not destination.exists() or destination.read_bytes() != data:
                raise RuntimeError("Notice bundle differs: " + path)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
    # Surface obsolete files instead of silently leaving them in a release bundle.
    obsolete = [p.relative_to(ROOT).as_posix() for p in (ROOT / "third-party").rglob("*")
                if p.is_file() and p.relative_to(ROOT).as_posix() not in output]
    if obsolete:
        raise RuntimeError("Review/remove obsolete notice files: " + ", ".join(obsolete))
    print(f"{'Verified' if args.check else 'Generated'} {len(components)} Cargo components; {len(output)} notice/index files.")

if __name__ == "__main__":
    main()
