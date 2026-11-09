# mdBook AI Skill

Auto-updated [mdBook](https://rust-lang.github.io/mdBook/) documentation for AI coding assistants.

Uses the open [Agent Skills](https://agentskills.io) standard (SKILL.md). Works with **33+ AI coding assistants** including Claude Code, Cursor, Windsurf, GitHub Copilot, OpenAI Codex, Gemini CLI, Amp, OpenCode, Cline, Aider, Goose, Roo Code, and [many more](https://agentskills.io/clients).

References are **auto-generated** daily from the official [mdBook guide](https://github.com/rust-lang/mdBook/tree/main/guide) via GitHub Actions.

## Install

### Quick start

```bash
# Claude Code (global)
git clone https://github.com/marceloeatworld/mdbook-ai-skill.git ~/.claude/skills/mdbook

# Cursor
git clone https://github.com/marceloeatworld/mdbook-ai-skill.git .cursor/skills/mdbook

# Windsurf
git clone https://github.com/marceloeatworld/mdbook-ai-skill.git .windsurf/skills/mdbook

# Cross-agent standard
git clone https://github.com/marceloeatworld/mdbook-ai-skill.git .agents/skills/mdbook
```

### With the install script

```bash
curl -fsSL https://raw.githubusercontent.com/marceloeatworld/mdbook-ai-skill/main/install.sh | bash -s -- --claude
curl -fsSL https://raw.githubusercontent.com/marceloeatworld/mdbook-ai-skill/main/install.sh | bash -s -- --cursor
```

Run `./install.sh --help` for all options.

## Update

```bash
git -C <install-path> pull
```

## What it covers

| Category | Topics |
|----------|--------|
| **Getting Started** | Installation, creating a book, reading books |
| **CLI** | init, build, watch, serve, test, clean, completions |
| **Book Format** | SUMMARY.md syntax, Markdown extensions, mdBook-specific features, MathJax |
| **Configuration** | book.toml (general, preprocessors, renderers, environment variables) |
| **Theme** | Custom themes, index.hbs, syntax highlighting, editor integration |
| **For Developers** | Writing preprocessors, writing backends (Rust API) |
| **Deployment** | GitHub Actions, GitLab CI, GitHub Pages |

## How it works

The `SKILL.md` maps topics to reference files. The `references/` directory contains the **full unmodified guide** from the mdBook repository.

A [GitHub Actions workflow](.github/workflows/update-references.yml) runs daily to keep references in sync.

## Structure

```
mdbook-ai-skill/
├── SKILL.md                              # Topic -> reference routing table
├── install.sh                            # Multi-tool install script
├── scripts/
│   └── generate-references.sh            # Fetches guide and generates references
├── .github/workflows/
│   └── update-references.yml             # Daily auto-update
└── references/                           # Auto-generated from mdBook guide
    ├── .wiki-version                     # Source commit tracker
    ├── cli.md                            # All CLI commands
    ├── configuration.md                  # book.toml reference
    ├── summary-format.md                 # SUMMARY.md syntax
    ├── theme.md                          # Theme customization
    └── ...
```

## Credits

- [mdBook](https://github.com/rust-lang/mdBook) by the Rust team and contributors
