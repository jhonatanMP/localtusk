#!/usr/bin/env python3
"""Gera src-tauri/resources/postgres-versions.json, o catálogo de versões do PostgreSQL embutido no app.

Os binários vêm do projeto theseus-rs/postgresql-binaries (builds para macOS e Linux com psql,
pg_dump e o servidor completo). O app não consulta a API do GitHub em tempo de execução (limite de 60
requisições por hora sem login), então o catálogo é gerado aqui e vai junto com o app.

Guarda o patch mais novo de cada versão principal (14, 15, 16...) com tamanho e SHA-256 de cada
arquivo, que o app confere depois do download.

Uso:  python3 scripts/update_postgres_versions.py
"""
import json
import urllib.request
from datetime import date
from pathlib import Path

REPO = "theseus-rs/postgresql-binaries"
MIN_MAJOR = 14
# chave no catálogo -> alvo do pacote (macOS e Linux)
TARGETS = {
    "arm64": "aarch64-apple-darwin",
    "x86_64": "x86_64-apple-darwin",
    "linux-aarch64": "aarch64-unknown-linux-gnu",
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "win-x86_64": "x86_64-pc-windows-msvc",
}
OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "resources" / "postgres-versions.json"


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "LocalTusk-script", "Accept": "application/vnd.github+json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def key(v):
    return tuple(int(x) for x in v.split("."))


def main():
    best = {}
    for page in (1, 2, 3):
        releases = get(f"https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}")
        if not releases:
            break
        for r in releases:
            tag = r["tag_name"]
            parts = tag.split(".")
            if len(parts) != 3 or not all(p.isdigit() for p in parts) or r.get("prerelease") or r.get("draft"):
                continue
            major = int(parts[0])
            if major < MIN_MAJOR:
                continue
            builds = {}
            for arch, target in TARGETS.items():
                name = f"postgresql-{tag}-{target}.tar.gz"
                asset = next((a for a in r["assets"] if a["name"] == name), None)
                digest = (asset or {}).get("digest") or ""
                if asset and digest.startswith("sha256:"):
                    builds[arch] = {"bytes": asset["size"], "sha256": digest[7:]}
            if len(builds) == len(TARGETS) and (str(major) not in best or key(tag) > key(best[str(major)]["version"])):
                best[str(major)] = {"version": tag, "major": str(major), "builds": builds}

    versions = sorted(best.values(), key=lambda v: key(v["version"]), reverse=True)
    OUT.write_text(json.dumps({"generated": date.today().isoformat(), "versions": versions}, indent=1) + "\n", encoding="utf-8")
    for v in versions:
        print(v["version"], {a: b["bytes"] // 1024 // 1024 for a, b in v["builds"].items()}, "MB")
    print("gravado em", OUT)


if __name__ == "__main__":
    main()
