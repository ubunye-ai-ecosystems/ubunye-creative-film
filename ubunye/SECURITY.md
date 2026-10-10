# Security policy: Ubunye Creative Film

## Reporting
Please **do not** open public issues for vulnerabilities. Use GitHub's private reporting ("Security" → "Report a vulnerability") on `ubunye-ai-ecosystems/ubunye-creative-film`. We acknowledge within 5 working days.
If the issue also affects upstream FilmCraft, we coordinate disclosure with upstream (https://github.com/storytold/filmcraft).

## Supported versions
Only the latest `vX.Y.Z-ubunye.N` release. Pre-1.0 builds are previews.

## Threat model summary
- **Untrusted files.** Every importer (MP4/MOV, MKV/WebM, MXF, MPEG-TS, Ogg, WAV, AAF, OMF, FCP XML, H.264, HEVC, VP9, AV1, ProRes, DNx, MPEG-2, AAC, Opus, AC-3, captions, LUTs) parses attacker-controlled data with in-house Rust parsers. Fuzzing status: deterministic mutation tests only; no cargo-fuzz harnesses yet (Milestone 2.5).
- **Local automation.** The `--control` port and the MCP server can read and write files. They are off by default; never expose them beyond localhost. **The upstream control port has no authentication** and can write files anywhere while enabled (fix planned in Milestone 2.7).
- **Downloads.** Optional AI model downloads are user-initiated and hash-pinned upstream.
- **Supply chain.** `deny.toml` + `cargo audit` in CI (`ubunye-audit.yml`); toolchain pinned; Actions pinned by SHA in Ubunye workflows.

## Known advisories at import (cargo audit, 2026-10-09 DB)
- RUSTSEC-2023-0071 `rsa` 0.9.10, plus RUSTSEC-2026-0049/0098/0099/0104 `rustls-webpki` 0.102.8, both via `rustls-rustcrypto` 0.0.2-alpha (`crates/speech`, optional `download` feature).
- Warnings: unsound `glib` 0.18 (Linux menus via muda); unmaintained `paste`, `proc-macro-error`.
