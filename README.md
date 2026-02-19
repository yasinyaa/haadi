# haadi

`haadi` is a Rust CLI that scans JavaScript/TypeScript projects and reports:

- A report summary (coverage and confidence metrics)
- Unused files (not reachable from detected/provided entries)
- Unused asset files (images/fonts/media/styles not referenced by reachable source files)
- Unused dependencies (declared in `package.json` but never imported/required)
- Unused exports (exported symbols not imported by other files)

The default mode is conservative to reduce false positives.

## Build

```bash
cargo build --release
```

## Usage

```bash
cargo run -- --root /path/to/project
```

Or run the built binary directly:

```bash
./target/release/haadi --root /path/to/project
```

Optional flags:

```bash
cargo run -- --root /path/to/project \
  --entry src/index.ts \
  --entry src/cli.ts \
  --include-non-prod-deps \
  --include-low-confidence \
  --tui \
  --json
```

## TUI mode

Launch an interactive dashboard:

```bash
cargo run -- --root /path/to/project --tui
```

Controls:

- Summary page:
  - `d`: open delete page
  - `q` or `Esc`: quit
- In delete page:
  - `j`/`k` or arrows: move
  - `Space`/`Enter`: select or unselect item
  - `a`: select all
  - `c`: clear selection
  - `x`: request delete for selected items
  - `y`: approve pending action (delete or empty trash)
  - `n` or `Esc`: cancel pending action
  - `u`: undo last approved delete batch
  - `z`: request empty trash
  - `b`: back to summary page
  - `q`: quit

## Notes

- Output includes a `summary` section (in both text and JSON) with totals and confidence status.
- TUI deletes are reversible: deleted files are moved into `.haadi_trash/sessions/*` and logged in `.haadi_trash/deletions.jsonl`.
- Entry points are auto-detected from `package.json` fields (`main`, `module`, `types`, `browser`, `bin`, `exports`) and common defaults (`src/index.*`, `src/main.*`, `index.*`).
- Pass `--entry` explicitly for best accuracy.
- Regex-based static analysis cannot perfectly model runtime behavior; review findings before deleting code.
