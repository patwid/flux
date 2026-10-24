# Flux

Flux is a GTK4/libadwaita streaming frontend written in Rust using Relm4.
It takes its inspiration from [NewPipe](https://newpipe.net/) (Android), but it
targets **mobile Linux first** (Phosh, Plasma Mobile; for example PinePhone and Librem 5)
and also runs on desktop Linux.

This file is the project spec and the working agreement for anyone
contributing to the code, human or agent.

## Goals

- A privacy-friendly YouTube client. It needs no Google account and has no ads.
- **Responsive UI**: it must work at 360×640 (phone portrait) and scale up to desktop windows.
- **Features (v1)**:
  1. **Search**: search for videos and channels.
  2. **Playback**: play videos in-app, with quality selection and seeking.
  3. **Subscriptions**: subscribe to channels and get an aggregated feed from RSS.
  4. **History**: keep a watch history with resume position, and let the user clear it.
  5. **Download**: download videos or audio to disk, with progress and cancel. Downloaded items can be played offline.

### Non-goals (v1)

- Background/audio-only playback and MPRIS
- Importing NewPipe or OPML subscriptions
- Flatpak packaging
- Sites other than YouTube. The backend is still kept behind a trait, so other sites can be added later.
- Comments, playlists, and account login

## Technology

| Concern       | Choice                                                                 |
|---------------|------------------------------------------------------------------------|
| Language      | Rust (stable, edition 2024)                                             |
| UI            | GTK4 and libadwaita via `relm4` (The Elm Architecture). Target the **latest GNOME** (currently GNOME 50: relm4 feature `gnome_50`, libadwaita 1.9, GTK 4.22) and prefer modern libadwaita widgets (e.g. `AdwSpinner`, `AdwMultiLayoutView`, `AdwViewSwitcherSidebar`, `AdwToggleGroup`, `AdwWrapBox`) over older or GTK-only equivalents. |
| Extraction    | `rustypipe` (YouTube internal API, pure Rust)                           |
| Subscriptions | YouTube channel RSS (`https://www.youtube.com/feeds/videos.xml?channel_id=…`) |
| Playback      | GStreamer (`playbin` + `gtk4paintablesink` → `gtk::Picture`)            |
| Storage       | SQLite via `rusqlite`, with schema migrations and the DB in `$XDG_DATA_HOME/flux/` |
| Downloads     | `rustypipe-downloader` (needs ffmpeg for muxing), saved to `$XDG_DOWNLOAD_DIR/Flux/` |
| Async         | `tokio`, through Relm4 commands/workers                                         |

App ID: `io.github.patwid.Flux`

## Architecture

Pure logic is kept separate from GTK so that most of the code can be tested
without a display.

```
flake.nix, nix/        # Nix flake: package, app, dev shell
migrations/            # numbered SQL schema migrations (append-only)
src/
  main.rs              # app bootstrap only
  lib.rs               # crate root (so integration tests in tests/ can use it)
  core/                # no gtk imports allowed
    model.rs           # domain types: Video, Channel, Stream, HistoryEntry, Download…
    backend.rs         # trait Backend (search, video details, streams, channel); RustyPipeBackend impl
    feed.rs            # RSS/Atom parsing → Vec<Video>; feed aggregation and sorting
    db.rs              # rusqlite: migrations, subscriptions, history, downloads repos
    download.rs        # download manager state machine (queued/running/done/failed/cancelled)
  player.rs            # GStreamer playback engine (no gtk; sinks are injected)
  ui/                  # relm4 components
    app.rs             # root: AdwApplicationWindow, navigation, breakpoints
    search.rs  feed.rs  subscriptions.rs  history.rs  downloads.rs  player.rs  video_detail.rs
```

### TEA conventions

- Every component has a `Model`, an `Input`/`Msg` enum, and an `update`. Side effects such as network, DB, and the filesystem
  run as Relm4 commands, and their results come back as messages.
- The state transitions of each component live in a **plain function or struct
  that does not depend on GTK**, for example `fn update(state: &mut State, msg: Msg) -> Vec<Effect>`.
  The Relm4 component is a thin adapter: it calls the pure update and then runs the
  returned `Effect`s. This is what makes the TEA logic unit-testable.
- IO is reached only through traits (`Backend`, the repositories, a `Clock`), so tests
  can inject fakes.

### Responsive layout

- `AdwNavigationView` for drill-down (list → video detail → player).
- Top-level sections are Feed, Search, Subscriptions, History, and Downloads, and they live in an `AdwViewStack`.
  - An `AdwMultiLayoutView` provides two layouts, and the stack is placed in each through an `AdwLayoutSlot`:
    - `narrow`: an `AdwViewSwitcherBar` at the bottom.
    - `wide`: an `AdwViewSwitcherSidebar` in an `AdwOverlaySplitView`.
- An `AdwBreakpoint` (max-width 550sp) switches the layout, rather than size-allocation hacks.
- Touch-friendly: list rows have adequate hit targets, and nothing depends on hover.
- The player goes fullscreen in landscape or on request.

## Offline demo

`cargo run --example demo` starts Flux against the fake backend with an in-memory database
and generated test videos. Search for "demo". Use it to check UI changes without network access,
e.g. under `xvfb-run` with screenshots at phone (360×720) and desktop sizes.

## Building with Nix

A Nix flake (`flake.nix`, with the package in `nix/package.nix`) builds and runs the app on
`x86_64-linux` and `aarch64-linux`. It tracks `nixos-unstable`, which provides the latest GNOME.

```sh
nix run              # build and run Flux
nix build            # build into ./result (runs the test suite)
nix develop          # shell with the Rust toolchain and all system libraries
nix flake update     # bump nixpkgs
```

- The flake only sees files tracked by git. `git add` new files before running `nix build`.
- Whenever a change needs a new system library, GStreamer plugin or runtime tool, add it to
  `nix/package.nix` in the same commit.
- The `src` file set in `nix/package.nix` lists the paths the build needs. Extend it when adding
  top-level directories that the build or the tests read.

## Development process: TDD (mandatory)

Each change follows **red → green → refactor**:

1. **Red**: write a test for the next small behaviour first. Run it and **confirm it
   fails** for the expected reason, not because of a compile error in unrelated code.
2. **Green**: write the minimal implementation that makes the test pass. Run the
   test and confirm it passes.
3. **Refactor**: clean up while all tests stay green.
4. Run `./scripts/check.sh` (fmt check, clippy with `-D warnings`, full test suite)
   before committing. If it fails, do not commit.

The tests are the specification of the behaviour. The implementation is validated
by them, not by manual inspection alone.

### Test guidelines

- Unit tests sit next to the code (`#[cfg(test)] mod tests`). Cross-module tests go in `tests/`.
- **No network in tests.** Use fakes for `Backend`. Feed parsing is tested against
  recorded fixtures in `tests/fixtures/`.
- DB tests use an in-memory SQLite (`Connection::open_in_memory()`) with migrations applied.
- TEA logic is tested by feeding messages to the pure `update` and asserting both the
  resulting state and the emitted effects.
- GTK widget code is kept thin. Where a widget test is needed, it runs under a headless
  display (e.g. `xvfb-run` or `GDK_BACKEND=broadway`) and is marked `#[ignore]` unless the
  environment supports it.
- Tests that hit the real network (e.g. rustypipe smoke tests) are `#[ignore]` and run
  manually.

## Version control

- Use git, and make **one commit per change**: one TDD cycle, or one coherent step.
- A commit message is a **single short line** in imperative mood, e.g. `Add RSS feed parser`.
  It has **no body and no trailers** (no `Co-Authored-By`, no `Signed-off-by`).
- Never commit a red test suite. The red step is run locally, and the test and its implementation are
  committed together once they are green. A setup step with no testable behaviour (e.g. scaffolding, docs) is
  its own commit.

## Milestones

1. Scaffolding: cargo project, deps, empty adw window, CI-style checks
2. Core domain and DB: migrations, subscriptions, history, and download repos
3. RSS feed parsing and aggregation
4. Backend trait, rustypipe impl, and fake
5. Search UI
6. Video detail and playback (GStreamer)
7. Subscriptions and feed UI
8. History, with resume position
9. Downloads: manager, UI, and offline playback
10. Responsive polish and mobile testing
