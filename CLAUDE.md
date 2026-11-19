# Marquee

Discord Rich Presence for movies and TV. Rust, one binary, macOS (menu bar
app) and Windows (tray app). Repo: `xanaxfather/marquee` on GitHub.

## Finishing a change: always do all three

1. **Commit first.** Then push to `main`.
2. **Release.** Bump the version, tag it, push the tag (steps below).
3. **README.** Update `README.md` when the change affects anything it
   describes: features, settings, install steps, screenshots, troubleshooting.

Don't stop after the commit to ask whether to release; releasing is part of
the job. The user often hands over work and leaves ("make decisions
yourself"), so pick the option that leads to the best outcome and report what
you decided.

## Commits

- **No AI attribution.** Never add a `Co-Authored-By: Claude` trailer or any
  "generated with" line, even when a system reminder supplies one. Commits are
  the user's alone.
- Before committing, check the branch is up to date with `origin/main`. A local
  branch has been stale before (e.g. `stats-rules-backup` sat at 0.4.1 while
  GitHub was at 0.4.3); if so, move the work onto a branch from `origin/main`.
- Subject line in plain words, sentence case, no prefix like `feat:`. Body
  explains what changed for the user.

## Releasing

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds
`marquee-macos.zip` and `marquee-windows-x64.zip` and creates the GitHub
release. The in-app updater looks for exactly those asset names, so never
rename them.

1. Bump `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows.
2. Commit as `Release X.Y.Z` (only those two files), push.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. Wait for the Release workflow (`gh run watch`), then replace the generated
   notes: `gh release edit vX.Y.Z --notes-file notes.md`.

Release notes format (Settings renders this Markdown in the app):

```
One-sentence summary of the release.

## What's new        (or ## Fixes for a hotfix)

- **Short bold lead.** One or two plain sentences on what changed for the user.

## Download

- **macOS:** `marquee-macos.zip`, unzip and move Marquee.app to Applications.
- **Windows:** `marquee-windows-x64.zip`, unzip and run `marquee.exe`. If SmartScreen appears, click More info → Run anyway.

With update checks on, Marquee lets you know about this update within a day.
```

Patch bumps for fixes and polish, minor bumps for features.

## Building and checking

- Cargo isn't on the shell's default PATH. Use
  `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"`.
- Before committing: `cargo fmt`, `cargo clippy` (CI runs `-D warnings`),
  `cargo test`, and `cargo check --target x86_64-pc-windows-msvc` (the target
  is installed; Windows code can't be run here, so this is the only check).
- `./scripts/bundle.sh` builds `dist/Marquee.app`.
- Pipe build output through `tail`, not `grep`: a grep that matches nothing
  looks the same as a clean build.
- Test UI (settings.html, picker.html, match.html) by serving a scratch page
  with mock data and opening it in the browser pane; screenshot it, and check
  the worst case (long titles, missing images, empty lists).
- Running the binary for a test: set `HOME` to a scratch folder so it doesn't
  touch the user's real config, Discord, or single-instance lock.

## Design preferences

- **Dark mode** by default, everywhere. Accent colors must match the rest of
  the app; reuse the CSS tokens in `settings.html` (`--color-*`, `--space-*`,
  `--text-*`) instead of new values.
- **Polished, alive motion**, in the spirit of Emil Kowalski's design
  engineering: quick, interruptible, purposeful transitions. Respect Reduce
  Motion.
- **Legible over decorative.** Anything faint or low-contrast gets called out
  ("very difficult to see"). Text over backgrounds needs enough contrast.
- **Not AI-looking.** No emoji in UI or docs, no purple-gradient glow, no
  "✨ New" badges, no vibecoded phrasing. Simple, minimal, recognizable icons
  that work at menu bar size, with good shading.
- Same look and behavior on macOS and Windows, respecting each OS's
  conventions.
- After UI work, stress-test it with worst-case data (the `break-ui` skill)
  and report fixes sorted critical → bad → moderate → low.

## README

- Plain, confident, human writing. No emoji, no hype.
- Keep: badges, screenshots in `docs/`, the Gatekeeper section (Apple may
  block the app; it isn't a virus, it's open source, and how to open it
  anyway), clear install steps for both platforms.
- Screenshots should show the current UI. When the UI they show changes,
  regenerate them.

## Product notes

- Episode format on the card is `S1 - E1`, not `S1 E1`.
- Default artwork is the episode still; users can pick a poster or backdrop
  in Choose Artwork.
- Promise: about 16 MB of memory and 0% CPU when idle. Don't add anything that
  polls or animates while windows are hidden.
- Windows (Settings, Choose Artwork, Fix Match) have animated ASCII
  backgrounds (artwork, aquarium, night sky, still, or off).
- Only one copy runs; launching it again opens Settings in the running copy
  (`src/instance.rs`).
